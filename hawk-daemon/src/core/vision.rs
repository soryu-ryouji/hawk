//! 可选视觉扩展：DINOv2 模型下载、后台向量计算、SQLite 持久化与 HNSW 检索。

use crate::core::index::ItemIndex;
use crate::core::metadata_store::MetadataStore;
use crate::core::paths::LibraryPaths;
use crate::core::thumbnail::ThumbnailService;
use futures_util::StreamExt;
use hnsw_rs::prelude::*;
use ort::session::Session;
use ort::value::TensorRef;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

fn onnx_environment() -> &'static ort::environment::Environment {
    static ENVIRONMENT: OnceLock<ort::environment::Environment> = OnceLock::new();
    ENVIRONMENT.get_or_init(|| ort::init().build().expect("ONNX Runtime 初始化失败"))
}

pub const VISION_MODEL_ID: &str = "dinov2-small-quantized-v1";
const MODEL_URL: &str =
    "https://huggingface.co/onnx-community/dinov2-small/resolve/main/onnx/model_quantized.onnx";
const MODEL_SHA256: &str = "c179f8f7f592449c4c1bca4cd124a7538021428c5ffb89afde9503935b197efb";
const MODEL_BYTES: u64 = 24_446_700;
const VECTOR_DIM: usize = 384;

#[derive(Clone, Debug, Default, Serialize, utoipa::ToSchema)]
pub struct VisionStatus {
    pub installed: bool,
    pub enabled: bool,
    pub downloading: bool,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub indexed_items: usize,
    pub total_items: usize,
    pub pending_items: usize,
    /// 正在重建索引或处理当前任务（队列尚未形成时也覆盖）
    pub indexing: bool,
    pub error: Option<String>,
}

#[derive(Default, Deserialize, Serialize)]
struct VisionConfig {
    enabled: bool,
}

struct VectorIndex {
    hnsw: Hnsw<'static, f32, DistCosine>,
    ids: Vec<String>,
    vectors: HashMap<String, Vec<f32>>,
}

pub struct VisionService {
    paths: LibraryPaths,
    store: Arc<MetadataStore>,
    index: Arc<ItemIndex>,
    enabled: AtomicBool,
    started: AtomicBool,
    queue: Mutex<VecDeque<(String, String)>>,
    wake: Condvar,
    vector_index: Mutex<Option<VectorIndex>>,
    status: Mutex<VisionStatus>,
    environment: &'static ort::environment::Environment,
}

impl VisionService {
    pub fn new(paths: LibraryPaths, store: Arc<MetadataStore>, index: Arc<ItemIndex>) -> Arc<Self> {
        Arc::new(Self {
            paths,
            store,
            index,
            enabled: AtomicBool::new(false),
            started: AtomicBool::new(false),
            queue: Mutex::new(VecDeque::new()),
            wake: Condvar::new(),
            vector_index: Mutex::new(None),
            status: Mutex::new(VisionStatus {
                total_bytes: MODEL_BYTES,
                ..Default::default()
            }),
            environment: onnx_environment(),
        })
    }

    fn model_path(&self) -> String {
        format!("{}/plugins/dinov2-small/model.onnx", self.paths.cache_dir)
    }

    fn config_path(&self) -> String {
        format!("{}/vision.json", self.paths.hawk_dir)
    }

    fn read_enabled(&self) -> bool {
        std::fs::read_to_string(self.config_path())
            .ok()
            .and_then(|text| serde_json::from_str::<VisionConfig>(&text).ok())
            .is_some_and(|config| config.enabled)
    }

    fn save_enabled(&self, enabled: bool) -> Result<(), String> {
        std::fs::write(
            self.config_path(),
            serde_json::to_string(&VisionConfig { enabled }).map_err(|e| e.to_string())?,
        )
        .map_err(|e| format!("视觉扩展配置写入失败: {e}"))
    }

    pub fn status(&self) -> VisionStatus {
        let mut status = self.status.lock().unwrap().clone();
        status.installed = std::path::Path::new(&self.model_path()).is_file();
        status.enabled = self.enabled.load(Ordering::Relaxed);
        status.pending_items = self.queue.lock().unwrap().len();
        status.total_items = self.index.duplicate_items(&Default::default()).len();
        status.indexed_items = self
            .vector_index
            .lock()
            .unwrap()
            .as_ref()
            .map_or(0, |index| index.vectors.len());
        status
    }

    /// 启动已有扩展；模型下载不会在此自动发生。
    pub fn start(self: &Arc<Self>) {
        let enabled = self.read_enabled() && std::path::Path::new(&self.model_path()).is_file();
        self.enabled.store(enabled, Ordering::Relaxed);
        if enabled {
            self.start_worker();
        }
    }

    fn start_worker(self: &Arc<Self>) {
        if self.started.swap(true, Ordering::Relaxed) {
            self.wake.notify_one();
            return;
        }
        self.status.lock().unwrap().indexing = true;
        let service = self.clone();
        std::thread::Builder::new()
            .name("hawk-vision-worker".to_string())
            .spawn(move || {
                // HNSW 构建与全库缺失扫描放在 worker，避免阻塞 API/启动线程
                service.rebuild_index();
                service.worker_loop();
            })
            .expect("视觉 worker 启动失败");
    }

    fn rebuild_index(&self) {
        let vectors = self.store.vision_vectors(VISION_MODEL_ID);
        let ids: Vec<String> = vectors.iter().map(|(id, _)| id.clone()).collect();
        let hnsw =
            Hnsw::<f32, DistCosine>::new(16, vectors.len().max(10_000), 16, 200, DistCosine {});
        let mut by_id = HashMap::new();
        for (position, (id, vector)) in vectors.iter().enumerate() {
            if vector.len() == VECTOR_DIM {
                hnsw.insert((vector, position));
                by_id.insert(id.clone(), vector.clone());
            }
        }
        *self.vector_index.lock().unwrap() = Some(VectorIndex {
            hnsw,
            ids,
            vectors: by_id,
        });
        self.enqueue_missing();
    }

    fn enqueue_missing(&self) {
        for item in self.index.duplicate_items(&Default::default()) {
            if self
                .store
                .vision_vector(&item.id, VISION_MODEL_ID)
                .is_none()
            {
                if let Some(path) = item.locations.first() {
                    self.enqueue(&item.id, &path.path);
                }
            }
        }
    }

    pub fn enqueue(&self, id: &str, path: &str) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        let mut queue = self.queue.lock().unwrap();
        if queue.iter().any(|(queued, _)| queued == id) {
            return;
        }
        queue.push_back((id.to_string(), path.to_string()));
        drop(queue);
        self.wake.notify_one();
    }

    fn worker_loop(&self) {
        let mut session: Option<Session> = None;
        loop {
            let job = {
                let mut queue = self.queue.lock().unwrap();
                loop {
                    if let Some(job) = queue.pop_front() {
                        break Some(job);
                    }
                    if !self.enabled.load(Ordering::Relaxed) {
                        session = None;
                        break None;
                    }
                    self.status.lock().unwrap().indexing = false;
                    queue = self.wake.wait(queue).unwrap();
                }
            };
            let Some((id, path)) = job else {
                self.status.lock().unwrap().indexing = false;
                self.started.store(false, Ordering::Relaxed);
                return;
            };
            if !self.enabled.load(Ordering::Relaxed) {
                continue;
            }
            if session.is_none() {
                match Session::builder(self.environment)
                    .and_then(|mut builder| builder.commit_from_file(self.model_path()))
                {
                    Ok(loaded) => session = Some(loaded),
                    Err(e) => {
                        self.set_error(format!("视觉模型加载失败: {e}"));
                        self.status.lock().unwrap().indexing = false;
                        self.started.store(false, Ordering::Relaxed);
                        return;
                    }
                }
            }
            let abs = if std::path::Path::new(&path).is_absolute() {
                path.clone()
            } else {
                match self.paths.to_absolute(&path) {
                    Some(abs) => abs,
                    None => {
                        self.set_error(format!("图片路径无效: {path}"));
                        continue;
                    }
                }
            };
            let result = ThumbnailService::decode(&abs)
                .ok_or_else(|| "图片解码失败".to_string())
                .and_then(|image| embed(session.as_mut().unwrap(), &image));
            match result {
                Ok(vector) => {
                    if let Err(e) = self.store.save_vision_vector(&id, VISION_MODEL_ID, &vector) {
                        self.set_error(e);
                        continue;
                    }
                    let mut guard = self.vector_index.lock().unwrap();
                    let Some(index) = guard.as_mut() else {
                        continue;
                    };
                    insert_vector(index, &id, &vector);
                    self.status.lock().unwrap().indexed_items = index.vectors.len();
                }
                Err(e) => self.set_error(format!("视觉分析失败 {id}: {e}")),
            }
        }
    }

    fn set_error(&self, message: String) {
        tracing::error!("{message}");
        self.status.lock().unwrap().error = Some(message);
    }

    pub fn set_enabled(self: &Arc<Self>, enabled: bool) -> Result<(), String> {
        if enabled && !std::path::Path::new(&self.model_path()).is_file() {
            return Err("视觉模型尚未安装".to_string());
        }
        self.save_enabled(enabled)?;
        self.enabled.store(enabled, Ordering::Relaxed);
        if enabled {
            self.start_worker();
        } else {
            self.queue.lock().unwrap().clear();
            self.status.lock().unwrap().indexing = false;
            self.wake.notify_one();
        }
        Ok(())
    }

    /// 用户显式触发后才下载；下载完成前不启用模型。
    pub async fn install(self: &Arc<Self>) -> Result<(), String> {
        if std::path::Path::new(&self.model_path()).is_file() {
            return self.set_enabled(true);
        }
        {
            let mut status = self.status.lock().unwrap();
            if status.downloading {
                return Ok(());
            }
            status.downloading = true;
            status.downloaded_bytes = 0;
            status.error = None;
        }
        let result = self.download_model().await;
        self.status.lock().unwrap().downloading = false;
        match result {
            Ok(()) => self.set_enabled(true),
            Err(e) => {
                self.set_error(e.clone());
                Err(e)
            }
        }
    }

    async fn download_model(&self) -> Result<(), String> {
        let response = reqwest::get(MODEL_URL)
            .await
            .map_err(|e| format!("模型下载失败: {e}"))?
            .error_for_status()
            .map_err(|e| format!("模型下载失败: {e}"))?;
        let directory = format!("{}/plugins/dinov2-small", self.paths.cache_dir);
        std::fs::create_dir_all(&directory).map_err(|e| format!("模型目录创建失败: {e}"))?;
        let temp = format!("{directory}/model.download");
        let mut file =
            std::fs::File::create(&temp).map_err(|e| format!("模型文件创建失败: {e}"))?;
        let mut stream = response.bytes_stream();
        let mut hasher = Sha256::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("模型下载中断: {e}"))?;
            use std::io::Write;
            file.write_all(&chunk)
                .map_err(|e| format!("模型写入失败: {e}"))?;
            hasher.update(&chunk);
            self.status.lock().unwrap().downloaded_bytes += chunk.len() as u64;
        }
        drop(file);
        let actual = format!("{:x}", hasher.finalize());
        if actual != MODEL_SHA256 {
            let _ = std::fs::remove_file(&temp);
            return Err("模型校验失败，已删除损坏文件".to_string());
        }
        std::fs::rename(&temp, self.model_path()).map_err(|e| format!("模型安装失败: {e}"))?;
        Ok(())
    }

    pub fn remove(self: &Arc<Self>) -> Result<(), String> {
        self.set_enabled(false)?;
        let path = self.model_path();
        if std::path::Path::new(&path).is_file() {
            std::fs::remove_file(path).map_err(|e| format!("模型删除失败: {e}"))?;
        }
        Ok(())
    }

    /// HNSW 返回近似候选；score 仍按持久化向量精确计算余弦相似度。
    pub fn search(&self, id: &str, min_similarity: f32) -> Result<Vec<(String, f32)>, String> {
        if !self.enabled.load(Ordering::Relaxed) {
            return Err("视觉扩展尚未启用".to_string());
        }
        let source = self
            .store
            .vision_vector(id, VISION_MODEL_ID)
            .ok_or_else(|| "源图片尚未完成构图分析，请稍后重试".to_string())?;
        let guard = self.vector_index.lock().unwrap();
        let index = guard
            .as_ref()
            .ok_or_else(|| "构图索引尚未建立".to_string())?;
        let mut matches = Vec::new();
        for neighbour in index
            .hnsw
            .search(&source, index.ids.len(), index.ids.len().max(64))
        {
            let Some(candidate) = index.ids.get(neighbour.d_id) else {
                continue;
            };
            let Some(vector) = index.vectors.get(candidate) else {
                continue;
            };
            let score = cosine_similarity(&source, vector);
            if score >= min_similarity {
                matches.push((candidate.clone(), score));
            }
        }
        matches.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        Ok(matches)
    }
}

fn insert_vector(index: &mut VectorIndex, id: &str, vector: &[f32]) {
    // 对账/重复事件可能让同一 item 多次入队；IDs 与向量都必须按内容 id 去重
    if index.vectors.contains_key(id) {
        return;
    }
    let position = index.ids.len();
    index.hnsw.insert((vector, position));
    index.ids.push(id.to_string());
    index.vectors.insert(id.to_string(), vector.to_vec());
}

fn preprocess(image: &image::DynamicImage) -> Vec<f32> {
    let (width, height) = (image.width() as f32, image.height() as f32);
    let scale = 256.0 / width.min(height);
    let resized = image::DynamicImage::ImageRgba8(image::imageops::resize(
        image,
        (width * scale).round() as u32,
        (height * scale).round() as u32,
        image::imageops::FilterType::CatmullRom,
    ))
    .to_rgb8();
    let x = (resized.width().saturating_sub(224)) / 2;
    let y = (resized.height().saturating_sub(224)) / 2;
    let cropped = image::imageops::crop_imm(&resized, x, y, 224, 224).to_image();
    let mut data = vec![0.0; 3 * 224 * 224];
    let mean = [0.485, 0.456, 0.406];
    let std = [0.229, 0.224, 0.225];
    for (position, pixel) in cropped.pixels().enumerate() {
        for channel in 0..3 {
            data[channel * 224 * 224 + position] =
                (f32::from(pixel[channel]) / 255.0 - mean[channel]) / std[channel];
        }
    }
    data
}

fn embed(session: &mut Session, image: &image::DynamicImage) -> Result<Vec<f32>, String> {
    let data = preprocess(image);
    let input = TensorRef::from_array_view(([1usize, 3, 224, 224], data.as_slice()))
        .map_err(|e| e.to_string())?;
    let outputs = session
        .run(ort::inputs!["pixel_values" => input])
        .map_err(|e| format!("ONNX 推理失败: {e}"))?;
    let value = outputs.get("pooler_output").unwrap_or(&outputs[0]);
    let array = value
        .try_extract_array::<f32>()
        .map_err(|e| e.to_string())?;
    // Transformers.js 的 DINOv2 输出 last_hidden_state [1, tokens, 384]；图像检索取 CLS token
    let vector: Vec<f32> = if array.len() == VECTOR_DIM {
        array.iter().copied().collect()
    } else if array.len() % VECTOR_DIM == 0 {
        array.iter().take(VECTOR_DIM).copied().collect()
    } else {
        return Err(format!("模型输出维度异常: {}", array.len()));
    };
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm <= f32::EPSILON {
        return Err("模型输出为零向量".to_string());
    }
    Ok(vector.into_iter().map(|value| value / norm).collect())
}

pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot = a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
    let norm_a = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a <= f32::EPSILON || norm_b <= f32::EPSILON {
        0.0
    } else {
        dot / (norm_a * norm_b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vector_insert_is_deduplicated() {
        let mut index = VectorIndex {
            hnsw: Hnsw::new(2, 10, 2, 20, DistCosine {}),
            ids: Vec::new(),
            vectors: HashMap::new(),
        };
        let vector = vec![1.0; VECTOR_DIM];
        insert_vector(&mut index, "same-item", &vector);
        insert_vector(&mut index, "same-item", &vector);
        assert_eq!(index.ids.len(), 1);
        assert_eq!(index.vectors.len(), 1);
    }

    #[test]
    fn cosine_is_normalized() {
        assert!((cosine_similarity(&[1.0, 0.0], &[2.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!(cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        assert_eq!(cosine_similarity(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
    }

    #[test]
    #[ignore = "需设置 HAWK_VISION_MODEL 指向本地 DINOv2 ONNX，不进入常规测试"]
    fn real_dinov2_smoke() {
        let path = std::env::var("HAWK_VISION_MODEL").expect("HAWK_VISION_MODEL");
        let environment = onnx_environment();
        let mut session = Session::builder(environment)
            .unwrap()
            .commit_from_file(path)
            .unwrap();
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(256, 256, |x, y| {
            image::Rgba([(x % 256) as u8, (y % 256) as u8, 80, 255])
        }));
        let vector = embed(&mut session, &image).unwrap();
        assert_eq!(vector.len(), VECTOR_DIM);
        let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-4);
    }

    #[test]
    fn preprocessing_has_imagenet_chw_shape() {
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            500,
            300,
            image::Rgba([255, 0, 0, 255]),
        ));
        let data = preprocess(&image);
        assert_eq!(data.len(), 3 * 224 * 224);
        let expected = (1.0 - 0.485) / 0.229;
        assert!((data[0] - expected).abs() < 1e-5);
    }
}
