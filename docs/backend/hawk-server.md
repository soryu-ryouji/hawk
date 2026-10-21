# hawk-server 使用指南

> hawk 远程访问服务端的单用户自托管部署（仓库 `hawk-server/`）。本文讲怎么跑、怎么配、对外提供哪些接口、日常怎么运维。架构与安全模型见 [远程访问设计](remote-access.md)，字段级契约见 [远程访问协议](remote-protocol.md)，编译输出与反代部署见 [远程访问部署](remote-deployment.md)。

## 职责与边界

hawk-server 是设备之间的「信令 + 中继 + 网页版宿主」，三块能力：

| 能力 | 说明 |
| ---- | ---- |
| 控制面 | 设备注册、心跳与在线判定、设备列表、会话撮合（WSS 推送 + REST 兜底） |
| 中继 | 设备间直连失败时，按会话转发 QUIC 加密数据报（一帧 = 一个数据报，只搬运密文） |
| 网页版宿主 | 以「网页版 (hawk-server)」设备身份发起会话，向浏览器提供登录、设备选择、只读查看 |
| 插件写入通道 | 持有写入密钥的浏览器插件经查看面代理向目标设备导入素材（目标设备需开启广域网写入） |

不承担的事：素材存储与索引、素材内容解析、账号体系。直连模式下素材不经过服务器，中继只见密文；**网页版固定走中继，素材明文经服务器中转**（自托管单用户信任前提）。服务端单用户、无配额与计费；控制面用访问密钥，广域网写用可选的独立写入密钥。

## 构建

```bash
cd hawk-server
cargo build --release     # 产物 target/release/hawk-server（单文件，不依赖外部服务）
cargo test                # 协议测试 + 网页版全链路互操作测试（测试依赖 hawk-daemon）
```

网页版与局域网 web 查看共用同一份前端产物（`hawk-app/web/dist`），由**目标设备的 daemon** 托管：浏览器的 `/app`、`/assets/*` 请求经服务端隧道转发过去，服务端不再需要单独部署一份前端。

部署时用 `tools/deploy-hawk-server.sh`：它执行同样的 `cargo build --release --locked` 并把二进制复制到指定路径（默认仓库根 `hawk-server.bin`，`--path` 可改）。脚本的 cargo 依赖默认走 rsproxy 国内镜像，`CARGO_MIRROR=` 回退官方源；本机手动 `cargo build` 使用你自己的 `~/.cargo/config.toml`。

## 启动

最小启动（密钥至少 16 个字符，启动时校验）：

```bash
openssl rand -hex 32           # 生成访问密钥
openssl rand -hex 32           # 可选：生成广域网写入密钥
hawk-server --access-key <访问密钥> --write-key <写入密钥> --data-dir /var/lib/hawk-server
```

本地跑起来看效果：

```bash
cd hawk-server
cargo run -- \
  --access-key dev-key-0123456789 \
  --write-key dev-write-key-0123456789 \
  --data-dir ./hawk-server-data
# 浏览器打开 http://127.0.0.1:27370 即可看到网页版登录页（进入查看器需有在线且开启远程访问的桌面端）
```

生产环境的编译输出、nginx 反代与 systemd 单元见 [远程访问部署](remote-deployment.md)。

## 配置

| 参数 | 环境变量 | 默认值 | 说明 |
| ---- | -------- | ------ | ---- |
| `--port` | `HAWK_SERVER_PORT` | `27370` | 监听端口，绑定 `0.0.0.0`。本进程只说明文 HTTP，TLS 由前置反代终结 |
| `--access-key` | `HAWK_SERVER_ACCESS_KEY` | 必填 | 控制面与只读网页登录的凭据。少于 16 字符直接退出（退出码 2） |
| `--write-key` | `HAWK_SERVER_WRITE_KEY` | 空 | 可选：广域网写入密钥，仅用于换发写作用域浏览器令牌。不配置则广域网写关闭；少于 16 字符或与访问密钥相同直接退出 |
| `--data-dir` | `HAWK_SERVER_DATA_DIR` | `hawk-server-data` | 状态目录，相对路径按当前工作目录解析；不存在则自动创建 |

- 日志走 stdout，级别由 `RUST_LOG` 控制（默认 `info`）；排障时 `RUST_LOG=debug`，启动成功会打印 `hawk-server listening`。
- 密钥写在命令行会出现在 `ps` 和 systemd 单元里，生产环境建议走环境变量（systemd `Environment=`）。
- 服务端无数据库：监听成功即服务可用，没有额外的依赖探活。

## 凭据与鉴权

服务端有两类密钥与一种派生令牌，别混用：

| 凭据 | 用途 | 携带方式 |
| ---- | ---- | -------- |
| 访问密钥 `access_key` | 控制面全部 REST、WSS 控制通道、中继 WS、网页只读登录 | REST 用 `Authorization: Bearer <key>`；WSS/中继用 `?key=`；登录放在请求体 |
| 写入密钥 `write_key` | 网页/插件登录换发写作用域 `bt`；不参与控制面 | 登录请求体 |
| 浏览器令牌 `bt` | 网页版/插件查看面（登录后的客户端） | `Authorization: Bearer <bt>` 或 `?token=`（SPA 的 img/SSE 场景）；静态子资源（`/assets/*`、`/icon.png`）用 `hawk_bt` cookie |

`bt` 由网页登录换发（形如 `hb_...`），服务端只存 sha256 哈希与作用域（read/write），可随时注销。`access_key` 只换发 read；`write_key` 换发 write。写令牌只能开写会话（且目标设备必须 `allow_write`），不能访问控制面设备管理。**换密钥不会让已换发的 `bt` 失效**，需要一并清理见「运维」。

`hawk_bt` cookie（`Path=/; HttpOnly; SameSite=Strict`，30 天）在页面 URL 带 `?token=` 时下发，供查看器的 `<script>`/`<img>` 等无法设请求头的子资源使用。`/api/*` 不认 cookie，只接受显式令牌，避免 cookie 自动携带带来的 CSRF 面；反代声明 `X-Forwarded-Proto: https` 时 cookie 才带 `Secure`。

错误信封不带业务错误码，语义由 HTTP 状态码承载：

```json
{ "status": "success", "data": {} }
{ "status": "error", "error": { "message": "密钥无效" } }
```

## 接口一览

控制面与查看面各有专用 handler，剩余未注册的 `/api/v1/*` 才进入网页版隧道代理——浏览器拿 `bt` 调 `/api/v1/devices` 会 401，设备列表要走 `/api/v1/viewer/devices`。

### 控制面（`access_key`）

| 方法 | 路径 | 说明 |
| ---- | ---- | ---- |
| POST | `/api/v1/devices` | 注册设备，返回 `device_id` |
| GET | `/api/v1/devices` | 设备列表（含在线状态、最近心跳、可达性），也可当健康检查 |
| PATCH | `/api/v1/devices/{device_id}` | 改名称 / `allow_remote` / 库名 / 证书指纹（更新指纹即轮换） |
| DELETE | `/api/v1/devices/{device_id}` | 注销设备，其进行中的会话一并取消 |
| POST | `/api/v1/devices/heartbeat` | 心跳 + 可达性上报；间隔 30s，最小 10s（更密返回 429），90s 未心跳判离线 |
| POST | `/api/v1/sessions` | 发起会话（`mode=direct` 或 `relay`） |
| GET | `/api/v1/sessions/{session_id}` | 查询会话状态（`pending` / `ready` / `rejected` / `unreachable` / `expired` / `cancelled`） |
| POST | `/api/v1/sessions/{session_id}/accept` | 目标设备接受会话 |
| POST | `/api/v1/sessions/{session_id}/reject` | 目标设备拒绝会话 |
| POST | `/api/v1/sessions/{session_id}/cancel` | 任一方取消会话 |
| GET | `/api/v1/ws?key=&device_id=` | WSS 控制通道，服务端推送 `hello` / `ping` / `session.*` |
| GET | `/api/v1/relay?key=&device_id=&session_id=&role=` | WS 中继通道（`role=a` 为素材库所在机，`role=b` 为发起方） |

请求/响应字段、会话状态机、中继配对规则见 [远程访问协议](remote-protocol.md)。

### 网页版查看面（`bt`）

| 方法 | 路径 | 说明 |
| ---- | ---- | ---- |
| POST | `/api/v1/viewer/login` | `{ "key": "..." }` 换发 `bt`（免鉴权）；响应含 `token` 与 `access`（read/write） |
| POST | `/api/v1/viewer/logout` | 注销当前 `bt` |
| GET | `/api/v1/viewer/devices` | 设备列表（结构同控制面，含 `allow_write`，供插件筛选可写素材库） |
| POST | `/api/v1/viewer/session` | `{ "target_device_id": "...", "access": "read" \| "write" }` 打开（或复用）查看会话；write 需写令牌且目标 `allow_write` |
| GET | `/api/v1/viewer/session` | 当前绑定的会话，无则 `data: null` |
| DELETE | `/api/v1/viewer/session` | 断开当前会话（幂等） |
| GET | `/` | 设备选择器页（免鉴权，登录 + 选设备） |
| 其他 | `/api/v1/*` | 经 `bt` 绑定的隧道代理，转发到目标设备 daemon（read 会话只读；write 会话仅开放导入/建文件夹） |
| GET | `/app`、`/assets/*` | 查看器 SPA，同样经隧道由目标设备 daemon 托管（需已建会话，否则重定向回 `/`） |

### 示例

```bash
BASE=https://hawk.example.com
KEY=<访问密钥>
WRITE_KEY=<写入密钥>

# 健康检查 / 设备列表
curl -s -H "Authorization: Bearer $KEY" "$BASE/api/v1/devices"

# 模拟网页登录：access_key 换只读浏览器令牌
curl -s -X POST "$BASE/api/v1/viewer/login" \
  -H 'Content-Type: application/json' \
  -d "{\"key\":\"$KEY\"}"

# 模拟插件登录：写入密钥换写作用域令牌
curl -s -X POST "$BASE/api/v1/viewer/login" \
  -H 'Content-Type: application/json' \
  -d "{\"key\":\"$WRITE_KEY\"}"

# 用浏览器令牌看设备列表
curl -s -H "Authorization: Bearer <bt>" "$BASE/api/v1/viewer/devices"
```

## 客户端接入

桌面端 hawk → 设置 → 远程，填两项即可：

- 服务端地址：`https://hawk.example.com`
- 访问密钥：上面的 `access_key`

保存后本机 daemon 自动注册、每 30s 心跳、拉取设备列表；设备之间可见后可发起直连（UPnP/IPv6），直连失败按设置决定是否经确认走中继。

浏览器插件远程保存另需两件事：

1. 服务端配置 `--write-key`，插件设置里填写该密钥
2. 目标设备设置 → 远程 → 开启「允许广域网保存素材」

插件登录后只列出在线、开启远程访问且开启广域网写入的设备；选定素材库后保存的图片经服务器代理落到目标 daemon。

## 网页版

1. 浏览器打开 `https://hawk.example.com` → 设备选择器页
2. 输入访问密钥 → 换发 `bt`（存 localStorage，下次免登录；`?token=` 可携带已有令牌）
3. 选一台在线且开启远程访问的设备 →「浏览素材库」→ 跳转 `/app?token=<bt>`；服务端以「网页版 (hawk-server)」身份建 relay 会话，并终结 QUIC 隧道
4. 进入只读查看器（与局域网 web 查看同一套界面，页面与静态资源经隧道来自该设备的 daemon）。页面注销即让服务端删除该 `bt` 哈希

使用前提与代价：

- 目标设备必须是带前端产物的桌面端（查看器页面与 API 都来自它）
- 目标设备必须在线且 `allow_remote` 开启，否则打开会话会失败
- **网页版固定走中继**，素材明文经过服务器；浏览素材消耗的是服务器带宽

## 数据目录

| 文件 | 内容 | 丢失后的影响 |
| ---- | ---- | ------------ |
| `devices.json` | 设备注册表（`{"devices":[...]}`，原子写） | 客户端心跳收到 404 后自动重新注册（换新 `device_id`） |
| `identity.json` | 网页版设备身份：`device_id` + QUIC 自签证书/私钥 + 指纹（Unix 下 0600） | 启动时重新生成；旧网页会话失效，浏览器 `bt` 不受影响 |
| `viewer_tokens.json` | 浏览器令牌 sha256 哈希与 read/write 作用域（原子写） | 全部浏览器/插件需重新登录 |

- 整个目录即全部状态，备份/迁移直接拷目录；恢复后保持 `identity.json` 权限
- 会话与在线状态在内存，服务端重启后设备在下一次心跳（≤30s）恢复在线

## 运维

### 健康检查

```bash
curl -s -o /dev/null -w '%{http_code}\n' -H "Authorization: Bearer $KEY" \
  https://hawk.example.com/api/v1/devices      # 200 即 HTTP 与鉴权都正常
curl -s -o /dev/null -w '%{http_code}\n' https://hawk.example.com/   # 200 表示进程存活
```

### 设备管理

- `GET /api/v1/devices` 看 `online`、`last_seen_at_ms`、`allow_remote`、`allow_write`、`reachability`
- 列表里的「网页版 (hawk-server)」（`platform=server`）是服务端自身的网页版宿主身份，**不要删除**：删掉后网页版打开会话会报「来源设备不存在」，重启服务端才会重建（网页版选择器页会自动过滤掉这一条）
- 临时关闭某台设备的远程访问：`PATCH` 该设备 `{"allow_remote": false}`，新会话直接 `rejected`
- 彻底移除设备：`DELETE /api/v1/devices/{device_id}`；设备侧心跳收到 404 会自动重新注册

### 撤销浏览器令牌

- 单个浏览器：在网页版页面注销
- 全部撤销：停服务 → 删除 `viewer_tokens.json` → 启动（所有浏览器重新登录）

### 更换访问密钥 / 写入密钥

1. 改 `--access-key` / `--write-key` 并重启
2. 所有客户端/插件更新配置（旧密钥一律 401）
3. 停服务后删除 `viewer_tokens.json` 再启动（旧 `bt` 不随密钥失效；只换写入密钥时可只清理旧的写令牌，当前实现统一清空）

### 升级

- 用 `tools/deploy-hawk-server.sh` 重新构建并覆盖旧二进制，然后重启服务
- 状态只有三个 JSON，无迁移步骤；升级前备份数据目录
- 协议路径为 `/api/v1`，不兼容变更升 v2（见协议文档「版本策略」）

## 故障排查

| 现象 | 原因与处理 |
| ---- | ---------- |
| 启动即退出，提示 `access key 过短：至少 16 个字符` / `write key 过短` | 密钥不足 16 字符，重新生成后再启动 |
| 插件登录提示「该密钥只有浏览权限」 | 插件里填的是访问密钥；改为服务端 `--write-key` 对应的写入密钥 |
| 控制面返回 401 `访问密钥无效` | 客户端/登录页的密钥与服务端配置不一致 |
| 查看面返回 401 `未登录` / `令牌无效或已注销` | `bt` 已注销或 `viewer_tokens.json` 被清空，重新登录 |
| 设备列表里 `online: false` | daemon 未运行、未启用远程访问、心跳已过期（90s），或 HTTPS 链路不通 |
| 网页版登录成功但打开查看器失败 | 目标设备离线、`allow_remote` 关闭，或反代拦截了中继 WebSocket；服务端日志有会话建立记录 |
| `/app` 返回 302 回到登录页 | 未建查看会话（`bt` 未绑设备）或 `bt` 已失效：在选择器页重新登录并选设备 |
| `/app` 返回 502 | 目标设备 daemon 已离线或隧道断开，回选择器页重新选设备 |
| 查看器白屏、`/assets/*` 返回 401 | 浏览器丢了 `hawk_bt` cookie（清过站点数据/隐私模式拦截）：回 `/` 重新选设备 |
| 网页版列表/图片不更新、大文件超时 | 反代缓冲了 SSE 或超时过短：`proxy_buffering off`、加大 `proxy_read_timeout` |
| 插件保存返回 413 | 反代请求体上限过小（nginx 默认 1MB）：配置 `client_max_body_size 256m` |
| 插件保存返回 403 `目标设备未开启广域网写入` / `remote writer is limited...` | 目标设备未开启远程访问的 `writable`，或该会话不是写会话；按提示检查目标设备设置与服务端写入密钥 |
| 心跳返回 429 | 心跳间隔小于 10s，检查客户端配置 |
| 服务端日志看会话细节 | `RUST_LOG=debug` 重启；systemd 部署用 `journalctl -u hawk-server -f` |

## 相关文档

- [远程访问设计](remote-access.md)：架构、连接模型、安全模型
- [远程访问协议](remote-protocol.md)：接口契约与状态机
- [远程访问部署](remote-deployment.md)：编译输出、nginx 反代、TLS、systemd
- [技术栈](../tech-stack.md)：hawk-server 的依赖选型
