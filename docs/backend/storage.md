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
    ├── categories.toml ← 分类注册表（参与同步）
    ├── tags.toml       ← 标签注册表（参与同步）
    ├── metadata/       ← 素材参数，纯文本（参与同步）
    └── trash/          ← 回收站（本地专用，不参与同步）

派生缓存（缩略图、调色板）不进 .hawk/，放在库外系统缓存目录，
避免库位于 iCloud/Dropbox 等同步盘时 .hawk/ 膨胀拖累同步：

```text
%LOCALAPPDATA%/hawk/cache/<库标识>/        ← Windows（iCloud 不同步）
~/.local/share/hawk/cache/<库标识>/        ← Linux
~/Library/Application Support/hawk/cache/<库标识>/  ← macOS
```

`<库标识>` 为库根路径 SHA-256 的前 16 位十六进制，多库互不干扰。缓存按内容哈希寻址，可整体删除，重启扫描自动重建。

## 同步边界

| 内容          | 是否参与同步 | 说明                       |
| ------------- | ------------ | -------------------------- |
| `config.toml`     | 是           | 项目配置，随素材目录一起走 |
| `categories.toml` | 是           | 分类注册表（含空分类）     |
| `tags.toml`       | 是           | 标签注册表（含空标签）     |
| `metadata/`       | 是           | 素材参数，唯一数据源       |
| `trash/`      | 否           | 回收站，仅本机可恢复       |
| 库外缩略图/调色板缓存 | 否        | 系统缓存目录，可重建，不进同步盘 |

`.hawk/.gitignore` 由 hawk 自动生成，排除 `trash/`。

注意：部分网盘客户端（OneDrive、Syncthing 等）不识别 `.gitignore`，需要用户在网盘客户端中手动配置排除规则。后续应在用户文档中说明。

## 纯文本元数据

所有素材参数（标签、评分、备注等）集中存放在 `.hawk/metadata/` 中，按内容哈希命名，每个素材对应一个独立的纯文本文件（TOML）。

格式（初级版本，后续可能调整）：

```toml
# .hawk/metadata/<hash>.toml

# 文件位置：相同内容的文件共享一个 item，可有多条
[[paths]]
path = "posters/2024/sunset-photo.jpg"
size = 245760
modification_time = 1700000000000

url = "https://example.com/photo.jpg"   # 来源网址
tags = ["nature", "sunset"]             # 标签
categories = ["海报", "灵感参考"]      # 分类（虚拟分类维度，扁平可多选，见 category.md）
star = 4                                # 评分 0–5
annotation = "Beautiful sunset"         # 备注
```

尺寸、文件大小、扩展名等派生信息不写入元数据，索引时从文件读取。元数据写入采用「临时文件 + rename」的原子写，避免网盘同步走写了一半的文件。

只识别 `<hash>.toml` 命名的文件；网盘同步冲突产生的副本（如 `<hash>.sync-conflict-20250101.toml`）直接忽略，不参与索引。

## 回收站

通过 API 删除的素材或文件夹会被移入 `.hawk/trash/`（保留原有目录结构），而非直接删除。「是否在回收站」不是独立属性，由文件位置派生：位于 `.hawk/trash/` 内即在回收站。回收站是本地目录，不参与同步——恢复操作只能在执行删除的机器上完成。

移入 trash 的素材，其元数据（`metadata/<hash>.toml`）保留不删，`paths` 仍记录原来的库内路径，作为恢复时的放回目标；清空回收站时清理对应的元数据和缩略图——但仅限该内容在库内已无其他位置引用的情况（同一哈希的文件可能仍存在于其他目录）。

## 内存索引

索引完全在内存中维护，`.hawk/` 中不存在任何索引文件。启动流程：

1. 扫描素材目录得到全部文件清单（路径、大小、mtime）——只读目录，不读文件内容
2. 逐文件与 `metadata/` 比对：
   - 路径存在于某元数据的 `paths` 且 `size`、`modification_time` 一致 → 复用哈希（即元数据文件名），不重算
   - 路径不存在 → 新文件，计算哈希并入库
   - 大小或 mtime 变化 → 重算该文件哈希，按「路径 + 文件名」迁移元数据
   - 元数据中的路径已不存在 → 对应位置从索引移除
3. 哈希确认后将最新 `size`、`modification_time` 回写元数据，保持校验依据新鲜（否则下次启动会误判变动、重复哈希）
4. 运行期间由文件监听保持增量更新

元数据本身就是哈希缓存（文件名即哈希，`paths` 记录校验依据），平时启动无需读取文件内容。hawk 未运行期间对素材目录的改动会在下次启动时被上述比对自动发现；如对索引状态存疑，可手动调用 `POST /api/v1/library/reindex` 全量重建（见 API 文档）。

满足以下任一条件时，再引入磁盘索引（SQLite 等派生缓存），届时不改 API：

1. 文本搜索延迟无法接受，或需要相关性排序
2. 服务器版需要多用户集中存储

## 项目配置

每个项目的设置保存在 `.hawk/config.toml` 中，随素材目录一起同步、备份。库首次打开时若缺失会自动生成带注释的默认模板（已存在绝不覆盖，用户手工编辑安全）：

```toml
# .hawk/config.toml 示例

# 项目名（界面显示用）
name = "设计素材库"

# 索引时忽略的路径
ignore = ["node_modules", "*.tmp"]

# 生成的缩略图尺寸
thumbnail_sizes = [256, 512, 1024]

# 局域网 web 查看（只读；桌面端设置面板读写,按库隔离,多库可同时开启互不冲突）
[web]
enabled = false      # 开启后 server 追加监听 0.0.0.0:<port>,并托管前端页面
port = 27372
token = ""           # 查看者 token;浏览器打开 http://<电脑IP>:<port> 后输入,仅可浏览
```

`[web]` 的读取热更（文件监听 Reload），但**端口/绑定/token 的生效需重启监听**——保存后由桌面端重启 hawk-server。viewer token 通过时写端点一律 `403 READ_ONLY`（放行一切 GET 与 `item/list`、`item/skeleton` 两个查询类 POST）。

全局配置文件位于 `~/.config/hawk/config.toml`，只存放跨项目的全局设置（目前没有全局配置项）。

## 内容寻址（Content-Addressable）

每个文件通过其内容哈希（BLAKE3）唯一标识，缩略图与调色板缓存也按 hash 存储，避免重复生成：

```text
<库外缓存目录>/thumbnails/     # 见「目录结构」一节，%LOCALAPPDATA%/hawk/cache/<库标识>/ 等
├── 256/                    # 列表视图
│   └── abcdef123....webp
├── 512/                    # 列表视图（大图/高分屏）
│   └── abcdef123....webp
└── 1024/                   # 预览面板
    └── abcdef123....webp

<库外缓存目录>/colors/        # 调色板缓存（提炼算法见 docs/backend/color-search.md）
└── abcdef123....json   # { "v": 1, "palette": [{ "color", "percentage" }] }
```

缓存文件直接平铺在尺寸/类型目录下，不再按哈希前两位分桶：访问一律按内容哈希直接寻址、从不枚举目录，
现代文件系统（NTFS/APFS/ext4）承载单目录数万文件没有实际问题；省去分桶后路径计算与排障都更直白。

调色板缓存带算法版本号 `v`：提炼算法变更时版本 +1，旧缓存视为缺失自动重建。清空回收站时随缩略图一并清理。

## 实时文件监听

hawk 通过文件系统事件（FileSystemWatcher）实时感知变化，新增、删除、重命名、修改文件时，索引自动更新。`.hawk/` 目录自身不参与监听与索引。`config.toml` 与注册表文件（categories.toml / tags.toml）的变更同样被监听，修改后自动生效。

文件监听可能静默丢事件（尤其 macOS FSEvents，无溢出错误可捕获），因此另有**周期对账**：默认每 60 秒跑一次轻量全量扫描（复用哈希、不读文件内容），保证最终一致。间隔由环境变量 `HAWK_RESCAN_INTERVAL` 控制（秒，0 关闭）。
