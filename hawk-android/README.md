# hawk-android

Android 原生客户端（Kotlin + Jetpack Compose）。设计文档见 [../docs/frontend/android-app.md](../docs/frontend/android-app.md)。

## 环境

- Android Studio（AGP 9.4.1）与 SDK Platform 37
- JDK 21+：直接用 Android Studio 自带的 JBR，`JAVA_HOME` 指向 `.../Android Studio/jbr`
- `local.properties` 里的 `sdk.dir` 由 Android Studio 生成，不进仓库（已 ignore）

## 工程现状

| 项 | 值 |
| -- | -- |
| 构建 | Gradle 9.6.0 Wrapper，Kotlin 2.4.10，AGP 9.4.1 |
| 模块 | 单模块 `app`（`com.ryouji.hawk`） |
| compileSdk / targetSdk / minSdk | 37 / 37 / 26 |
| UI | Jetpack Compose（BOM 2026.02.01，Material 3 深色主题） |
| 网络 | OkHttp 5 + kotlinx.serialization（`{status,data,error}` 信封） |
| 图片 | Coil 3（缩略图/原图解码与磁盘缓存） |
| 二维码 | CameraX + ZXing core |
| 测试 | JUnit4 纯 JVM 单测（布局算法、配对载荷、地址规范化） |

忽略规则在本目录的 `.gitignore`（`.gradle/`、`build/`、`local.properties`、本地冒烟产物 `.smoke/`）。

## 已实现（M1 只读浏览）

- 连接管理
  - 局域网直连：地址 + 访问 token，验证通过后记住；启动自动连接上次使用的连接
  - 广域网：`/viewer/gate` → `/viewer/login` 换 `bt`，再按 `bt` 绑定的设备打开查看会话；后续 `/api/v1/*` 由 hawk-server 隧道代理
  - 扫码导入配对载荷（`v/kind/url/token/tenant_key/key`）与手动输入；版本不符提示升级
  - 连接增删改、重新连接、切换连接、断开
- 浏览
  - 在线首屏不建索引：库信息 + 全局隐藏集到位即进主界面；`item/list` 首页与 `item/skeleton` 并发，分类/标签与计数后台补齐
  - 齐行网格（算法与 web `layout.ts` 一致）+ `LazyColumn` 行级懒渲染；骨架与窗口分页详情分离
  - 全屏预览：双指缩放、双击缩放、放大后拖动、左右滑动切换、下拉关闭
  - 保存原图到系统相册（API 29+ 走 MediaStore，无需权限；26–28 申请存储写权限）
  - 搜索（名称/备注）、排序（修改时间/名称/大小/评分 × 升降序）
  - 筛选：评分、短边像素区间、调色板颜色
  - 侧栏：全部/根目录/未分类/未标签/回收站、文件夹树、分类、标签与计数；手机为抽屉，宽屏（≥840dp）为常驻侧栏
  - 下拉刷新 + 回前台节流刷新
- 主题：沿用 web 的深色变量（`#1e1e1e` / `#252526` / `#2d2d30` / `#4f8cff`）

## 未实现 / 后续里程碑

- 本地索引（Room/SQLite）与离线浏览、SSE 增量同步（M2）
- App Intents 对应能力（App Actions / 系统搜索）、Glance 小组件（M2 评估）
- 端上 AI（M3 评估，Android 侧暂无与 Apple 智能对等且全机型可用的统一能力）
- 写回（标签/备注）与设备内 iroh 直连（M4）
- 锁视图解锁 UI（当前锁定的文件夹/分类/标签展示服务端 403 message）
- 仪器化 UI 测试（当前只有 JVM 单测）

## 开发与自检

一键构建（仓库根执行，产物归置到 `out/`；未设置 `JAVA_HOME` 时自动找 Android Studio 自带的 JBR）：

```bash
./tools/build-android.sh                  # debug APK → out/hawk-android-debug.apk
./tools/build-android.ps1                 # Windows（pwsh）
./tools/build-android.ps1 -Release        # release APK（未签名）→ out/hawk-android-release-unsigned.apk
# 也可走统一入口：./tools/build.sh --platform android [--release]
```

直接调用 Gradle：

```bash
export JAVA_HOME="/c/Program Files/Android/Android Studio/jbr"   # Windows Git Bash；按本机实际路径
cd hawk-android

./gradlew :app:assembleDebug        # 构建 debug APK
./gradlew :app:lintDebug            # lint（0 error 才算过）
./gradlew :app:testDebugUnitTest    # JVM 单测
./gradlew :app:assembleRelease      # R8 压缩后的 release 构建（未签名）
```

产物：`app/build/outputs/apk/debug/app-debug.apk`。

安装到模拟器/真机：

```bash
adb install -r app/build/outputs/apk/debug/app-debug.apk
adb shell am start -n com.ryouji.hawk/.MainActivity
```

### 本地冒烟（Debug）

Debug 构建支持从应用外部私有目录读取一条连接，免去手点表单（`BuildConfig.DEBUG` 限定，Release 不含该路径）：

```bash
# 1) 起一个测试库 daemon（token 走环境变量）
HAWK_ADMIN_TOKEN=smoke-token ../hawk-daemon/target/debug/hawk-daemon.exe \
  local /path/to/test-library --port 27372

# 2) 模拟器内访问宿主机端口
adb reverse tcp:27372 tcp:27372

# 3) 写好连接（kind 为 lan / remote；remote 用 tenantKey + appKey）
adb shell pm clear com.ryouji.hawk
adb shell mkdir -p /sdcard/Android/data/com.ryouji.hawk/files
mkdir -p .smoke
printf '{"kind":"lan","url":"http://127.0.0.1:27372","token":"smoke-token"}' \
  > .smoke/profile.json
adb push .smoke/profile.json /sdcard/Android/data/com.ryouji.hawk/files/hawk_profile.json

# 4) 启动
adb shell am start -n com.ryouji.hawk/.MainActivity
```

真机冒烟仍需覆盖：

1. 局域网：桌面端开 `[web]`，App 手填/扫码导入局域网地址 + token，浏览缩略图与预览
2. 广域网：填服务端地址 + tenant-key + hawk-app key，进入绑定的在线设备
3. 相册保存：保存原图，确认是原图而非 1024 缩略图（API 26–28 还需确认权限弹窗）
4. 回前台：切后台再回来，列表刷新且不白屏

## 源码结构

```text
hawk-android/app/src/main/java/com/ryouji/hawk/
├── MainActivity.kt          Compose 入口
├── HawkApplication.kt       Coil ImageLoader（内存/磁盘缓存）
├── AppViewModel.kt          启动状态机、连接编排、连接配置与缩略图尺寸
├── data/
│   ├── model/Models.kt      REST DTO、查询状态、视图枚举、信封
│   ├── net/ApiClient.kt     信封解包、Bearer、缩略图/原图 URL、错误映射
│   ├── connection/          ConnectionProfile / Store / Launcher / PairingPayload
│   └── library/LibraryStore.kt  首屏分页/骨架/详情/查询/计数
├── util/                    Format / GridLayout / PhotoSaver
└── ui/
    ├── RootScreen.kt        启动/连接中/主界面切换
    ├── connect/             连接列表、表单、扫码
    ├── library/             侧栏、网格、预览、筛选、连接信息
    └── theme/               深色主题与颜色
```
