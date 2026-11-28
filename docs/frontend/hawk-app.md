# hawk-app 设计

## 定位

Electron 壳 + Vue 3 前端。前端只吃 HTTP API，不依赖 Electron IPC 做业务数据；主进程只负责窗口、后端进程生命周期、连接描述符与系统能力。

纯局域网版本：

- 本地库：Electron 拉起本机 Go daemon（`127.0.0.1:<apiPort>`，admin token）。
- 局域网库：主进程把前端 API base 直接指向目标 daemon 的 Peer 端口（`http://<ip>:27375`），Bearer 目标 AccessKey。
- 没有远程会话代理、hawk-server、iroh 隧道或 TLS。

## 进程与连接模型

```text
Electron 主进程
  ├─ 生成随机 admin token
  ├─ spawn hawk-daemon local <lib> --port <apiPort> --web-dist <dist>
  │     --identity-dir ~/.config/hawk/remote --cache-parent <可选>
  ├─ 轮询 /api/v1/app/startup → ready 后通知渲染进程
  ├─ 本地库连接描述符：{ api_base: http://127.0.0.1:<port>, credential: admin_token, kind: local }
  └─ 局域网连接：{ api_base: http://<target>:<peer_port>, credential: <access_key>, kind: lan, peer_id }
```

渲染进程收到 `connection-changed` 后统一执行：abort 请求/SSE → 清 store → `GET /app/info` → 重注水。
当前连接只有 `local | lan` 两种 `kind`。

已配对局域网设备保存在 `~/.config/hawk/config.toml` 的 `lanPeers`（含 URL 与 token）；`lanTarget` 记录上次浏览目标，冷启动 daemon 就绪后尝试恢复。主进程每 10 秒对当前目标 `GET /app/info`；失败后保持目标并进入恢复等待界面（继续每 10 秒探测），恢复成功后自动重新注水，不静默回切本地库。只有显式“打开本地素材库”或忘记设备才清目标。同一目标换网段/换 IP 后，源端会优先用已保存 AccessKey 探测发现到的同一设备新地址，验证通过只更新 URL，不重新配对或要求目标端再次确认。

## 主进程模块

| 文件 | 职责 |
| ---- | ---- |
| `electron/src/main.ts` | 应用入口、单实例、窗口/托盘、启动本地库 |
| `electron/src/server.ts` | 解析 Go 二进制、严格端口、拉起/回收/重启 daemon、启动进度轮询、换库 |
| `electron/src/connections.ts` | 本地/局域网连接描述符、UDP 发现、配对、直连、健康检查、回连恢复 |
| `electron/src/ipc.ts` | 白名单 IPC：窗口、选库/历史、端口、缓存目录、LAN 管理、文件管理器/剪贴板 |
| `electron/src/app-config.ts` | `~/.config/hawk/config.toml`（库路径、历史、apiPort、缓存父目录、lanPeers/lanTarget） |
| `electron/src/resource-auth.ts` | 图片/SSE 等 `<img>` 直链的资源凭证 |
| `electron/src/paths.ts` | 应用配置/日志/身份目录路径 |
| `electron/src/updater.ts` | GitHub Releases 自动更新（Windows PowerShell / macOS sh / Linux AppImage 接力安装） |

IPC 契约单一定义在 `electron/src/ipc-contract.ts`，web 端 `shared/types.ts` re-export。

## 前端结构

```text
web/src/
├── app/           启动相位机、App 壳、标题栏、启动/引导/门页/断线恢复
├── domains/
│   ├── library/   网格、检查器、筛选、查询、SSE 应用
│   ├── taxonomy/  文件夹/分类/标签侧栏与库名下拉
│   ├── preview/   预览/缩放/编辑
│   ├── import/    拖拽/上传/重复策略
│   ├── lan/       局域网发现、配对、已配对设备
│   └── settings/  外观、隐藏项、网络、存储、更新
├── shared/        API client/endpoints/events、连接展示态、UI 组件
└── styles/
```

连接展示态在 `shared/connection.ts`（`useConnectionLabel`、`initConnectionLabel`、`ipcMessage`）。局域网发现/配对动作在 `domains/lan`，设置面板 UI 在 `domains/settings/components/SettingsLan*.vue`。

## 启动与换库

1. `main.ts` 读 `config.toml`；无有效库 → 引导页（`SetupScreen`）。
2. 有库 → `openLibraryAt` 生成 token，spawn daemon，页面立即显示启动屏。
3. 主进程 200ms 轮询 `/api/v1/app/startup`；starting 阶段推送进度，ready 推送连接，error 推送失败。
4. 渲染进程 `useBoot.runBoot`：分类先行 → `library.init` → 连接 SSE → ready。
5. 换库/改端口/改缓存目录 → `stopServerAndWait` → 重新 spawn → 同上。
6. 端口占用（daemon 退出码 4 / `PORT_IN_USE`）→ 启动屏提供自动换端口或手输端口。

## 局域网连接

- 发现：`GET /api/v1/app/lan/peers`（本机 daemon 发 UDP probe）。
- 配对：本机创建反向设备密钥 → `POST 目标 /app/lan/pair-requests` → 轮询结果 → 保存目标 profile → 直连。
- 手动：目标地址 + 预创建 AccessKey，读取目标 `connection-info` 与 `app/info` 后保存直连。
- 目标端确认：`LanPairRequestPrompt` 轮询本机 daemon 的待确认请求，调用 decision。
- 设备管理：`SettingsLanDevices` 列出已配对设备与目标端签发的密钥，可调权限/删除/生成移动端二维码。

## 应用自动更新

- 检查/下载/校验都在 Electron 主进程完成；stable 比 semver，nightly 比构建 sha。
- Windows：更新包（zip）校验通过后，应用在退出前启动随包分发的 `resources/update.ps1`。脚本等待旧进程与 daemon 退出，解包到 AppDir 同卷 staging 目录，改名替换应用目录并拉起新版本。脚本只依赖系统自带 PowerShell，不需要独立的更新 exe。
- Windows 替换失败时会恢复旧目录并重新启动旧版本；日志在 `%TEMP%\hawk-update\install.log`，同时弹窗提示。
- 更新脚本要求应用目录对当前用户可写；解压到 `Program Files` 等受保护目录时需要以管理员身份运行应用。
- macOS：detached sh 脚本替换 `.app`；Linux：AppImage 原地替换后 `relaunch`。

## 打包与开发

```bash
cd hawk-app
npm ci
npm run dev             # vite + electron + 本地 Go daemon
npm run gen:types       # 先构建 hawk-daemon/hawk-daemon(.exe)
npm run build
npm run pack            # 前端 + Go daemon + electron-builder
```

Go 二进制由 `scripts/build-server.mjs` 按 RID 交叉编译到 `resources/hawk-daemon/`；`pack` 时经 electron-builder `extraResources` 进入安装包。

## 契约与测试

- 类型从 `hawk-daemon/openapi.json` 生成（`npm run gen:types`）。
- 单测：`npm run test:unit`（Vitest）。
- 类型/构建：`npm run build`。
- 端到端：`node tools/ui-check.mjs`（vite + electron + daemon + CDP）。
- 移动端 Web 冒烟：`npm run test:mobile`。
