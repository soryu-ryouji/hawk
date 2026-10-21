# hawk-server 使用指南

> hawk 远程访问服务端的多租户自托管部署（仓库 `hawk-server/`）。本文讲怎么跑、怎么配、对外提供哪些接口、日常怎么运维。架构与安全模型见 [远程访问设计](remote-access.md)，字段级契约见 [远程访问协议](remote-protocol.md)，编译输出与反代部署见 [远程访问部署](remote-deployment.md)。

## 职责与边界

hawk-server 是设备之间的「信令 + 中继 + 网页版宿主」，三块能力：

| 能力 | 说明 |
| ---- | ---- |
| 控制面 | 设备注册、心跳与在线判定、池内设备列表、会话撮合（WSS 推送 + REST 兜底） |
| 中继 | 设备间直连失败时，按会话转发 QUIC 加密数据报（一帧 = 一个数据报，只搬运密文） |
| 网页版宿主 | 以「网页版 (hawk-server)」全局设备身份发起会话，向浏览器提供两级登录与查看 |
| 插件写入通道 | 持有带写权限 hawk-app key 的浏览器插件经查看面代理向目标设备导入素材 |

不承担的事：素材存储与索引、素材内容解析、账号体系、**素材库权限的最终裁决**（权限以目标 daemon 本地 key 映射为准，server 只做映射与撮合）。直连模式下素材不经过服务器，中继只见密文；**网页版固定走中继，素材明文经服务器中转**（自托管信任前提）。

多租户：一台 server 可服务多个池（租户），每池一把 service-key；池间设备/会话/key 注册表完全隔离（跨池一律 404）。

## 构建

```bash
cd hawk-server
cargo build --release     # 产物 target/release/hawk-server（单文件，不依赖外部服务）
cargo test                # 协议测试 + 网页版全链路互操作测试（测试依赖 hawk-daemon）
```

网页版与局域网 web 查看共用同一份前端产物（`hawk-app/web/dist`），由**目标设备的 daemon** 托管：浏览器的 `/app`、`/assets/*` 请求经服务端隧道转发过去，服务端不再需要单独部署一份前端。

部署时用 `tools/deploy-hawk-server.sh`：它执行同样的 `cargo build --release --locked` 并把二进制复制到指定路径（默认仓库根 `hawk-server.bin`，`--path` 可改）。脚本的 cargo 依赖默认走 rsproxy 国内镜像，`CARGO_MIRROR=` 回退官方源；本机手动 `cargo build` 使用你自己的 `~/.cargo/config.toml`。

## 启动

```bash
hawk-server --data-dir /var/lib/hawk-server            # 启动（无需任何密钥参数）
hawk-server tenant add --label home                    # 创建租户，打印 service-key（只显示一次）
```

本地跑起来看效果（不写 --data-dir 就落在 `~/.config/hawk-server`）：

```bash
cd hawk-server
cargo run --
cargo run -- tenant add --label dev
# 浏览器打开 http://127.0.0.1:9100 即可看到网页版登录页（进入查看器需有在线且开启远程访问的桌面端）
```

生产环境的编译输出、nginx 反代与 systemd 单元见 [远程访问部署](remote-deployment.md)。

## 配置

| 参数 | 环境变量 | 默认值 | 说明 |
| ---- | -------- | ------ | ---- |
| `--port` | `HAWK_SERVER_PORT` | `9100` | 监听端口，绑定 `0.0.0.0`。本进程只说明文 HTTP，TLS 由前置反代终结 |
| `--admin-port` | `HAWK_SERVER_ADMIN_PORT` | `9110` | 管理面监听端口（租户管理），**只绑 `127.0.0.1`**，不鉴权；不得经反代/防火墙暴露 |
| `--data-dir` | `HAWK_SERVER_DATA_DIR` | `~/.config/hawk-server` | 状态目录（全平台统一的家目录约定，与 hawk-app 的 `~/.config/hawk` 同族）；不存在则自动创建 |

- 日志走 stdout，级别由 `RUST_LOG` 控制（默认 `info`）；排障时 `RUST_LOG=debug`，启动成功会打印 `hawk-server listening`。
- 服务端无数据库：监听成功即服务可用，没有额外的依赖探活。
- 没有任何「主密钥」启动参数：准入凭证全部来自 `tenant` 子命令创建的租户（见下）。

## 租户管理（service-key）

service-key 是池门禁凭证：hawk-app 接入与查看者第一级验证都靠它。程序生成（`hs_` + 256-bit 随机），**明文只在创建时打印一次**，服务端只存 HMAC 哈希（`tenants.json`）。

两条管理路径写同一注册表（文件即事实源，即时互见，运行中的 server 无需重启）：

**CLI 子命令**（服务停了也能用）：

```bash
hawk-server tenant add --label home      # 创建租户并生成 service-key
hawk-server tenant list                  # 列出租户 id/标签/创建时间（不含密钥）
hawk-server tenant revoke t_xxxx         # 吊销：service-key 与池内 hawk-app key 映射立即失效
```

**管理面 REST**（运行时使用，仅环回监听 `127.0.0.1:9110`，不鉴权——环回访问权与本机 shell/数据目录同级信任）：

```bash
# 创建（明文 service-key 只在本次响应出现）
curl -X POST http://127.0.0.1:9110/api/v1/admin/tenants \
  -H 'Content-Type: application/json' -d '{"label":"home"}'
# 查询（密钥本体不可恢复，只回 id/标签/创建时间）
curl http://127.0.0.1:9110/api/v1/admin/tenants
# 吊销
curl -X DELETE http://127.0.0.1:9110/api/v1/admin/tenants/t_xxxx
```

管理面端口**不得经反代或防火墙暴露**；远程管理用 SSH 端口转发：`ssh -L 9110:127.0.0.1:9110 <服务器>`。

吊销后该池设备记录保留但无法再通过鉴权（重新创建租户拿到新 key 让设备换配置接入即可）。

## 凭据与鉴权

两级凭证 + 一种派生令牌，别混用：

| 凭据 | 用途 | 携带方式 |
| ---- | ---- | -------- |
| 服务密钥 `service-key` | 控制面全部 REST、WSS 控制通道、中继 WS、查看者第一级验证 | REST/WS 用 `Authorization: Bearer <key>`（**不走 URL 查询串**）；gate 放在请求体 |
| 访问密钥 `hawk-app key` | 查看者第二级验证（网页/插件登录）；逐把读/写权限，由目标 daemon 签发与吊销 | 登录请求体 |
| 浏览器令牌 `bt` | 网页版/插件查看面（登录后的客户端） | `Authorization: Bearer <bt>` 或 `?token=`（SPA 的 img/SSE 场景）；静态子资源（`/assets/*`、`/icon.png`）用 `hawk_bt` cookie |

`bt`（形如 `hb_...`）由两级登录换发：`POST /viewer/gate`（service-key → 门令牌，5 分钟、5 次尝试预算）→ `POST /viewer/login`（门令牌 + hawk-app key → bt）。bt 绑定唯一目标设备（key 映射决定）与作用域（read/write，write 含读），服务端只存 sha256 哈希。写令牌开写会话需目标设备 `allow_write`；**最终权限由目标 daemon 按本地 key 映射裁决**（会话请求携带 key 验证子，server 声称的权限不被采信）。

`hawk_bt` cookie（`Path=/; HttpOnly; SameSite=Strict`，30 天）在页面 URL 带 `?token=` 时下发，供查看器的 `<script>`/`<img>` 等无法设请求头的子资源使用。`/api/*` 不认 cookie，只接受显式令牌，避免 cookie 自动携带带来的 CSRF 面；反代声明 `X-Forwarded-Proto: https` 时 cookie 才带 `Secure`。

**风控**：gate/login/控制面鉴权失败按来源 IP 计（10 分钟窗口 30 次 → 冷却 15 分钟）；login 另按租户计（20 次 → 整池冷却 15 分钟起指数增长，封顶 24 小时，一次成功登录清零）。冷却中返回 429，message 含剩余秒数。

错误信封不带业务错误码，语义由 HTTP 状态码承载：

```json
{ "status": "success", "data": {} }
{ "status": "error", "error": { "message": "服务密钥无效" } }
```

## 接口一览

控制面与查看面各有专用 handler，剩余未注册的 `/api/v1/*` 才进入网页版隧道代理——浏览器拿 `bt` 调 `/api/v1/devices` 会 401，设备列表要走 `/api/v1/viewer/devices`。

### 控制面（`service-key`）

| 方法 | 路径 | 说明 |
| ---- | ---- | ---- |
| POST | `/api/v1/devices` | 注册设备（入本池），返回 `device_id` |
| GET | `/api/v1/devices` | 本池设备列表（含在线状态、最近心跳、可达性），也可当健康检查 |
| PATCH | `/api/v1/devices/{device_id}` | 改名称 / `allow_remote` / 库名 / 证书指纹（更新指纹即轮换）；他池设备 404 |
| DELETE | `/api/v1/devices/{device_id}` | 注销设备：会话取消 + key 映射清除；他池设备 404 |
| PUT | `/api/v1/devices/{device_id}/keys` | 全量同步该设备的 hawk-app key 映射（daemon 推送；`{keys:[{s,read,write}]}`，`s`=sha256(key)） |
| POST | `/api/v1/devices/heartbeat` | 心跳 + 可达性上报；间隔 30s，最小 10s（更密返回 429），90s 未心跳判离线 |
| POST | `/api/v1/sessions` | 发起会话（`mode=direct` 或 `relay`；双方须同池） |
| GET | `/api/v1/sessions/{session_id}` | 查询会话状态（`pending` / `ready` / `rejected` / `unreachable` / `expired` / `cancelled`） |
| POST | `/api/v1/sessions/{session_id}/accept` | 目标设备接受会话 |
| POST | `/api/v1/sessions/{session_id}/reject` | 目标设备拒绝会话 |
| POST | `/api/v1/sessions/{session_id}/cancel` | 任一方取消会话 |
| GET | `/api/v1/ws?device_id=` | WSS 控制通道（Authorization 头鉴权），服务端推送 `hello` / `ping` / `session.*` |
| GET | `/api/v1/relay?device_id=&session_id=&role=` | WS 中继通道（Authorization 头鉴权；`role=a` 为素材库所在机，`role=b` 为发起方） |

请求/响应字段、会话状态机、中继配对规则见 [远程访问协议](remote-protocol.md)。

### 网页版查看面（两级登录 → `bt`）

| 方法 | 路径 | 说明 |
| ---- | ---- | ---- |
| POST | `/api/v1/viewer/gate` | `{ "service_key": "..." }` 换门令牌（免鉴权，IP 风控） |
| POST | `/api/v1/viewer/login` | `{ "gate_token": "...", "key": "..." }` 换发 `bt`（IP + 租户风控）；响应含 `token`、`access`（read/write）与绑定的 `device_id` |
| POST | `/api/v1/viewer/logout` | 注销当前 `bt` |
| GET | `/api/v1/viewer/devices` | bt 绑定设备的单元素列表（bt 即定位，无池枚举面） |
| POST | `/api/v1/viewer/session` | `{ "target_device_id": "...", "access": "read" \| "write" }` 打开（或复用）查看会话；target 须等于 bt 绑定设备，write 需写令牌且目标 `allow_write` |
| GET | `/api/v1/viewer/session` | 当前绑定的会话，无则 `data: null` |
| DELETE | `/api/v1/viewer/session` | 断开当前会话（幂等） |
| GET | `/` | 登录页（免鉴权，两步表单：service-key → 访问密钥） |
| 其他 | `/api/v1/*` | 经 `bt` 绑定的隧道代理，转发到目标设备 daemon（read 会话只读；write 会话仅开放导入/建文件夹） |
| GET | `/app`、`/assets/*` | 查看器 SPA，同样经隧道由目标设备 daemon 托管（需已建会话，否则重定向回 `/`） |

### 示例

```bash
BASE=https://hawk.example.com
SK=<service-key>
APP_KEY=<hawk-app key>

# 健康检查 / 本池设备列表
curl -s -H "Authorization: Bearer $SK" "$BASE/api/v1/devices"

# 模拟网页登录：两级验证换浏览器令牌
GT=$(curl -s -X POST "$BASE/api/v1/viewer/gate" -H 'Content-Type: application/json' \
  -d "{\"service_key\":\"$SK\"}" | jq -r .data.gate_token)
curl -s -X POST "$BASE/api/v1/viewer/login" -H 'Content-Type: application/json' \
  -d "{\"gate_token\":\"$GT\",\"key\":\"$APP_KEY\"}"

# 用浏览器令牌看绑定设备
curl -s -H "Authorization: Bearer <bt>" "$BASE/api/v1/viewer/devices"
```

## 客户端接入

桌面端 hawk → 设置 → 远程，填两项即可：

- 服务端地址：`https://hawk.example.com`
- 服务密钥（service-key）：`tenant add` 生成的那把

保存后本机 daemon 自动注册、每 30s 心跳、同步本机访问密钥映射；同池设备互见后可发起直连（UPnP/IPv6），直连失败按设置决定是否经确认走中继。

向网页版/插件分发访问入口：设置 → 远程 →「访问密钥」签发（默认程序生成 128 位随机串，可自定义；勾选读/写权限），把「服务器地址 + service-key + 访问密钥」三段发给查看者（签发界面有一键复制分享串）。吊销即点「吊销」。

浏览器插件远程保存另需两件事：

1. 插件设置里填写 hawk-server 域名 + service-key + 带写权限的访问密钥
2. 目标设备设置 → 远程 → 开启「允许广域网保存素材」

插件登录后自动绑定访问密钥对应的素材库（无需再选设备）；保存的图片经服务器代理落到目标 daemon。

## 网页版

1. 浏览器打开 `https://hawk.example.com` → 登录页
2. 输入 service-key →「下一步」（换门令牌）→ 输入访问密钥 → 换发 `bt`（存 localStorage，下次免登录；`?token=` 可携带已有令牌）
3. 列表即访问密钥绑定的素材库 →「浏览素材库」→ 跳转 `/app?token=<bt>`；服务端以「网页版 (hawk-server)」身份建 relay 会话，并终结 QUIC 隧道
4. 进入只读查看器（与局域网 web 查看同一套界面，页面与静态资源经隧道来自该设备的 daemon）。页面注销即让服务端删除该 `bt` 哈希

使用前提与代价：

- 目标设备必须是带前端产物的桌面端（查看器页面与 API 都来自它）
- 目标设备必须在线且 `allow_remote` 开启，否则打开会话会失败
- **网页版固定走中继**，素材明文经过服务器；浏览素材消耗的是服务器带宽

## 数据目录

| 文件 | 内容 | 丢失后的影响 |
| ---- | ---- | ------------ |
| `devices.json` | 设备注册表（含 `tenant_id` 池归属，原子写） | 客户端心跳收到 404 后自动重新注册（换新 `device_id`） |
| `tenants.json` | 租户注册表（service-key 的 HMAC 哈希 + 标签） | 全部客户端/查看者无法接入（无恢复手段，需重建租户并换 key） |
| `keys.json` | hawk-app key 映射（HMAC 哈希 + 验证子 + 权限 + 池/设备归属） | 查看者无法登录；各设备下次接入时自动重新同步恢复 |
| `pepper` | 全部 HMAC 的胡椒（0600） | 所有已存哈希不可验证——**与注册表一起备份、一起恢复**；单独丢失等同 tenants.json 丢失 |
| `identity.json` | 网页版设备身份：`device_id` + QUIC 自签证书/私钥 + 指纹（Unix 下 0600） | 启动时重新生成；旧网页会话失效，浏览器 `bt` 不受影响 |
| `viewer_tokens.json` | 浏览器令牌 sha256 哈希与授权（作用域 + 绑定设备 + 凭证，原子写，0600） | 全部浏览器/插件需重新登录 |

- 整个目录即全部状态，备份/迁移直接拷目录（`pepper` 必须一起，否则注册表哈希作废）；恢复后保持文件权限
- 会话与在线状态在内存，服务端重启后设备在下一次心跳（≤30s）恢复在线

## 运维

### 健康检查

```bash
curl -s -o /dev/null -w '%{http_code}\n' -H "Authorization: Bearer $SK" \
  https://hawk.example.com/api/v1/devices      # 200 即 HTTP 与鉴权都正常
curl -s -o /dev/null -w '%{http_code}\n' https://hawk.example.com/   # 200 表示进程存活
```

### 设备管理

- `GET /api/v1/devices` 看 `online`、`last_seen_at_ms`、`allow_remote`、`allow_write`、`reachability`（本池视角）
- 「网页版 (hawk-server)」是全局设备（不属于任何池），不出现在设备列表中；它是服务端自身的网页版宿主身份，勿在 devices.json 里手工删除
- 临时关闭某台设备的远程访问：`PATCH` 该设备 `{"allow_remote": false}`，新会话直接 `rejected`
- 彻底移除设备：`DELETE /api/v1/devices/{device_id}`；设备侧心跳收到 404 会自动重新注册

### 撤销与更换

- 撤某个查看者的访问：让库主在 hawk 设置 → 远程中吊销对应访问密钥（server 映射与 daemon 本地映射同步删除，后续会话即被拒）
- 撤单个浏览器令牌：在网页版页面注销
- 全部浏览器令牌：停服务 → 删除 `viewer_tokens.json` → 启动
- service-key 泄露：`tenant revoke` 旧租户 → `tenant add` 新租户 → 所有设备/查看者更新配置（该池 hawk-app key 映射一并失效，设备接入后自动重新同步）

### 升级

- 用 `tools/deploy-hawk-server.sh` 重新构建并覆盖旧二进制，然后重启服务
- 状态只有几个 JSON + pepper 文件，无迁移步骤；升级前备份数据目录
- 协议路径为 `/api/v1`，不兼容变更升 v2（见协议文档「版本策略」）

## 故障排查

| 现象 | 原因与处理 |
| ---- | ---------- |
| 控制面返回 401 `服务密钥无效` | 客户端的 service-key 不存在或已吊销（`tenant list` 核对） |
| 登录页第二步 401 `访问密钥无效` | 访问密钥错误/已吊销，或不属于该 service-key 的池；让库主在设置 → 远程中核对 |
| gate/login 返回 429 | 验证失败过多进入冷却（message 含剩余秒数）；池冷却期间全部查看者都无法登录——排查是否有爆破行为 |
| 插件登录提示「该访问密钥没有写权限」 | 该 key 签发时未勾选写权限；在 hawk 设置 → 远程中为其开启「写」 |
| 查看面返回 401 `未登录` / `令牌无效或已注销` | `bt` 已注销或 `viewer_tokens.json` 被清空，重新登录 |
| 设备列表里 `online: false` | daemon 未运行、未启用远程访问、心跳已过期（90s），或 HTTPS 链路不通 |
| 网页版登录成功但打开查看器失败 | 目标设备离线、`allow_remote` 关闭，或反代拦截了中继 WebSocket；服务端日志有会话建立记录 |
| `/app` 返回 302 回到登录页 | 未建查看会话或 `bt` 已失效：重新登录 |
| `/app` 返回 502 | 目标设备 daemon 已离线或隧道断开 |
| 查看器白屏、`/assets/*` 返回 401 | 浏览器丢了 `hawk_bt` cookie（清过站点数据/隐私模式拦截）：回 `/` 重新登录 |
| 网页版列表/图片不更新、大文件超时 | 反代缓冲了 SSE 或超时过短：`proxy_buffering off`、加大 `proxy_read_timeout` |
| 插件保存返回 413 | 反代请求体上限过小（nginx 默认 1MB）：配置 `client_max_body_size 256m` |
| 插件保存返回 403 `目标设备未开启广域网写入` / `remote writer is limited...` | 目标设备未开启远程访问的 `writable`，或该 key 无写权限；按提示检查目标设备设置 |
| 心跳返回 429 | 心跳间隔小于 10s，检查客户端配置 |
| 服务端日志看会话细节 | `RUST_LOG=debug` 重启；systemd 部署用 `journalctl -u hawk-server -f` |

## 相关文档

- [远程访问设计](remote-access.md)：架构、连接模型、安全模型
- [远程访问协议](remote-protocol.md)：接口契约与状态机
- [远程访问部署](remote-deployment.md)：编译输出、nginx 反代、TLS、systemd
- [技术栈](../tech-stack.md)：hawk-server 的依赖选型
