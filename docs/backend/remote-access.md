# 远程访问设计

> 远程访问是可选能力：每台设备两个进程——hawk-app（Electron 壳）与 hawk-daemon（素材库服务），广域网连接能力全部收在 daemon 的 **`remote/` 模块**（可选子系统，supervisor 管理）。远程访问服务端 **hawk-server** 位于本仓库 `hawk-server/`，多租户方案（池 = 租户），已实现控制面、中继、网页版与受限广域网写。接口契约见 [remote-protocol.md](remote-protocol.md)。

## 目标与非目标

**目标**：在客户端配置接入信息（服务端地址 + service-key）后，查看自己其他在线设备，远程浏览对方设备上的素材库；持有带写权限 hawk-app key 的浏览器插件可向开启了广域网写入的设备导入素材。外部查看者（网页版/插件）需两级凭证：service-key（池门禁）+ hawk-app key（逐把读/写权限）。

同网段的 hawk-app 之间优先走独立的局域网直连链路，不需要配置 hawk-server：见 [局域网 hawk-app 互联](lan-interconnect.md)。两条链路共享素材 HTTP 契约，但发现、凭证与写权限开关彼此独立。

**非目标**（v1 不做）：

- 广域网完整库管理（删除、改标签/评分、管理设置等）——v1 只开放导入所需的 `item/add`（base64）/`item/upload`（multipart）与 `folder/create`
- 账号 / 会员体系——服务端为多租户自托管方案，准入即租户 service-key（后续可接账号系统，届时 service-key 下沉为内部凭证，协议不动）
- 共享库给他人——设备只属于同一套接入凭据
- 移动端查看器——后续迭代，协议不为此预留特殊设计

## 总体架构

两台设备各自的 hawk-daemon 内嵌 remote 模块，彼此之间经 iroh 建立端到端加密隧道，素材数据始终由「素材库所在机」的 daemon 产出。

```text
        设备 A（素材库所在机）                      设备 B（查看方）
┌────────────────────────────┐          ┌────────────────────────────┐
│ hawk-app (Electron)        │          │ hawk-app (Electron)        │
│  前端 ──HTTP/SSE──► daemon  │          │  前端 ──HTTP──► 会话代理     │
│  主进程：拉起/回收 daemon    │          │  主进程：拉起/回收 daemon    │
└──────────┬─────────────────┘          └──────────┬─────────────────┘
           ▼                                       ▼
┌────────────────────────────┐          ┌────────────────────────────┐
│ hawk-daemon                │          │ hawk-daemon                │
│  API 127.0.0.1:p1          │          │  API 127.0.0.1:p1          │
│  ┌──────────────────────┐  │          │  ┌──────────────────────┐  │
│  │ remote/ 模块          │  │          │  │ remote/ 模块          │  │
│  │ 信令（注册/心跳/WSS）  │  │          │  │ 信令                  │  │
│  │ A 侧隧道端 ◄───────────┼──┼─iroh 隧道（端到端加密）──┤ B 侧会话代理 :p4 │  │
│  │ （进程内转发 viewer 级）│  │          │  │ 127.0.0.1，随机 token │  │
│  └──────────────────────┘  │          │  └──────────────────────┘  │
└───────────────┬────────────┘          └───────────────┬────────────┘
                │  HTTPS/WSS 控制面（注册/心跳/会话/WSS）  │
                └───────────────┬───────────────────────┘
                                ▼
              hawk-server（本仓库 hawk-server/，控制面/中继/查看面已实现；
              见 remote-protocol.md）
```

端口默认值：hawk-server 9100 / 内嵌中继 9102（可选组件）/ daemon API 27371（用户可配，自动选择限 27371–27400；严格绑定，被占用由 app 询问换端口）/ LAN 网站 TCP 27372 / hawk-app 直连 TCP 27375 / 远程隧道 UDP 27373 / 局域网发现 UDP 27374（仅 LAN 查看启用时监听）。会话代理端口由客户端在会话建立时分配空闲端口（不固定绑定）。

## 组件职责

### hawk-daemon（含 remote/ 模块）

素材库服务本体 + `remote/` 模块承载全部广域网能力：

- 信令客户端：设备注册、周期心跳（30s，携带当前库名与可达性）、WSS 控制通道（自动重连）、设备列表、会话发起与响应
- 数据面：iroh endpoint（打洞/中继/路径迁移/映射保活全部由 iroh 自治）——A 侧 accept 循环按会话放行集合执法（连接建立后身份不符即断开），隧道内 HTTP 请求**在进程内转发**给自身路由，携带 viewer 级或受限远程写权限标记（见安全模型）；B 侧会话代理：环回监听器（supervisor 管理，同 LAN web 模型），每次会话随机 token，web 查看器以 `?token=` 携带，响应不带 CORS 头；连接死亡在原会话内重拨热替换（前端无感）
**可选子系统约束**：应用级 `remote.toml` 的 `enabled = true` 且 url/key 非空才激活，supervisor reconcile 模型（同 LAN web）——接入参数变化热重启信令，`writable` 变更热生效不重启；绑定/连接失败只写状态不崩进程。未启用时任务不 spawn、端口不绑定，该能力在运行时不存在。热重启时旧数据面（QUIC 端点 + A 侧 accept 循环）随任务一起关停，supervisor 等其结束再拉起新任务，并对端口绑定做短暂重试——避免新一轮绑定自己刚占用的端口；停用后端口立即释放。

**应用级配置与设备身份**：接入配置存 `~/.config/hawk/remote.toml`（Electron 经 `--remote-config` 传入），设备身份存同级的 `remote/identity.json`。两者不再随素材库切换：同一台机器在服务端始终是一台设备，切换库后心跳把设备表的 `library_name` 收敛到当前库。以 `local` 子命令独立运行 daemon 时可不传 `--remote-config`，回退到库内 `.hawk/remote.toml` 与 `.hawk/remote/`（测试/单库脚本用）。手工修改 `remote.toml` 需重启 daemon；设置面板经 API 写入即时生效。

```toml
# ~/.config/hawk/remote.toml（应用级；设备身份与访问密钥在 ~/.config/hawk/remote/）
enabled = true
url = "https://hawk.example.com"
service_key = "hs_<service-key>"
quic_port = 27373
writable = false
relay = false
```

访问密钥（hawk-app key）存同级 `remote/keys.json`（sha256 验证子 + 标签 + 权限，0600；明文不落盘）。签发/改权限/吊销走 daemon API（`remote/keys` 端点），变更与每次接入时全量推送 server 映射。

### hawk-app（Electron 壳，接线枢纽）

主进程职责不变（拉起/回收 daemon、注入 admin token），远程相关只做两件事：

- 维护「当前活动连接」（见连接模型），向渲染进程下发连接变更；远程库的 `api_base` 指向 daemon 的会话代理端口
- 无本地素材库时提供「仅浏览远程素材库」入口：拉起无库形态的 daemon（远程查看模式，不创建任何本地目录），前端进远程查看屏
- 订阅 daemon SSE 的 remote 会话生命周期事件，驱动断线回切

不再拉起第三进程，无端口预选下发、无受托 token 注入、无额外状态 API 通道。

### hawk-server（本仓库 `hawk-server/`，多租户）

远程访问服务端，与 hawk-daemon **零代码共享**（独立 crate，协议契约是唯一共享物）：

- 多租户准入：`tenant add/list/revoke` CLI 管理 service-key（程序生成 256-bit 随机、明文只打印一次、服务端只存 HMAC 哈希）；池间设备/会话/key 注册表完全隔离；无账号、会员、计费、额度概念；凭证验证面带 IP + 租户双维度风控（详见协议「凭证模型」）
- 控制面（已实现）：设备注册/心跳/列表/更新/注销、WSS 控制通道、会话编排（直连候选组装、打洞编排、状态机）
- 内嵌中继（可选组件）：iroh-relay 库形态（不另起进程），打洞协调 + 数据兜底；共享 token 准入（数据目录生成，经控制面下发），`--relay-url` 未配置即不提供
- 中继（已实现，可选组件）：内嵌 iroh-relay（库形态），打洞协调 + 数据兜底；token 准入经控制面下发；端到端语义不变（只见密文）
- **网页版宿主（已实现）**：服务端作为 B 侧（以「网页版」全局设备身份走既有协议发起会话、经长驻 iroh endpoint 拨号并终结隧道），任意浏览器经域名浏览素材库。浏览器令牌（bt）由两级登录换发（service-key → 门令牌 → hawk-app key → bt；bt 绑定唯一目标设备与 read/write 作用域，服务端只存哈希），查看面 API（`/api/v1/viewer/*`）+ 隧道代理 + 登录页；查看器前端产物不落服务端，`/app` 与 `/assets/*` 同样经隧道由目标设备 daemon 托管。部署见 [远程访问部署](remote-deployment.md)
- 持久化：设备注册表/网页版身份/令牌 JSON 文件；部署：监听明文 HTTP（默认 9100），TLS/WSS 由前置反代终结

## 数据流：daemon 为何不需要理解「远程」语义

remote 模块是「网络延长线」，只做字节转发，不产生、不转换任何素材数据。隧道内 HTTP 请求与局域网浏览器请求走同一条路由管线：

```text
B 浏览器 / B app 查看窗口
  │ ① 普通 HTTP 请求
  ▼
B 侧会话代理（remote 模块）──② 原样塞进 QUIC 隧道──►
A 侧隧道端（remote 模块）──③ 进程内转发，按会话标记 viewer 级/受限写权限──►
A 的 API 路由 ──④ 与局域网请求无区别地处理──► 响应原路返回
```

- B 侧的素材库服务与远程查看完全无关（照常服务 B 自己的库）；跨公网传输的请求/响应，收件方是浏览器
- app 前端查看远端库不需要新数据通路：app 的窗口与 web 查看器是同一个构建产物，只是 API base 指向会话代理端口

### 低带宽内容策略

- A 侧鉴权中间件为隧道请求写入受托标记，`app/info.remote=true`；桌面 app 与 hawk-server 网页版都据此识别广域网，无需根据 URL 猜测。本地与局域网为 false，预览行为不变
- 网格统一请求 512 WebP；广域网预览及相邻预载使用 1024 WebP，只有当前图进入放大状态才请求 `/item/file` 原图。保存、复制、编辑仍明确读取原图
- 缩略图未命中返回 404 并由后台生成，完成经 SSE `item.updated` 重试，不把原图作为缩略图回退
- daemon 按 `Accept-Encoding` 对 JSON/文本响应协商 gzip；图片与 SSE 不压缩。B 侧代理和 hawk-server 查看面只转发字节与响应头，不重新编码

## 连接模型：切换库无感

设计目标：用户在「我的库」与「远端设备库」之间切换时，UI 结构与连接方式对前端无感——前端始终只面对「一个 API base + 一个 token」。

**连接描述符**（主进程维护，preload 暴露 `getCurrent()` + `onChanged()`）：

```text
Connection {
  api_base: "http://127.0.0.1:<port>"   // 端口语义前端不可见
  token:    "..."
  label:    "我的库" | "家里台式机"        // 仅用于展示
}
```

- 本地库：`api_base` 指向 daemon API 端口；远程库：指向 B 侧会话代理端口
- 前端收到 `connection-changed` 后执行**统一的切换重置**：abort 进行中的请求与 SSE → 清空内存 store → `GET /app/info` → 按返回重新注水。该流程是启动注水的泛化，只写一次
- **断线分级处理**：路径级抖动（NAT 重绑定、网络切换）由 iroh 路径迁移自治（不断连）；连接真正死亡时 B 侧原会话内重拨并热替换（前端无感）；重拨连续失败/对端离线才回切——主进程经 daemon SSE 的 remote 事件获知，自动把当前连接切回本地库再通知前端。用户看到的是无声回到自己的库，而非远程白屏报错
- **无感的边界**：写能力降级与来源标识保留。桌面端远程浏览先争取写会话：目标开启「允许广域网保存素材」即得 `remote_writer`（**仅导入素材与新建文件夹**，其余写端点 403），否则自动降级只读；前端按 `app/info.access` 区分三态（`viewer` 只读 / `remote_writer` 仅导入 / admin 可读写），侧栏左上角库名按钮改为「库名 [远程]」（悬停提示为设备名与写能力；侧栏隐藏时为顶栏左组紧凑徽标；显式断开入口在库名下拉顶部）——这是安全与认知底线，不是可省略的区分逻辑。浏览器插件本地模式跟随当前连接（`GET /api/v1/app/connection` 拿会话代理地址与令牌），远端库可写时拖拽直接入库，与桌面端同一条会话通道

## 关键设计决策

**1. remote 是 daemon 内模块，不是独立进程**

原方案（独立进程 hawk-remote）的两个动机在当前前提下已失效或可用更省的方式满足：闭源云端边界（社区构建物理剔除云端代码）随服务端开源自托管而消失；崩溃域隔离在 Rust 里可用 supervisor 任务级重启达到同等实效——daemon 已有先例：LAN web 就是 reconcile 模型的可选网络子系统，故障只写状态不崩进程。合并后省掉三块接线：第三进程的 spawn/传参/回收、受托 token 双进程注入机制、独立状态 API 端口（设备列表/会话事件走 daemon 现有 REST + SSE）。代价是 iroh 等依赖进入 daemon 依赖树（未启用时运行成本为零），以及 WAN 代码与素材库服务同进程（以 supervisor + 应用级远程配置可关闭缓解）。

**2. 直连优先，服务器只做撮合**

素材浏览是重带宽场景，自托管服务器的带宽/流量有限，数据面默认不经过服务器：服务端只做注册、在线与寻址撮合（控制面），素材数据走设备间直连（iroh 打洞自治）。中继是服务端可选组件（内嵌 iroh-relay），消耗的是用户自己的服务器带宽。

**3. 传输层用 iroh（QUIC/noq + magicsock），不自研穿透**

远程传输的全部难点——打洞、中继、路径迁移、NAT 映射保活、多路径竞速——由 iroh 的 magicsock 内建承担（多年打磨 + NAT 类型矩阵测试）。hawk 的控制面只做撮合与凭证（endpoint_id + 直连地址 + 可选中继的下发），传输语义不进协议。身份即 iroh EndpointId（Ed25519 公钥，握手按公钥认证），替代原自签证书指纹钉扎。中继为服务端可选组件（内嵌 iroh-relay，token 准入随心跳下发），无中继部署退化为纯直连（打洞协调依赖中继，文档写明）。
**4. 协议契约先行，零代码共享**

客户端（daemon remote 模块）与 hawk-server 之间的契约是 remote-protocol.md。hawk-server 保持独立 crate、不建 workspace 羁绊、不共享代码——它对 hawk 的全部知识就是协议文档；remote 模块对 daemon 本体的全部知识就是进程内路由 + viewer 权限标记。

**5. 客户端最小知识：URL + KEY，无登录**

桌面端配置只有服务端地址 URL 与 service-key（应用级 `remote.toml`，跨素材库共享）。控制面无登录流程、无会话令牌；key 无效即 401，客户端直接展示服务端返回的 message。查看者凭证（hawk-app key）由目标设备 daemon 本地签发与保管（明文只在签发时出现一次，本地只存 sha256 验证子），server 经设备推送只存 HMAC 哈希映射；会话请求携带验证子，权限由目标 daemon 按本地映射执法（server 被攻破也无法放大权限）。

**6. 会话认证在 iroh 身份层，隧道内 HTTP 进程内授权**

设备身份即 iroh EndpointId（Ed25519 公钥），注册时登记，握手按公钥认证（无单独钉扎环节）。隧道内 HTTP 由 A 侧隧道端在进程内标记权限：只读会话为 viewer 级；写会话需查看者 key 带写权限 + 目标设备 `remote.writable=true`，且仅开放导入素材/新建文件夹。查看方 B 全程不接触 daemon 的任何 token，服务端也接触不到 daemon admin token。

### 广域网写权限分层

| 层 | 控制项 | 作用 |
| -- | ------ | ---- |
| hawk-server | hawk-app key 映射（HMAC 哈希） | 只有映射中带写权限的 key 能换发写作用域 bt；key 由目标设备 daemon 推送，吊销即失效 |
| 目标设备 | `remote.writable` | 允许广域网写会话导入素材/新建文件夹（查看者 key 会话与同租户设备会话同受此开关约束）；默认关闭，每请求校验，关闭立即失效 |
| 目标 daemon | 本地 key 映射执法 + `AccessLevel::RemoteWriter` 端点白名单 | 查看者会话按 key 验证子裁决；app 间会话（同租户设备）按本机 `remote.writable` 裁决（server 声称的 access 不采信）。只放行 GET、查询类 POST、`item/add`（禁止 `path`）、`item/upload`（可附带分类/标签）与 `folder/create`；其余写端点 403 |

局域网写权限 `[web]` 与以上链路完全独立：开启局域网写不会放开广域网写，反之亦然。

**7. 中继由两个静态开关承载，无逐次确认**

「服务端是否提供（内嵌 relay 组件）× 客户端是否使用（设备级 `relay` 开关，默认关闭）」。传输层（iroh）按可用性自治：中继在场时首字节不等打洞（relay 先行、打洞成功自动升级）；任一端关闭即纯直连。自建带宽保护靠开关而不是弹窗——中继只在开关双方共识时消耗。

## 远程查看模式（没有本地素材库）

daemon 是双形态进程：`local <素材库路径>` 启动本地库模式，`remote --remote-config <路径>` 启动无本地库的远程查看模式。后者不构建任何库子系统（索引/监听/缩略图/LAN），库端点一律 400 `NO_LIBRARY`，仅保留远程管理面（`/remote/*`）与会话代理；接入配置与设备身份在应用级 remote.toml/remote/（无库无库内回退位，`--remote-config` 由 clap 强制要求）。无库设备注册时 `allow_remote=false`、库名恒为「远程查看」（可被池内发现，但没有素材可服务）。

首次启动或本地库失效时，引导页提供「仅浏览远程素材库」：主进程直接拉起无库 daemon（不创建任何本地目录），前端经 `app/info.has_library=false` 进远程查看屏（复用设置面板的远程分区：填服务端/密钥 → 选设备 → 连接）。冷启动模式记忆为 config.toml 的 `remoteViewer`（开本地库即复位）；真实本地库从远程查看屏「打开本地素材库」或侧栏库下拉切换。

远程会话建立成功即把目标设备 id 记入 config.toml 的 `remoteTarget`（显式断开或切本地库时清除）：冷启动 daemon 就绪后、通知渲染进程之前先恢复该会话（信令上线等最多 5s → 目标在线且开放访问才发起 → 建连失败回退），成功则第一个连接描述符就是远端代理，远程查看屏与空页不会出现；失败/目标离线则照常进远程查看屏，记录保留供下次启动再试。

```text
无 libraryPath → SetupScreen
  ├─ 打开资源文件夹 → 系统目录选择框 → 本地库
  └─ 仅浏览远程素材库 → 拉起无库 daemon（不建目录）→ 远程查看屏
       → 填服务端/密钥 → 选设备 → 连接（连接描述符切换，boot 进主界面）
```

## 连接建立流程

### 建连流程

```text
B 配置接入（应用级 remote.toml url + key）→ 拉取设备列表（A 在线 + 心跳携带直连地址）
B 前端发起会话 → 服务端 WSS 推送给 A：session.request（含 B 的 endpoint_id 与凭证）
A 侧自动接受（allow_remote 开关 + 查看者 key 本地执法）→ 回 accept
服务端 ready：connect = A 的 endpoint_id + 直连地址（+ 服务端提供中继时的 relay url/token）
B 拨号（iroh connect）：中继在场先经中继建连（首字节零等待），后台打洞成功自动升级直连；
  打洞不成则留在中继；无中继部署只有直连地址可达才建得起来（否则 unreachable）
隧道就绪 → 主进程切换连接描述符 → B 前端无感重置为远端库
```

### 中继（服务端可选组件）

hawk-server 内嵌 iroh-relay（库形态，不另起进程），准入为共享 token（数据目录生成，随心跳/会话经控制面下发）。中继对协议无独立端点——`connect.relay` 字段即全部表达；客户端与 relay 之间是 iroh 内部事务。中继承载端到端加密的 QUIC 数据报，服务端只见密文。`--relay-url` 未配置即不提供中继（纯直连部署：无打洞协调，只有直连地址可达的会话能建立）。

## 安全模型

| 环节     | 措施 |
| -------- | ---- |
| 控制面   | HTTPS/WSS（TLS 由前置反代终结）；service-key 程序生成 256-bit 随机（`tenant add`），服务端只存 HMAC-SHA256(pepper, key)，恒定时间比较；key 一律走 Authorization 头，不进 URL（防反代日志泄露） |
| 凭证风控 | gate/login/控制面鉴权失败按来源 IP 计（10min/30 次 → 冷却 15min）；login 另按租户计（20 次 → 池冷却 15min 起指数增长封顶 24h，成功清零）；门令牌 5min TTL + 5 次尝试预算 |
| 数据面   | iroh（QUIC/noq + TLS 1.3）端到端加密，endpoint_id 即公钥经控制面交换、握手认证；服务端接触不到素材内容（直连不过服务端，中继只见密文） |
| 隧道端点 | 隧道内请求在进程内转发时标记 viewer 级或受限远程写权限；写会话必须携带与对端身份（endpoint_id）匹配的会话标识，且目标 `remote.writable=true`（查看者会话另需 key 带写权限）。`GET /api/v1/app/token`（免鉴权返回主 token，Host 限定环回）在隧道端直接 404 |
| 本地防护 | B 侧会话代理：每次会话随机 token、响应无 CORS、环回监听 |
| 凭据存储 | 接入配置（url + service_key）存应用级 `~/.config/hawk/remote.toml`（与壳偏好 `config.toml` 分开，daemon 权威写入；明文，权限 0600）；设备身份（device_id、自签证书私钥）存同级 `remote/identity.json`、访问密钥验证子存同级 `remote/keys.json`（均 0600）。浏览器插件的 service-key 与访问密钥存 `browser.storage.local`，与本地 daemon token 同级 |
| 网页版   | **隧道在服务器终结**：素材明文过服务器（自托管信任前提，与「中继只见密文」刻意不同的取舍）。浏览器令牌 bt 随机换发、服务端存 sha256 哈希与授权（作用域 + 绑定设备 + 凭证）、可随时注销；service-key 与 hawk-app key 都只出现在登录请求体（服务端只见 service-key 与 key 的哈希输入，key 明文以哈希落盘验证后即弃） |
| 广域网写 | 带写权限的 hawk-app key + 目标设备开关双重准入 + daemon 本地执法；远程写只覆盖导入与建文件夹，禁止 `path` 导入（防读本机文件）；目标关闭开关或吊销 key 后，现有会话的下一个写请求即 403 |

## 分发形态

- hawk-server 位于本仓库 `hawk-server/`，随 AGPL 开源，自托管部署（单二进制 + CLI/env 配置，TLS 交反代）
- daemon 单二进制内含 remote 模块，无第三进程、无构建期门控；远程配置未启用则运行时该能力不存在（任务不 spawn、端口不绑定）
- app 远程 UI 以 daemon `/api/v1/remote/*` 状态为准：未配置时展示接入引导，无独立构建开关

## 对 hawk 仓库的改动清单

| 位置                       | 改动 |
| -------------------------- | ---- |
| `hawk-server/`             | 远程访问服务端（多租户：租户注册表 + key 映射 + 门令牌两步登录 + 风控；控制面 + 中继 + 查看面） |
| `hawk-daemon/src/remote/`  | 模块：信令客户端 / iroh 数据面 / B 侧会话代理 / 访问密钥保管（keys.rs），supervisor reconcile 管理 |
| `hawk-daemon` config       | 应用级 `remote.toml`（enabled/url/service_key/writable），保存即热生效（写开关不重启信令）；设备身份与 keys.json 同级 `remote/` |
| `hawk-daemon` 鉴权中间件   | 进程内 viewer / remote_writer 权限标记（供隧道转发请求使用），无 remote 语义、无门控 |
| `hawk-daemon` API          | `/api/v1/remote/*` 状态端点 + 访问密钥管理端点 + SSE remote 会话生命周期事件 |
| `hawk-app`（主进程 + 前端） | 远程 UI（接入配置/访问密钥管理/设备列表/会话流程）；连接描述符与 `connection-changed`；订阅 daemon SSE remote 事件驱动断线回切 |
| `~/.config/hawk/remote/`   | 应用级设备身份目录（device_id、自签证书私钥）与访问密钥验证子（keys.json），不参与素材库同步 |

## 落地记录

| 阶段 | 内容 | 验收标准 |
| ---- | ---- | -------- |
| 1-3 | 控制面 + 数据面 + 中继 + 打洞（自研穿透栈，已完成） | 历史记录 |
| 4 | ~~传输层迁移 iroh~~（已完成：magicsock 承担打洞/中继/迁移/保活；身份换 EndpointId；中继内嵌 hawk-server 可选组件；控制面只剩撮合与凭证） | 全部测试套件在 iroh 基座上全绿；真实双 NAT 手动验证 |

## remote/ 模块结构

```text
hawk-daemon/src/remote/
├── mod.rs          # supervisor（reconcile：remote.toml 变更 → 热重启信令）+ 状态快照 + 数据面槽位发布
├── config.rs       # 应用级 remote.toml 读写（设备身份目录由配置位置派生）
├── identity.rs     # remote/identity.json（device_id、iroh 密钥对与 endpoint_id）
├── keys.rs         # remote/keys.json：访问密钥（hawk-app key）签发/保管/吊销 + server 映射推送
├── signal.rs       # REST 客户端（注册/心跳/设备/会话/key 同步）+ WSS 通道 + 自动重连 + 查看者会话本地执法 + relay 下发驱动的数据面重建
├── data.rs         # iroh endpoint、A 侧隧道端（accept 执法 + hyper-over-stream 进程内转发 TunneledPeer）、B 侧会话代理（连接可热替换）
└── session.rs      # B 侧会话编排：发起 → 轮询 ready → iroh 拨号 → 代理；断线重拨兜底；SSE remote.session
```

新增依赖：tokio-tungstenite（WSS）、reqwest（REST，rustls）、iroh（传输层：打洞/中继/迁移/保活）、hyper + hyper-util（隧道内 HTTP/1.1 解析）。

数据面传输模型：一条 iroh 双向流即一条 HTTP/1.1 连接——B 侧会话代理逐请求开流原样转发，A 侧经 hyper 解析后进程内调 API 路由。SSE/缩略图等流式响应由 HTTP/1.1 语义原生承载，两端都不解析素材数据。

## 相关文档

- [远程访问协议](remote-protocol.md)（接口契约，唯一共享物）
- [hawk-server 使用指南](hawk-server.md)（启动配置、接口、运维排查）
- [远程访问部署](remote-deployment.md)（编译、反代、TLS）
- [hawk-app 设计](../frontend/hawk-app.md)（进程模型、打包分发）
