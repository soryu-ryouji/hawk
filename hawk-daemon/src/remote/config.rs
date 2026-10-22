//! 应用级远程访问配置（remote.toml）：接入参数跨素材库共享，设备身份放同级 remote/ 目录。
//! Electron 经 `--remote-config` 指向 `~/.config/hawk/remote.toml`；独立运行 daemon 时
//! 缺省为库内 `.hawk/remote.toml`（身份仍在 `.hawk/remote/`，与旧行为同位置）。
//! 写入只经设置面板/API（`PUT /api/v1/remote`）：HTTP 端点更新内存并唤醒 supervisor，
//! 手工改文件需重启 daemon 生效。

use crate::core::paths::LibraryPaths;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// QUIC 隧道默认 UDP 端口（UPnP 映射目标）
const DEFAULT_QUIC_PORT: u16 = 27373;

const DEFAULT_CONFIG: &str = r#"# hawk 远程访问配置（应用级，跨素材库共享）
# 由设置面板「远程」分区写入；手工修改后需重启 hawk-daemon 生效。
enabled = false
url = ""
service_key = ""
quic_port = 27373
writable = false
relay = false
"#;

/// 远程访问接入设置：自托管 hawk-server 的接入信息。
/// service_key 明文存放（与 [web] token 同级先例）
#[derive(Clone, PartialEq)]
pub struct RemoteSettings {
    pub enabled: bool,
    /// 服务端地址（http(s):// 开头；TLS 由前置反代终结）
    pub url: String,
    /// 服务端 service-key（池门禁；hawk-server tenant add 生成）
    pub service_key: String,
    /// QUIC 隧道 UDP 端口（UPnP 映射目标；0 = OS 分配）
    pub quic_port: u16,
    /// 允许广域网会话导入素材/新建文件夹（受限写，与 [web] 的局域网写权限独立）
    pub writable: bool,
    /// 使用中继（服务端内嵌 relay 提供时；消耗自建带宽，设备级开关，默认关闭）。
    /// 请求级热配置（同 writable，不触发信令重启）
    pub relay: bool,
}

impl Default for RemoteSettings {
    fn default() -> RemoteSettings {
        RemoteSettings {
            enabled: false,
            url: String::new(),
            service_key: String::new(),
            quic_port: DEFAULT_QUIC_PORT,
            writable: false,
            relay: false,
        }
    }
}

/// 应用级远程配置存储：文件位置决定作用域（显式路径 = 应用级；缺省 = 库级回退）
pub struct RemoteConfigStore {
    file: PathBuf,
    /// 设备身份目录（device_id、自签证书私钥）；由配置文件位置派生，与配置同作用域
    identity_dir: String,
    current: RwLock<RemoteSettings>,
}

impl RemoteConfigStore {
    /// 打开（必要时创建）配置文件并加载到内存。解析失败按默认值继续：远程保持关闭，
    /// 错误写日志，用户可在设置面板重新保存覆盖。
    /// paths 为库内回退位（<library>/.hawk/remote.toml）；远程查看模式无库，必须显式 path
    pub fn open(path: Option<&str>, paths: Option<&LibraryPaths>) -> Arc<RemoteConfigStore> {
        let file = match path.map(str::trim).filter(|p| !p.is_empty()) {
            Some(path) => PathBuf::from(path),
            None => Path::new(
                &paths
                    .expect("远程查看模式（无本地库）必须显式提供 --remote-config")
                    .hawk_dir,
            )
            .join("remote.toml"),
        };
        let identity_dir = file
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("remote")
            .to_string_lossy()
            .into_owned();
        let store = Arc::new(RemoteConfigStore {
            file,
            identity_dir,
            current: RwLock::new(RemoteSettings::default()),
        });
        store.ensure_default();
        match store.read_file() {
            Ok(settings) => *store.current.write().unwrap() = settings,
            Err(e) => {
                tracing::warn!("远程访问配置加载失败，按默认值继续: {e}");
            }
        }
        store
    }

    pub fn current(&self) -> RemoteSettings {
        self.current.read().unwrap().clone()
    }

    pub fn identity_dir(&self) -> &str {
        &self.identity_dir
    }

    /// 写回配置并热更新内存。调用方随后唤醒 remote supervisor，
    /// 由它按连接参数是否变化决定是否重启信令。
    pub fn update(&self, settings: &RemoteSettings) -> Result<(), String> {
        let file = &self.file;
        if !file.is_file() {
            self.ensure_default();
        }
        let text = std::fs::read_to_string(file)
            .map_err(|e| format!("读取远程配置失败 {}: {e}", file.display()))?;
        // 文件被手工改坏时从默认模板重建：保存动作本身即用户可见的修复入口，
        // 不让人卡在「设置面板保存也报解析失败」
        let mut doc = text.parse::<toml_edit::DocumentMut>().unwrap_or_else(|e| {
            tracing::warn!("远程配置解析失败，按默认模板重建: {e}");
            DEFAULT_CONFIG.parse().expect("默认远程配置应为合法 TOML")
        });
        doc["enabled"] = toml_edit::value(settings.enabled);
        doc["url"] = toml_edit::value(settings.url.trim());
        doc["service_key"] = toml_edit::value(settings.service_key.trim());
        doc["quic_port"] = toml_edit::value(i64::from(settings.quic_port));
        doc["writable"] = toml_edit::value(settings.writable);
        doc["relay"] = toml_edit::value(settings.relay);
        let tmp = file.with_extension("toml.tmp");
        std::fs::write(&tmp, doc.to_string())
            .map_err(|e| format!("远程配置写入失败 {}: {e}", tmp.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&tmp, file).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("远程配置替换失败 {}: {e}", file.display())
        })?;
        *self.current.write().unwrap() = settings.clone();
        Ok(())
    }

    fn ensure_default(&self) {
        if self.file.is_file() {
            return;
        }
        if let Some(parent) = self.file.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!("远程配置目录创建失败 {}: {e}", parent.display());
                return;
            }
        }
        if let Err(e) = std::fs::write(&self.file, DEFAULT_CONFIG) {
            tracing::warn!("远程配置初始化写入失败 {}: {e}", self.file.display());
        }
    }

    fn read_file(&self) -> Result<RemoteSettings, String> {
        let text = std::fs::read_to_string(&self.file)
            .map_err(|e| format!("读取远程配置失败 {}: {e}", self.file.display()))?;
        let table: toml::Value = toml::from_str(&text)
            .map_err(|e| format!("远程配置解析失败 {}: {e}", self.file.display()))?;
        let mut settings = RemoteSettings::default();
        if let Some(table) = table.as_table() {
            settings.enabled = table
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if let Some(url) = table.get("url").and_then(|v| v.as_str()) {
                settings.url = url.trim().to_string();
            }
            if let Some(key) = table.get("service_key").and_then(|v| v.as_str()) {
                settings.service_key = key.trim().to_string();
            }
            if let Some(port) = table.get("quic_port").and_then(|v| v.as_integer()) {
                if (0..=65535).contains(&port) {
                    settings.quic_port = port as u16;
                }
            }
            settings.writable = table
                .get("writable")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            settings.relay = table
                .get("relay")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
        }
        Ok(settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(dir: &Path) -> LibraryPaths {
        let root = dir.join("library");
        std::fs::create_dir_all(&root).unwrap();
        LibraryPaths::new(root.to_str().unwrap(), None)
    }

    #[test]
    fn explicit_path_is_app_level_and_creates_default() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config").join("remote.toml");
        let store = RemoteConfigStore::open(Some(file.to_str().unwrap()), None);

        let current = store.current();
        assert!(!current.enabled);
        assert_eq!(current.quic_port, DEFAULT_QUIC_PORT);
        assert!(file.is_file(), "首次打开应创建配置文件");
        assert_eq!(
            store.identity_dir(),
            file.parent().unwrap().join("remote").to_str().unwrap()
        );
    }

    #[test]
    fn update_persists_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("remote.toml");
        let store = RemoteConfigStore::open(Some(file.to_str().unwrap()), None);
        store
            .update(&RemoteSettings {
                enabled: true,
                url: "https://hawk.example.com/".to_string(),
                service_key: "secret".to_string(),
                quic_port: 28000,
                writable: true,
                relay: true,
            })
            .unwrap();

        let reloaded = RemoteConfigStore::open(Some(file.to_str().unwrap()), None);
        let current = reloaded.current();
        assert!(current.enabled);
        assert_eq!(current.url, "https://hawk.example.com/");
        assert_eq!(current.service_key, "secret");
        assert_eq!(current.quic_port, 28000);
        assert!(current.writable);
    }

    #[test]
    fn fallback_lives_in_library_hawk_dir() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(dir.path());
        let store = RemoteConfigStore::open(None, Some(&paths));
        assert_eq!(
            store.identity_dir(),
            Path::new(&paths.hawk_dir).join("remote").to_str().unwrap()
        );
        assert!(Path::new(&paths.hawk_dir).join("remote.toml").is_file());
    }
}
