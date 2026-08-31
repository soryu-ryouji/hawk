# hawk-browser-extension

hawk 浏览器图片收集插件：右键保存网页图片到 hawk 素材库。Chrome / Firefox / Safari 同一套代码，由 [WXT](https://wxt.dev) 做跨浏览器构建。

## 功能

- **右键保存**：右键网页图片 →「保存图片到 hawk」，服务端按 URL 下载入库；`data:` 图片自动转 base64 提交
- **拖拽保存（Eagle 式）**：按住图片向某个方向拖过阈值，面板浮在指针旁（留有间隙）；面板只有两块——**文件夹列表**（投到某行存入该文件夹，投左侧大区域存根目录）与**「＋ 新建文件夹」投放区**（把图片拖到上面 → 命名 → 创建文件夹并把图片存入；平时点击也可只建文件夹）；Esc 取消
- 工具栏弹窗：连接状态展示 + 服务器地址 / Token 设置

## 工作原理

- 直连本机 hawk-daemon（默认 `http://127.0.0.1:27371`），`Authorization: Bearer <token>` 鉴权，接口见 [server-rest-api-v1.md](../docs/backend/server-rest-api-v1.md)
- 请求一律从 background 发起（`host_permissions` 覆盖本机地址，不受页面 CORS 限制）
- 来源网页随保存请求经 `website` 传入，入库为素材的 `url` 字段（与 Eagle 的 `website` 参数同义；`url` 参数只是下载来源）

## 鉴权与零配置

hawk-daemon 的 token 每次启动随机生成，防护对象是浏览器里的恶意网页（CSRF 直写素材库）。同时提供免鉴权的发现端点 `GET /api/v1/app/token`：响应不携带 CORS 头（跨源网页 JS 读不到），且 Host 限定环回地址（防 DNS rebinding）——只有持 host_permissions 的扩展能读取。

**因此插件无需填写 Token**：hawk 桌面应用启动后插件即可直接使用（Token 缓存 60 秒，服务重启后自动重新发现，401 自动重试一次）。设置里的 Token 输入框仅作手动覆盖（如连接非本机服务），日常留空即可。

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
  background.ts    # 后台：右键菜单 + 拖拽保存消息入口，与 hawk-daemon 通信，通知反馈
  content.ts       # 拖拽保存：阈值触发 + 浮于指针旁的保存面板（文件夹列表 + 新建文件夹投放区，iframe 内图片同样支持）
  popup/           # 工具栏弹窗（设置界面）
lib/
  api.ts           # hawk-daemon REST 客户端（Envelope 解包 + Bearer 鉴权）
  settings.ts      # 插件设置（browser.storage.local）
  notify.ts        # 系统通知
public/icons/      # 扩展图标（由 hawk-app/build/icon.png 生成，PowerShell 一次性产出）
wxt.config.ts      # 跨浏览器清单（WXT 自动处理 Firefox/Safari 差异）
```

## 已知限制 / 后续规划

- `blob:` 图片需 content script 协助抓取，暂未支持
- 批量收集、Alt 点选、整页/区域截图等（参考 eagle.crx 的 batch-saver / element-inspector / screen-capturer）后续按需添加
- 拖拽保存暂只处理 `<img>` 元素；CSS 背景图、<picture> 多候选等场景后续按需扩展
