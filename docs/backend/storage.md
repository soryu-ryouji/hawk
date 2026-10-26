# 存储设计

## 目录结构

hawk 不在素材文件和文件夹中写入数据，所有状态收敛在素材目录下的 `.hawk/` 隐藏文件夹中：

```text
你的素材目录/
├── 设计素材/
│   ├── 海报/
│   └── 图标/
├── 摄影/
└── .hawk/
    ├── config.toml        ← 项目配置（参与同步）
    ├── categories.toml    ← 分类注册表（参与同步）
    ├── tags.toml          ← 标签注册表（参与同步）
    ├── view.toml          ← 视图排序偏好（参与同步）
    ├── global_filter.toml ← 全局隐藏项（参与同步）
    ├── locks.toml         ← 文件夹/分类/标签锁（参与同步）
    ├── metadata.db        ← 素材元数据的唯一权威 SQLite（本地专用，不参与同步）
    └── trash/             ← 回收站（本地专用，不参与同步）
```

**素材元数据只有 SQLite 一条路线。** 旧 `.hawk/metadata/*.toml` 不读取、不迁移；`.hawk/storage_mode` 与库外 `index.db` 不再使用。其他 TOML 配置和小型注册表保持不变。

缩略图派生缓存位于库外，避免同步盘传播和膨胀：

```text
%LOCALAPPDATA%/hawk/cache/<库标识>/        ← Windows
~/.local/share/hawk/cache/<库标识>/        ← Linux
~/Library/Application Support/hawk/cache/<库标识>/  ← macOS

cache/<库标识>/thumbnails/
├── 512/<hash>.webp   # 网格
└── 1024/<hash>.webp  # 检查器与广域网预览
```

`<库标识>` = `<库文件夹名>_<库根路径SHA-256前16位>`。缓存父目录可在设置面板修改，迁移为整体搬迁；缓存可整体删除并由后台重建。调色板与 pHash 不是库外缓存，它们和宽高一起存入 `metadata.db`。

## 同步边界

| 内容 | 是否参与同步 | 说明 |
| ---- | ------------ | ---- |
| `config.toml` / `categories.toml` / `tags.toml` / `view.toml` | 是 | 配置与小型注册表 |
| `global_filter.toml` / `locks.toml` | 是 | 全局隐藏与锁 |
| `metadata.db` | 否 | 唯一素材元数据权威库；SQLite 跨机同步容易冲突或腐蚀 |
| `trash/` | 否 | 回收站仅本机可恢复 |
| 库外缩略图缓存 | 否 | 可重建 |

`.hawk/.gitignore` 自动排除 `trash/`、`remote/` 和 `metadata.db`。部分网盘客户端不识别 `.gitignore`，仍需在网盘客户端中手动排除 SQLite 文件。

## SQLite 元数据

`.hawk/metadata.db` 同时是权威存储和启动注水来源，不再有第二份镜像或回退格式：

- 单条写入：事务提交成功后更新内存副本。
- 批量写入：单个事务提交全部条目，成功后统一更新内存；失败返回错误，不静默降级。
- 启动：`load_all` 一次读取并恢复内存索引，无需先全库扫描。
- 数据库打不开、schema 初始化失败或读取失败：进入启动错误页，**不按空库继续写入**。
- 已有 v1 SQLite 数据只增量添加 `phash` / `phash_version` 列，原标签、评分、备注、位置与调色板保持不变；不进行 TOML 迁移。

主要表：

| 表 | 职责 |
| -- | ---- |
| `meta` | schema 版本等元信息 |
| `items` | hash、来源 URL、评分、备注、宽高、调色板、pHash 与算法版本 |
| `paths` | 同内容的一个或多个位置、大小与修改时间 |
| `tags` / `categories` | 标签与分类赋值 |
| `folders` | 增量扫描的目录快照 |

`source_mtime` 是旧 SQLite schema 留下的字段，当前不再承载业务语义。

## 派生数据与维护

宽高、调色板和 pHash 都是内容的纯函数，存于 `items`：

- `width` / `height`：图片尺寸，非图片或未知为 0。
- `palette`：JSON 数组，最多 10 色；`NULL` 表示未提炼，空数组是非图片负缓存。
- `phash`：16 位十六进制，供近重复查找；`phash_version=0` 表示未计算。
- 非图像的 pHash 使用 `version>0, value=NULL` 负缓存，避免周期重试；暂时解码失败保持未计算。

扫描、增量入库和周期维护复用同一 worker：单次解码后按需生成缩略图、调色板、宽高和 pHash。结果经 JobSender 回到单写者流水线，按 500 条或 2 秒批量写 SQLite，并合并 `items.updated`。周期维护间隔由 `HAWK_MAINTENANCE_INTERVAL` 控制（默认 60s，0 关闭）。

## 锁（文件夹/分类/标签）

`.hawk/locks.toml` 存放三维度锁（每条独立密码，Argon2id PHC 字符串自含盐与参数）。锁由服务端强制：未解锁时列表查询排除锁定条目，内容直连和相似查询源图校验返回 403，SSE 不广播被锁素材的元数据事件。

解锁票据只在 daemon 内存中保存，经 `X-Hawk-Unlock` 头或 `?unlock=` 查询参数附带。锁不是加密，文件系统层面无保护；判定细节见 [REST API V1](server-rest-api-v1.md)。

## 回收站

删除的素材或文件夹移入 `.hawk/trash/` 并保留目录结构。`metadata.db` 中仍保留原库内路径作为恢复目标；清空回收站时，只有内容在库内已无其他位置引用，才删除对应元数据与全部尺寸缩略图。

## 内存索引与启动

索引完全在内存中维护，不存在独立索引文件：

1. 从 `metadata.db` 注水元数据与位置。
2. 后台扫描素材目录，仅先读路径、大小、mtime。
3. 路径和 size/mtime 与数据库一致则复用内容哈希；新文件计算哈希，变化文件按路径迁移元数据。
4. 消失的位置从索引移除，最新 size/mtime 回写 SQLite。
5. 运行期间由文件监听保持增量更新，周期扫描收敛漏事件。

需要强制重算全部哈希时调用 `POST /api/v1/library/reindex`。

## 内容寻址与 pHash

item id 是文件内容的 BLAKE3 哈希；同内容多位置共享一个 item 和派生数据。缩略图按 `<尺寸>/<hash>.webp` 寻址。

pHash 使用 `image_hasher` 的 DCT + Mean 算法生成 64 位值，SQLite 中保存为 16 位 hex，内存中保存为整数。算法版本从 1 开始，变化时由维护任务重算。详见 [pHash 相似图片查找](similar-images.md)。

缓存文件直接平铺在尺寸目录下，不按哈希分桶；访问按内容哈希直接寻址，不枚举目录。

## 项目配置

`.hawk/config.toml` 保存项目名、ignore、扩展名白名单、周期重扫和局域网访问配置，随素材目录同步。该文件与分类/标签/视图/隐藏/锁注册表仍是 TOML，不属于本次移除的素材元数据路线。

远程接入使用应用级 `~/.config/hawk/remote.toml`，设备身份与访问密钥在同级 `remote/`；这些配置跨素材库共享，不进入 `metadata.db`。

## 实时文件监听

文件系统监听负责运行期增量更新。漏事件由启动扫描、周期兜底扫描、监听缓冲溢出重扫和系统 must-scan 提示收敛；`HAWK_FS_RESCAN_INTERVAL` 控制文件系统兜底重扫，与派生维护的 `HAWK_MAINTENANCE_INTERVAL` 是两条独立路径。
