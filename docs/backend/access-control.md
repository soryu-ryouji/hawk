# 权限与凭证

hawk 是纯局域网产品：所有访问都从局域网明文 HTTP 进入 daemon，权限由目标 daemon 统一执法。
没有 hawk-server、租户、设备签名、iroh 隧道或 TLS 证书。

## 信任边界

| 入口 | 凭证 | 角色上限 | 用途 |
| ---- | ---- | -------- | ---- |
| 本地环回（Electron / CLI） | `HAWK_ADMIN_TOKEN` | admin | 本机桌面 UI、管理设置 |
| 本地环回（浏览器插件） | 集成 Session（`app/admin-token` 发现） | importer | 插件读写当前库 |
| 局域网 Web 端口 | AccessKey 换 HttpOnly Session Cookie，或直接 Bearer | editor | 手机/平板浏览器 |
| 局域网 Peer 端口 | Bearer AccessKey | editor | hawk-app / iOS / Android |

`admin` 不能通过任何局域网凭证获得；局域网最高为 editor（可读、导入、编辑、删除，不能修改 daemon 配置或密钥）。

## 角色与能力

| Capability | 语义 |
| ---------- | ---- |
| Read | 浏览、查询、事件、文件、缩略图 |
| Import | 新增素材、创建导入目标目录 |
| Edit | 修改、替换、删除、恢复、组织素材 |
| Manage | 修改 daemon 服务与访问控制配置 |

| 角色 | 能力 |
| ---- | ---- |
| viewer | Read |
| importer | Read + Import |
| editor | Read + Import + Edit |
| admin | 全部 |

## AccessKey

- 由目标 daemon 签发，绑定当前素材库（`library_id`）。
- 明文只在创建/配对成功时返回一次；`keys.json` 只存 sha256 验证子与前缀，列表接口不回传明文。
- 每把密钥有独立角色（默认 viewer）；可在「已授权设备」中调整或删除，下一请求立即生效。
- 桌面配对由目标端用户现场确认；目标端为源端签发专用密钥。源端可同时签发反向密钥，一次审批双向连接。
- 手机端通过二维码（`access_key` + `lan_endpoints`）或手动输入「地址 + 密钥」连接。

## Web Session

局域网 Web 用 `POST /api/v1/app/session` 以 AccessKey 换 Session：

```http
Set-Cookie: hawk_session=<opaque>; Path=/; HttpOnly; SameSite=Strict; Max-Age=2592000
```

- 无 `Secure`（局域网明文）。
- 图片/SSE 等无法设置请求头的请求走 Cookie。
- Cookie 写请求必须同时带 `X-Hawk-Client: web` 与同源 `Origin`，防 CSRF；JS 发起的 API 请求仍用 Bearer，不受影响。

## 锁（文件夹/分类/标签）

锁不是传输加密，是 daemon 强制的可见性边界：

- 未解锁锁覆盖的条目在列表/骨架查询中被排除；内容直连（thumbnail/file/detail）返回 403 `LOCKED`。
- 密码以 Argon2id PHC 存储；解锁成功发放内存票据，请求经 `X-Hawk-Unlock` 或 `?unlock=` 附带。
- 连续 5 次失败冷却 60 秒。
- 文件系统层面无保护；拿到 `.hawk/locks.toml` 可离线爆破（慢哈希缓解）。

## 不做的事

- 不做传输加密/TLS/证书指纹；需要跨网段时由用户自行用 Tailscale 等工具组网。
- 不做多租户、账号体系、角色继承、文件夹级分享。
- 本机同权限进程可直接读写素材目录，daemon 的 token 只防浏览器跨源请求，不防本机进程。
