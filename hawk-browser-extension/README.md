# hawk-browser-extension

hawk 浏览器图片收集插件：右键或拖拽网页图片，保存到 hawk 素材库。Chrome / Firefox / Safari 同一套代码，由 [WXT](https://wxt.dev) 做跨浏览器构建。

## 功能

- **右键保存**：右键网页图片 →「保存图片到 hawk」
- **拖拽保存（Eagle 式）**：按住图片拖过阈值后，面板在指针处展开；左侧大投放区松手即存根目录，右侧可投到文件夹或「＋ 新建文件夹」
- **两种连接方式**：
  - 本地：直连本机 hawk-daemon（默认 `http://127.0.0.1:27371`），**跟随本地 hawk 当前浏览的库**——本机在看自己的库就存本地，正在浏览远端库就用它的会话代理存进那个远端库
  - 远程：连接自托管 hawk-server 域名（service-key + 访问密钥），不依赖本机 app 是否打开
- **目标素材库选择**：远程模式下只列出在线、开启远程访问且开启广域网写入的设备；选择持久化，拖拽面板显示当前目标

## 工作原理

### 本地模式

- 直连本机 hawk-daemon，`Authorization: Bearer <token>` 鉴权，接口见 [server-rest-api-v1.md](../docs/backend/server-rest-api-v1.md)
- 本机 daemon 的 token 每次启动随机生成；插件通过 `GET /api/v1/app/token` 自动发现（该端点仅环回地址可读），设置里的 Token 仅作手动覆盖
- 目标解析：每次保存前查 `GET /api/v1/app/connection`——`local` 存本地库；`remote` 表示本机 hawk 正在浏览远端库，按返回的会话代理地址 + 令牌直发（与 app 渲染进程同一条隧道）；`none` 提示「本地 hawk 未连接任何素材库」
- 远端库能否写入取决于目标设备的「允许广域网保存素材」（本机以同租户设备身份拿到写会话）；只读时提示保存失败原因，不静默丢图
- 请求一律从 background 发起（`host_permissions` 覆盖本机地址与任意 http(s) 图片源，不受页面 CORS 限制）

### 远程模式

1. 设置里填 hawk-server 域名、服务密钥（service-key，server 侧 `tenant add` 生成）与访问密钥（hawk 设置 → 远程中签发，须带写权限）
2. 插件两级登录：`POST /api/v1/viewer/gate`（service-key 换门令牌）→ `POST /api/v1/viewer/login`（门令牌 + 访问密钥换写作用域令牌 bt；bt 绑定唯一目标素材库，服务端只存哈希）
3. 素材库即访问密钥绑定的那一台（无需选择）；保存前用 `POST /api/v1/viewer/session` 打开 `access=write` 会话
4. 之后的库信息、文件夹树、保存请求经 hawk-server 查看面代理转发到目标 daemon
5. 目标设备必须同时开启「允许广域网保存素材」（应用级 `remote.toml` 的 `writable`）；广域网写只开放导入素材与新建文件夹，不开放删除、改标签等操作；权限最终由目标 daemon 按本地 key 映射裁决

来源网页随保存请求经 `website` 传入，入库为素材的 `url` 字段（与 Eagle 的 `website` 参数同义）。

## 设置

- 连接方式：本地 hawk / 远程 hawk-server
- 本地：服务器地址、Token（可选，留空自动发现）；**不需要任何 service-key**，本地库或本机正在浏览的远端库都可存
- 远程：hawk-server 域名（建议 HTTPS）、服务密钥（service-key）、访问密钥（须带写权限）；验证后自动绑定访问密钥对应的素材库

远程密钥与本地 daemon token 都以明文存于 `browser.storage.local`；请只在可信浏览器配置文件中使用。访问密钥泄露时让库主在 hawk 设置 → 远程中吊销该密钥即可（不影响其他密钥）。

## 开发

```bash
npm install
npm run dev          # Chrome：自动打开浏览器并加载扩展，支持 HMR
npm run dev:firefox  # Firefox
```

首次 `npm install` 会经 `wxt prepare` 生成 `.wxt/`（类型与自动导入配置，tsconfig 引用它）。

## 构建与本地加载

```bash
npm run build          # Chrome（MV3）→ .output/chrome-mv3
npm run build:firefox  # Firefox（MV2）→ .output/firefox-mv2
npm run build:safari   # Safari（MV2）→ .output/safari-mv2
```

- **Chrome**：`chrome://extensions` → 开发者模式 →「加载已解压的扩展程序」→ 选择 `.output/chrome-mv3`
- **Firefox**：`about:debugging` →「此 Firefox」→「临时加载附加组件」→ 选择 `.output/firefox-mv2/manifest.json`

## Safari

Safari 扩展必须在 macOS 上经 Xcode 转换后才能安装分发：

```bash
npm run build:safari
xcrun safari-web-extension-converter .output/safari-mv2
```

随后按普通 macOS App 签名、构建、分发（.app/.dmg）。Windows 上只能产出转换前的扩展目录。

## 发布打包

```bash
npm run zip          # Chrome 商店 / 自分发 zip
npm run zip:firefox
npm run zip:safari
```

## 项目结构

```text
entrypoints/
  background.ts    # 后台：右键菜单 + 拖拽保存消息入口，保存/文件夹/通知，按素材库隔离最近使用
  content.ts       # 拖拽保存：阈值触发 + 锚定指针的保存面板（文件夹树 + 新建文件夹）
  popup/           # 工具栏弹窗（本地/远程设置、素材库选择、连接状态）
lib/
  api.ts           # 本地 daemon / hawk-server 远程客户端（Envelope 解包、令牌、会话）
  settings.ts      # 插件设置（browser.storage.local）
  notify.ts        # 系统通知
public/icons/      # 扩展图标（由 hawk-app/build/icon.png 生成）
wxt.config.ts      # 跨浏览器清单（WXT 自动处理 Firefox/Safari 差异）
```

## 已知限制 / 后续规划

- `blob:` 图片需 content script 协助抓取，暂未支持
- 远程模式一次只绑定一个目标素材库（hawk-app key 决定）；本地模式跟随本机 hawk，无需配置目标
- 本地模式依赖 daemon 监听固定端口 `27371`（被其他进程占用时 app 回退随机端口，插件需在设置里改地址）
- 目标设备切换「允许广域网保存素材」后，经下一次心跳（≤30s）同步到服务端；刷新列表若未出现，稍后再试
- 广域网保存是浏览器下载后 base64 上传，大图会占用上行带宽；反代需放开请求体上限（见 [远程访问部署](../docs/backend/remote-deployment.md)）
- 批量收集、Alt 点选、整页/区域截图等后续按需添加
- 拖拽保存暂只处理 `<img>` 元素；CSS 背景图、`<picture>` 多候选等场景后续按需扩展
