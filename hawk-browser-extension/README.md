# hawk-browser-extension

hawk 浏览器图片收集插件：右键或拖拽网页图片，保存到本机 hawk 素材库。Chrome / Firefox / Safari 同一套代码，由 [WXT](https://wxt.dev) 做跨浏览器构建。

纯局域网版本只连接本机 hawk daemon；没有 hawk-server、远程登录或加密会话代理。

## 功能

- **右键保存**：右键网页图片 →「保存图片到 hawk」
- **拖拽保存（Eagle 式）**：按住图片拖过阈值后，面板在指针处展开；左侧大投放区松手即存根目录，右侧可投到文件夹或「＋ 新建文件夹」
- **零配置本地连接**：在 `127.0.0.1:27371–27400` 段内自动发现本机 daemon，token 自动获取
- **跟随本机 hawk**：本机在看自己的库就存本地；本机在浏览局域网设备时，插件仍只操作本机 daemon 当前库（局域网目标由桌面端切换）

## 工作原理

- 直连本机 hawk-daemon，`Authorization: Bearer <token>` 鉴权，接口见 [server-rest-api-v1.md](../docs/backend/server-rest-api-v1.md)
- **端口自动发现**：hawk app 的本地 API 端口默认 27371，被占用时在 `27371–27400` 段内换一个；插件按同一段顺序探活免鉴权的 `GET /api/v1/app/admin-token` 找到实际端口，结果缓存 60 秒，app 重启后 401 会立即重新探活
- 本机 daemon 的 token 每次启动随机生成；插件通过 `GET /api/v1/app/admin-token` 自动发现（仅环回地址可读），「高级选项」里的 Token 仅作手动覆盖
- 每次请求前查 `GET /api/v1/app/connection` 确认本机 daemon 有本地库；`none` 提示「本地 hawk 未连接任何素材库」
- 请求一律从 background 发起（`host_permissions` 覆盖本机地址与 http(s) 图片源，不受页面 CORS 限制）

来源网页随保存请求经 `website` 传入，入库为素材的 `url` 字段（与 Eagle 的 `website` 参数同义）。

## 设置

- 本地连接不需要任何配置（也不用填地址）——端口与 token 都自动发现
- 只有 hawk 在设置 → 网络 → 本地服务里把端口改到 `27371–27400` 之外时，才需在插件「高级选项」里填端口（Token 同样在其中，留空自动获取）

token 以明文存于 `browser.storage.local`；请只在可信浏览器配置文件中使用。

## 开发

```bash
npm install
npm run dev          # Chrome：自动打开浏览器并加载扩展，支持 HMR
npm run dev:firefox  # Firefox
```

首次 `npm install` 会经 `wxt prepare` 生成 `.wxt/`。

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

## 发布打包

```bash
npm run zip
npm run zip:firefox
npm run zip:safari
```

## 项目结构

```text
entrypoints/
  background.ts    # 后台：右键菜单 + 拖拽保存消息入口
  content.ts       # 拖拽保存：阈值触发 + 锚定指针的保存面板
  popup/           # 工具栏弹窗（本地设置、连接状态）
lib/
  api.ts           # 本机 daemon 客户端（Envelope 解包、token）
  local-discovery.ts # 端口自动发现（27371–27400 探活 /api/v1/app/admin-token）
  settings.ts      # 插件设置（browser.storage.local）
  notify.ts        # 系统通知
wxt.config.ts      # 跨浏览器清单
```

## 已知限制

- `blob:` 图片需 content script 协助抓取，暂未支持
- 本地模式自动发现的端口段是 `27371–27400`；用户手动把 hawk 端口改到段外时，需要在「高级选项」里填端口
- 批量收集、Alt 点选、整页/区域截图等后续按需添加
- 拖拽保存暂只处理 `<img>` 元素；CSS 背景图、`<picture>` 多候选等后续按需扩展
