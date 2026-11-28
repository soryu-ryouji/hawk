# 技术栈

## 总览

```text
后端：Go
前端：Vue 3 + TypeScript (Vite)，只走 HTTP
桌面壳：Electron，sidecar 拉起后端
移动端：Swift + SwiftUI（iOS 26+）/ Kotlin + Jetpack Compose（Android 8+），原生，复用同一套 HTTP 契约
契约：OpenAPI
传输：局域网明文 HTTP（不做 TLS/QUIC 加密）
```

## 后端

Go 1.27，见 [hawk-daemon](backend/server-go.md)（`hawk-daemon/`）：

- HTTP：标准库 `net/http`（Go 1.22+ 路由模式）+ SSE
- SQLite：`modernc.org/sqlite`（纯 Go，无 cgo）
- 文件监听：`fsnotify`
- 内容哈希：`zeebo/blake3`（item id = BLAKE3 hex，存储契约）
- 图像：标准库 + `golang.org/x/image` + `disintegration/imaging`（Lanczos）+ `gen2brain/webp`（cgo-free WebP 编解码）
- 配置/注册表：`pelletier/go-toml/v2`
- 密码哈希：`golang.org/x/crypto/argon2`（锁）

并发模型：一把 `RWMutex` 保护内存索引；写入由 `writeMu` 串行；扫描、监听、缩略图在独立 goroutine 中做 IO/CPU 工作，结果回到锁内应用。没有单独的 actor 框架。

## 前端与桌面壳：hawk-app/

Electron 壳 + Vue 3 前端（Composition API + `<script setup>` + Pinia），设计见 [hawk-app 设计](frontend/hawk-app.md)。约束：

- 前端只通过 REST API 与后端通信，不依赖 Electron IPC
- Electron 主进程只负责：创建窗口、拉起/回收后端进程、注入 token、局域网连接描述符
- electron-builder 打包，`extraResources` 携带各平台 Go 后端二进制
- 配对二维码：`qrcode`（仅设置面板按需生成 PNG data URL）

## 移动端：hawk-ios/（M1 只读浏览已实现）

原生 iOS / iPadOS 客户端，设计见 [iOS 客户端设计](frontend/ios-app.md)。约束：

- Swift 6 + SwiftUI，部署目标 iOS 26.0，iPhone/iPad 单 target；不使用 WebView 壳、Kotlin Multiplatform、Flutter
- 只走 HTTP 契约：局域网明文 HTTP 直连 daemon，Bearer AccessKey
- iOS 当前无第三方依赖：网络用 URLSession，图片降采样用 ImageIO，凭证存 Keychain
- AI 能力：App Intents（Spotlight 语义索引）、Foundation Models（本地推理）、Vision（OCR/分类）
- M1 交付范围：局域网直连、扫码配对、齐行网格、首屏分页先出、预览/保存、搜索/筛选、文件夹树

## 移动端：hawk-android/（M1 只读浏览已实现）

原生 Android 客户端，设计见 [Android 客户端设计](frontend/android-app.md)。约束：

- Kotlin 2.4 + Jetpack Compose（Material 3），单模块 `app`，minSdk 26 / targetSdk 37；不使用 XML 布局、Fragment、Navigation Compose
- 只走 HTTP 契约：局域网明文 HTTP 直连 daemon，Bearer AccessKey
- 依赖：OkHttp 5、kotlinx.serialization、Coil 3、CameraX + ZXing（扫码）；无 GMS 依赖
- 凭证存应用私有 SharedPreferences，备份通道关闭
- M1 交付范围：连接管理、扫码配对、在线首屏、齐行网格、窗口分页、预览/保存、搜索筛选、自适应侧栏

## 契约：OpenAPI

- `hawk-daemon/openapi.json` 是唯一契约来源；Go handler 与 DTO 按它实现
- `node tools/sync-openapi.mjs` 同步到 `hawk-daemon/internal/api/openapi.json`（`go:embed` 服务 `/openapi/v1.json`）
- Go 测试 `TestEmbeddedOpenAPIMatchesRepository` 校验两份文件同步
- TypeScript 类型从 schema 生成（`npm run gen:types`）；CI 校验生成产物与仓库同步
