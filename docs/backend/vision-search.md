# 可选视觉扩展与相似构图

## 目标与边界

通过可选 DINOv2 视觉扩展检索配色、构图、风格和主体相近的图片，与“相同图片”的 dHash + RGB 近重复检索分离。模型只处理本地图片，不上传素材；扩展未安装或停用时，基础功能不受影响。

本阶段不包含相似角色、自然语言搜索和 GPU 加速。

## 扩展管理

- 模型：`onnx-community/dinov2-small` 的 `model_quantized.onnx`，384 维 CLS token 向量，约 24MB。
- 只有用户在设置 → 扩展或查找弹窗中主动触发后才下载；安装端点立即返回并在后台执行，客户端轮询进度，下载经 SHA-256 校验，使用临时文件安装。
- HNSW 重建与全库缺失扫描也在 worker 中执行，不阻塞 API 或启动线程；`indexing` 状态覆盖重建和当前任务，即使队列尚未形成，UI 也会持续刷新。全库路径统一转换为绝对路径后解码。
- 启用状态写入 `.hawk/vision.json`，模型位于库外缓存 `plugins/dinov2-small/model.onnx`。
- 支持启停和删除；删除模型保留已有向量，重新安装后复用。
- 远程素材库由承载它的 daemon 安装模型并建立索引。

## 推理与索引

```text
图片 → 短边 256 → 中心裁剪 224×224 → ImageNet 归一化
     → DINOv2 ONNX → CLS 384 维 → L2 归一化
     → SQLite vision_vectors → 内存 HNSW
```

- 向量表以 `(hash, model)` 为主键，模型 ID 为 `dinov2-small-quantized-v1`。
- 首次启用从 SQLite 重建 HNSW，并为缺失项排队；新增/更新图片经入库流程增量入队。插入与计数均按 item id 去重，重复对账不会放大索引数。
- 后台 worker 串行加载一次模型，失败写入状态；向量持久化成功后插入索引。
- 查询取 HNSW 近似候选，再按持久化向量计算精确余弦相似度，随后执行锁与可见性过滤。

## API

| 方法 | 端点 | 说明 |
| ---- | ---- | ---- |
| GET | `/api/v1/vision/status` | 安装、下载、启停和索引进度 |
| POST | `/api/v1/vision/install` | 下载并启用（admin） |
| POST | `/api/v1/vision/enable` | 启停（admin） |
| POST | `/api/v1/vision/remove` | 删除模型（admin） |
| POST | `/api/v1/item/vision-search` | 相似构图查询 |

查询参数：`id`、`min_similarity`（默认 0.7）、`offset`、`limit`（默认 60，最大 100）。响应按余弦相似度降序，每项包含 `item` 与 `similarity_score`。源图或索引未就绪返回 `NOT_READY`，前端自动等待后重试，而不是显示为参数错误。

## 前端

- 查找弹窗提供“相同图片 / 相似构图”独立页签。
- 构图阈值是 0–100 的最低余弦相似度，经 `hawk:visionSimilarityThreshold` 持久化，与近重复阈值分开。
- 未安装时提供下载入口；索引后台补齐，结果卡片显示百分比相似度。
- 设置 → 扩展提供下载进度、启停、索引计数和删除。

## 验证

- ONNX 输入形状与归一化、CLS 输出和 L2 归一化。
- 余弦相似度已知向量。
- 真实模型冒烟为显式 opt-in：设置 `HAWK_VISION_MODEL` 后运行 `real_dinov2_smoke`，常规 CI 不下载模型。
- OpenAPI、路由能力、成功路径、前端构图查询和扩展主动下载测试。
