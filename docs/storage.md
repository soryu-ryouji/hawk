# 存储设计

## 目录结构

hawk 不会在素材文件和文件夹中存放任何文件，所有数据收敛在素材目录下的 `.hawk/` 隐藏文件夹中：

```text
你的素材目录/
├── 设计素材/
│   ├── 海报/
│   └── 图标/
├── 摄影/
│   ├── 风景/
│   └── 人像/
└── .hawk/              ← hawk 只在目录下创建这个隐藏文件夹
    ├── config.toml     ← 项目级配置（参与同步）
    ├── metadata/       ← 素材参数，纯文本（参与同步）
    ├── thumbnails/     ← 缩略图缓存（本地专用，不参与同步，可重建）
    └── hawk.db         ← SQLite 索引缓存（本地专用，不参与同步，可重建）
```

## 同步边界

| 内容 | 是否参与同步 | 说明 |
|---|---|---|
| `config.toml` | 是 | 项目配置，随素材目录一起走 |
| `metadata/` | 是 | 素材参数，唯一数据源 |
| `thumbnails/` | 否 | 本地缓存，可重建 |
| `hawk.db` | 否 | 本地缓存，可重建 |

`.hawk/.gitignore` 由 hawk 自动生成，排除 `thumbnails/` 和 `hawk.db`。

注意：部分网盘客户端（OneDrive、Syncthing 等）不识别 `.gitignore`，需要用户在网盘客户端中手动配置排除规则。后续应在用户文档中说明。

## 纯文本元数据

所有素材参数（标签、评分、备注等）集中存放在 `.hawk/metadata/` 中，按内容哈希命名，每个素材对应一个独立的纯文本文件（TOML）。

## 索引缓存

`.hawk/hawk.db` 是 SQLite 索引缓存（开启 FTS5），用于实现毫秒级搜索。它可以随时删除重建——真正的「数据源」永远是 `metadata/` 下的纯文本文件。

换一台机器时，hawk 从同步过来的 `metadata/` 重新生成索引和缩略图。

## 项目配置

每个项目的设置保存在 `.hawk/config.toml` 中，随素材目录一起同步、备份：

```toml
# .hawk/config.toml 示例

# 项目名（界面显示用）
name = "设计素材库"

# 索引时忽略的路径
ignore = ["node_modules", "*.tmp"]

# 生成的缩略图尺寸
thumbnail_sizes = [256, 1024]
```

全局配置文件位于 `~/.config/hawk/config.toml`，只存放跨项目的全局设置（目前没有全局配置项）。

## 内容寻址（Content-Addressable）

每个文件通过其内容哈希（BLAKE3）唯一标识，缩略图也按 hash 存储，避免重复生成：

```text
.hawk/thumbnails/
├── 256/                    # 列表视图
│   ├── ab/
│   │   └── abcdef123...webp
│   └── cd/
└── 1024/                   # 预览面板
    ├── ab/
    └── cd/
```

## 实时文件监听

hawk 通过文件系统事件（FileSystemWatcher）实时感知变化，新增、删除、重命名、修改文件时，索引自动更新。
