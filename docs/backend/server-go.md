# hawk-daemon（Go 实现）

> 逐文件的代码职责见 [代码导读](server-code-structure.md)。
> API 契约见 [REST API V1](server-rest-api-v1.md)，存储格式见 [storage.md](storage.md)。

hawk 素材管理后端，Go 实现（`hawk-daemon/`）。纯局域网：只提供明文 HTTP，不做 TLS/QUIC 加密。
行为对齐基准 = `openapi.json` + `.hawk/` 存储格式 + SSE 事件契约。

## 状态

- **app 唯一后端**：`hawk-app` 的开发态（`resolveServerCommand`）、打包（`scripts/build-server.mjs`）、
  CI（`ci.yml`）全部使用本实现
- **已实现**：全部 REST API、SSE、文件监听、扫描、缩略图（WebP q80）、调色板（median-cut）、
  按内容 hash 的重复位置分组、SQLite 元数据（schema v3）、局域网 Web/Peer 监听、UDP 发现与桌面配对
- **已删除**：Rust 版、hawk-server、hawk-transport、iroh 隧道、TLS 证书、广域网远程访问

## 技术选型

| 职责 | 选型 | 备注 |
| ---- | ---- | ---- |
| HTTP | `net/http` | Go 1.22+ 路由模式，标准库中间件 |
| SQLite | `modernc.org/sqlite` | 纯 Go，无 cgo，单文件 `.hawk/metadata.db` |
| 文件监听 | `fsnotify` | 递归目录注册 + 400ms 防抖 + rename 配对 |
| 哈希 | `zeebo/blake3` | item id = BLAKE3 hex（存储契约） |
| 图像解码/缩放 | stdlib + `golang.org/x/image` + `disintegration/imaging` | Lanczos |
| WebP 编码 | `gen2brain/webp` | cgo-free，q80 |
| TOML | `pelletier/go-toml/v2` | 配置与小型注册表 |
| 锁密码 | `golang.org/x/crypto/argon2` | Argon2id PHC，兼容旧 Rust 数据 |

## 构建与运行

```bash
cd hawk-daemon
go build -o hawk-daemon.exe .     # 产物 hawk-daemon(.exe)
go test ./...                     # 单元/行为/API 测试
```

运行协议：

```bash
HAWK_ADMIN_TOKEN=<token> hawk-daemon local <素材库路径> \
  --port 27371 \
  --web-dist <前端 dist> \
  --identity-dir <访问密钥目录> \
  [--cache-parent <缓存父目录>]

hawk-daemon openapi              # 打印 openapi.json
```

`--port` 严格绑定（被占用即以退出码 4 + `PORT_IN_USE <port>` 退出，不自动换端口）；`--port 0`
由系统分配空闲端口（仅供测试/脚本）。

环境变量：`HAWK_ADMIN_TOKEN`、`HAWK_PORT`、`HAWK_WEB_DIST`、`HAWK_CACHE_PARENT`、`HAWK_IDENTITY_DIR`、
`HAWK_MAINTENANCE_INTERVAL`（秒，默认 60）。

## 测试

```bash
go test ./... -count=1
bash tools/smoke.sh               # 端到端 HTTP 冒烟（需先构建二进制；Linux CI 环境）
```

`tools/bench-*.py` 为性能压测脚本（启动/入库/图像管线/大库全链路），输出带 git SHA 的 `RESULT` 行。

## 在 app 中使用

Electron 主进程的开发态直接运行 `hawk-daemon/hawk-daemon(.exe)`（`HAWK_DAEMON_EXE` 环境变量可覆盖）。
打包与 CI 经 `scripts/build-server.mjs`（`go build`，按 RID 设置 `GOOS/GOARCH`，`CGO_ENABLED=0`）。

## OpenAPI

`/openapi/v1.json` 返回 `go:embed` 的 `internal/api/openapi.json`。契约单一来源是
`hawk-daemon/openapi.json`；修改后执行：

```bash
node tools/sync-openapi.mjs
cd hawk-daemon && go test ./...
cd hawk-app && npm run gen:types
```
