// IPC 契约：主进程 handler、preload 暴露、web 前端消费的三方对齐点（单一定义）。
// 通道名一律经 IPC 常量引用，不手写字符串；web 端 types.ts 从本文件 re-export。

/** IPC 通道名（invoke/handle 与 send/on 的全集） */
export const IPC = {
  // ---- 渲染进程 → 主进程（ipcRenderer.invoke / ipcMain.handle） ----
  selectLibrary: 'hawk:select-library',
  openRemoteViewer: 'hawk:open-remote-viewer',
  listLibraries: 'hawk:list-libraries',
  openLibrary: 'hawk:open-library',
  openLibraryFolder: 'hawk:open-library-folder',
  removeLibrary: 'hawk:remove-library',
  copyPath: 'hawk:copy-path',
  lanAddresses: 'hawk:lan-addresses',
  quitApp: 'hawk:quit-app',
  restartServer: 'hawk:restart-server',
  appVersion: 'hawk:app-version',
  updateChannelGet: 'hawk:update-channel-get',
  updateChannelSet: 'hawk:update-channel-set',
  closeActionGet: 'hawk:close-action-get',
  closeActionSet: 'hawk:close-action-set',
  updateCheck: 'hawk:update-check',
  updateDownload: 'hawk:update-download',
  updateCancel: 'hawk:update-cancel',
  updateInstall: 'hawk:update-install',
  showInFinder: 'hawk:show-in-finder',
  openFolder: 'hawk:open-folder',
  cacheDirGet: 'hawk:cache-dir-get',
  cacheDirPick: 'hawk:cache-dir-pick',
  cacheDirChange: 'hawk:cache-dir-change',
  apiPortGet: 'hawk:api-port-get',
  apiPortSet: 'hawk:api-port-set',
  serverRetryPort: 'hawk:server-retry-port',
  winMinimize: 'hawk:win-minimize',
  winMaximizeToggle: 'hawk:win-maximize-toggle',
  winClose: 'hawk:win-close',
  remoteGetConnection: 'hawk:remote-get-connection',
  remoteStatus: 'hawk:remote-status',
  remotePutConfig: 'hawk:remote-put-config',
  remoteDevices: 'hawk:remote-devices',
  remotePatchDevice: 'hawk:remote-patch-device',
  remoteDeleteDevice: 'hawk:remote-delete-device',
  remoteRotate: 'hawk:remote-rotate',
  remoteConnect: 'hawk:remote-connect',
  remoteDisconnect: 'hawk:remote-disconnect',
  remoteKeys: 'hawk:remote-keys',
  remoteCreateKey: 'hawk:remote-create-key',
  remotePatchKey: 'hawk:remote-patch-key',
  remoteDeleteKey: 'hawk:remote-delete-key',
  lanDiscover: 'hawk:lan-discover',
  lanPair: 'hawk:lan-pair',
  lanConnect: 'hawk:lan-connect',
  lanPeers: 'hawk:lan-peers',
  lanForget: 'hawk:lan-forget',
  lanSpeedTest: 'hawk:lan-speed-test',
  lanPairRequests: 'hawk:lan-pair-requests',
  lanClients: 'hawk:lan-clients',
  lanDecidePairRequest: 'hawk:lan-decide-pair-request',
  // ---- 主进程 → 渲染进程（webContents.send / ipcRenderer.on） ----
  winMaximized: 'hawk:win-maximized',
  serverProgress: 'hawk:server-progress',
  serverRestarting: 'hawk:server-restarting',
  serverStarted: 'hawk:server-started',
  serverConn: 'hawk:server-conn',
  serverErrorGet: 'hawk:server-error-get',
  serverError: 'hawk:server-error',
  updateProgress: 'hawk:update-progress',
  connectionChanged: 'hawk:connection-changed',
} as const;

/** 素材库历史条目（主进程记录，最近使用在前） */
export interface LibraryHistoryItem {
  path: string;
  /** 目录名（basename） */
  name: string;
  /** 目录仍存在；已删除的历史项展示但不可选 */
  exists: boolean;
}

/** 更新通道（偏好枚举，config.toml 持久化）：stable=正式版（semver 比对）/
 *  nightly=滚动版（构建 sha 比对）/ off=不检查更新 */
export type UpdateChannel = 'stable' | 'nightly' | 'off';

/** 关窗行为（偏好枚举，config.toml 持久化）：exit=关窗直接退出（默认）/
 *  tray=关窗隐藏到托盘驻留后台 */
export type CloseAction = 'exit' | 'tray';

/** 可发起检查的通道（偏好的子集：off 只是不检查，不是可查询的 Release 线） */
export type CheckableChannel = Exclude<UpdateChannel, 'off'>;

/** 应用更新信息（主进程查询 GitHub Releases 的结果） */
export interface UpdateInfo {
  channel: CheckableChannel;
  /** stable：版本号（无 v 前缀）；nightly：目标 commit 短 sha */
  version: string;
  /** Release 说明（nightly 为触发提交信息） */
  notes: string;
  /** Release 页面链接 */
  url: string;
  assetName: string;
  /** 更新包字节数（未知为 0） */
  size: number;
  /** 安装包已在本地且 sha256 校验通过（磁盘缓存命中）：可跳过下载直接安装 */
  downloaded: boolean;
}

/** 用户取消下载的哨兵错误消息（渲染层识别后静默回 available，不进 error 态；
 *  哨兵惯例同 useStartup 的 'UNAUTHORIZED'） */
export const UPDATE_CANCELLED = 'UPDATE_CANCELLED';

/** 更新包下载进度事件 */
export type UpdateProgress = { phase: 'downloading'; received: number; total: number } | { phase: 'verifying' } | { phase: 'ready' };

/** server 就绪事件负载（冷启动/换库/应用设置重启都会到达，restart 会换端口） */
export interface ServerConn {
  address: string;
  token: string;
}

/** 端口被占用的错误码（daemon 退出码 4 / stderr PORT_IN_USE，见 hawk-daemon/src/bootstrap.rs） */
export const PORT_IN_USE = 'PORT_IN_USE';

/** server 启动/运行失败负载：code=PORT_IN_USE 时启动屏显示端口冲突处理（自动换端口/手输端口） */
export interface ServerFailure {
  message: string;
  code?: typeof PORT_IN_USE;
  /** 绑定失败的端口（code=PORT_IN_USE 时存在） */
  port?: number;
}

/** 本地 API 端口信息（设置面板展示当前值 + 候选段提示） */
export interface ApiPortInfo {
  port: number;
  /** 自动选择可用的候选段（跨仓库契约，见 app-config.ts 的 API_PORT_RANGE） */
  range: readonly [number, number];
}

/** 连接形态：local=本机库；remote=经 hawk-server 的广域网远端；lan=局域网直连的另一台 hawk-app */
export type ConnectionKind = 'local' | 'remote' | 'lan';

/** 连接描述符：前端唯一面对的连接语义（api_base + token + 展示名），本地库/远端库同构 */
export interface ConnectionDescriptor {
  api_base: string;
  token: string;
  /** 「我的库」| 远端设备名 */
  label: string;
  /** 非本机连接（remote/lan）；保留布尔字段给既有展示逻辑，细分来源用 kind */
  remote: boolean;
  kind: ConnectionKind;
  /** 当前浏览的远端设备 id（kind=remote 时存在；换库下拉打勾依据） */
  device_id?: string;
  /** 当前浏览的局域网设备 id（kind=lan 时存在） */
  peer_id?: string;
}

/** 局域网发现结果：只含展示信息与配对入口，不含访问 token */
export interface LanPeer {
  id: string;
  /** 设备主机名 */
  name: string;
  library_name: string;
  /** http://<局域网 IP>:<web.port> */
  url: string;
  /** 目标设备是否允许局域网写操作 */
  writable: boolean;
  requires_pairing: boolean;
}

/** 目标端待确认的自动配对请求（仅展示字段，无访问凭证） */
export interface LanPairRequest {
  id: string;
  client_name: string;
  requested_write: boolean;
  created_at_ms: number;
}

/** 当前活跃的局域网直连设备（目标端展示用；不含 token） */
export interface LanClient {
  key_id: string;
  label: string;
  last_seen_ms: number;
  online: boolean;
}

/** 已配对并保存在本机的局域网设备（token 只在主进程配置中，不送往渲染层） */
export interface LanPeerProfile {
  id: string;
  name: string;
  library_name: string;
  url: string;
  writable: boolean;
  /** 最近成功连接时间（Unix ms；仅展示排序用） */
  last_connected_at_ms: number;
}

/** 远程访问接入配置（读写 daemon /remote 的载荷；应用级，跨素材库共享） */
export interface RemoteConfig {
  enabled: boolean;
  url: string;
  /** 服务端 service-key（池门禁凭证，hawk-server tenant add 生成） */
  service_key: string;
  /** 允许广域网会话导入素材/新建文件夹（与 [web] 局域网写权限独立） */
  writable: boolean;
  /** 使用中继（服务端提供时；消耗自建带宽，设备级开关，默认关闭） */
  relay: boolean;
}

/** 信令运行状态（daemon /remote 的 status 子对象，含 relay 等运行配置回显） */
export interface RemoteStatus {
  state: 'disabled' | 'connecting' | 'online' | 'error';
  server_url?: string;
  device_id?: string;
  error?: string;
}

/** daemon /remote 的完整响应（配置 + 运行状态） */
export interface RemoteDto extends RemoteConfig {
  status: RemoteStatus;
}

/** 服务端设备（daemon /remote/devices 单条） */
export interface RemoteDevice {
  device_id: string;
  name: string;
  platform: string;
  library_name: string;
  online: boolean;
  allow_remote: boolean;
  allow_write: boolean;
  /** 是否有直连候选（UPnP 映射或全局 IPv6）；false = 直连必失败只能中继；缺省 = 无数据 */
  reachable?: boolean;
  last_seen_at_ms?: number;
}

/** 远程会话建立结果（daemon /remote/session 响应） */
export interface RemoteSessionDto {
  session_id: string;
  target_device_id: string;
  target_name: string;
  target_library_name: string;
  proxy_port: number;
  token: string;
}

/** 访问密钥条目（daemon /remote/keys 单条；不含明文——明文只在签发响应出现一次） */
export interface RemoteKeyEntry {
  id: string;
  label: string;
  read: boolean;
  write: boolean;
  /** key 前 4 字符（列表识别用） */
  prefix: string;
  created_at_ms: number;
}

/** 签发访问密钥的返回（明文 key 只在本次出现） */
export interface RemoteKeyCreated {
  id: string;
  key: string;
  /** 是否已同步到 server 映射（false = 远程未接入/推送失败，接入后自动重试） */
  synced: boolean;
}

/** server 索引进度事件负载（total=0 表示不定态） */
export interface ServerProgress {
  phase: string;
  processed: number;
  total: number;
}

/** Electron preload 注入的白名单通道（window.hawkShell；浏览器纯前端调试时不存在） */
export interface HawkShell {
  platform: string;
  /** 更换素材库：弹目录选择框并拉起新 server；就绪经 onServerStarted 通知 */
  selectLibrary(): Promise<boolean>;
  /** 没有本地素材库时进入远程查看模式：拉起无库 daemon（不创建本地目录，不弹目录选择框） */
  openRemoteViewer(): Promise<boolean>;
  /** 本机打开过的素材库历史与当前库路径 */
  listLibraries(): Promise<{ current: string | null; libraries: LibraryHistoryItem[] }>;
  /** 打开历史素材库（仅限历史记录内的路径） */
  openLibrary(path: string): Promise<boolean>;
  /** 在系统文件管理器中打开素材库目录（仅限历史记录内的路径） */
  openLibraryFolder(path: string): Promise<void>;
  /** 从历史记录移除一条素材库（不动目录本身），返回移除后的列表 */
  removeLibrary(path: string): Promise<{ current: string | null; libraries: LibraryHistoryItem[] }>;
  /** 复制库内文件的绝对路径到剪贴板 */
  copyPath(relPath: string): Promise<void>;
  /** 重启当前 server（存储方案迁移等需要进程级重启的设置变更；主进程切启动屏并重建连接）。
   *  本地库重启同库，远程查看模式重启无库 daemon */
  restartServer(): Promise<void>;
  /** 本地 API 端口当前值与候选段（设置面板「网络 → 本地服务」） */
  getApiPort(): Promise<ApiPortInfo>;
  /** 写入本地 API 端口配置（不重启；前端随后调 restartServer 生效）。返回错误文案或 null */
  setApiPort(port: number): Promise<string | null>;
  /** 端口冲突时重试：auto=在候选段内自动挑空闲端口，port=用户手输的端口（任意 1024–65535）。
   *  写入配置后按当前模式重启 daemon。返回错误文案或 null */
  retryServerPort(request: { mode: 'auto' } | { port: number }): Promise<string | null>;
  /** 本机局域网 IPv4 地址列表（设置面板展示用；LAN 配置读写走 REST app/lan） */
  lanAddresses(): Promise<string[]>;
  /** 在系统文件管理器中显示库内文件（相对路径） */
  showInFinder(relPath: string): Promise<void>;
  /** 在系统文件管理器中打开库内文件夹本身（区别 showInFinder 的「定位到父级并选中」） */
  openFolder(relPath: string): Promise<void>;
  /** 当前缓存父目录（isDefault=true 表示系统默认路径） */
  getCacheDir(): Promise<{ current: string; isDefault: boolean }>;
  /** 弹目录选择框选新缓存父目录（取消返回 null） */
  pickCacheDir(): Promise<string | null>;
  /** 迁移缓存父目录（整体搬迁：先复制后删除；server 重启，就绪经 onServerStarted）。返回错误文案或 null */
  changeCacheDir(path: string): Promise<string | null>;
  /** 拖拽导入时取文件绝对路径（Electron webUtils） */
  getPathForFile(file: File): string;
  minimizeWindow(): Promise<void>;
  /** 最大化/还原切换，返回切换后的最大化状态 */
  toggleMaximizeWindow(): Promise<boolean>;
  closeWindow(): Promise<void>;
  /** 真正退出应用（启动错误屏用；区别于 closeWindow 的隐藏到托盘） */
  quitApp(): Promise<void>;
  /** 当前应用版本与构建 sha（sha='dev' 表示无构建标识，如开发态） */
  getAppVersion(): Promise<{ version: string; sha: string }>;
  /** 检查更新（stable=latest 正式版比 semver；nightly=滚动预发布比构建 sha）；无更新返回 null */
  checkUpdate(channel: CheckableChannel): Promise<UpdateInfo | null>;
  /** 当前更新通道偏好（主进程 config.toml 持久化；未设置回退默认值） */
  getUpdateChannel(): Promise<UpdateChannel>;
  /** 保存更新通道偏好（config.toml） */
  setUpdateChannel(channel: UpdateChannel): Promise<void>;
  /** 当前关窗行为偏好（主进程 config.toml 持久化；未设置回退默认值 exit） */
  getCloseAction(): Promise<CloseAction>;
  /** 保存关窗行为偏好（config.toml；即改即生效，无需重启） */
  setCloseAction(action: CloseAction): Promise<void>;
  /** 下载并校验上次检查到的更新（进度经 onUpdateProgress 推送；已就绪时幂等） */
  downloadUpdate(): Promise<void>;
  /** 取消进行中的下载（清半成品；无下载在跑时为空操作） */
  cancelUpdate(): Promise<void>;
  /** 重启并安装已下载的更新（成功后应用退出，不再返回） */
  installUpdate(): Promise<void>;
  /** 订阅更新包下载进度，返回退订函数 */
  onUpdateProgress(cb: (p: UpdateProgress) => void): () => void;
  /** 订阅 server 就绪（携带新地址与 token，需重配 API 并重启数据），返回退订函数。
   *  事件只发一次：页面（重）加载晚于就绪时会丢失，须与 getServerConn 拉取配合 */
  onServerStarted(cb: (conn: ServerConn) => void): () => void;
  /** 拉取当前已就绪的 server 连接（未就绪返回 null）：页面加载晚于 server 就绪的竞态兜底 */
  getServerConn(): Promise<ServerConn | null>;
  /** 拉取最近一次启动/运行失败（未失败返回 null）：daemon 端口占用在页面订阅 IPC 之前就退出，
   *  推送会丢事件，挂载时必须拉一次兜底 */
  getServerError(): Promise<ServerFailure | null>;
  /** 订阅 server 启动/运行失败（code=PORT_IN_USE 时带端口冲突信息），返回退订函数 */
  onServerError(cb: (error: ServerFailure) => void): () => void;
  /** 订阅 server 即将重启（旧 server 已停，应立即切启动屏），返回退订函数 */
  onServerRestarting(cb: () => void): () => void;
  /** 订阅 server 扫描进度（应用内启动屏用），返回退订函数 */
  onServerProgress(cb: (progress: ServerProgress) => void): () => void;
  /** 订阅最大化状态变化（含 Aero Snap 等系统途径），返回退订函数 */
  onWindowMaximized(cb: (maximized: boolean) => void): () => void;
  /** 当前活动连接描述符（本地库 / 远程会话代理 / 局域网直连）；server 未就绪返回 null */
  remoteGetConnection(): Promise<ConnectionDescriptor | null>;
  /** 远程接入配置与信令运行状态（daemon /remote） */
  remoteStatus(): Promise<RemoteDto>;
  /** 写回应用级远程配置并热重启信令 */
  remotePutConfig(config: RemoteConfig): Promise<RemoteDto>;
  /** 服务端设备列表 */
  remoteDevices(): Promise<{ devices: RemoteDevice[] }>;
  /** 更新本机设备信息（显示名/allow_remote/库名） */
  remotePatchDevice(patch: { name?: string; allow_remote?: boolean; library_name?: string }): Promise<RemoteDevice>;
  /** 注销服务端设备记录（本机当前设备由 daemon 侧拒绝）；其会话与 key 映射一并清理 */
  remoteDeleteDevice(deviceId: string): Promise<void>;
  /** 证书轮换（旧证书会话失效） */
  remoteRotate(): Promise<{ endpoint_id: string }>;
  /** 建立远程会话并切换连接（返回新连接描述符）；失败抛错（message 直接展示）。
   *  传输路径（直连/打洞/中继）由 iroh 自治，两端静态开关决定中继可用性 */
  remoteConnect(targetDeviceId: string): Promise<ConnectionDescriptor>;
  /** 断开远程会话并回切本地库（幂等） */
  remoteDisconnect(): Promise<ConnectionDescriptor | null>;
  /** 本机签发的访问密钥列表（不含明文） */
  remoteKeys(): Promise<{ keys: RemoteKeyEntry[] }>;
  /** 签发访问密钥（custom 省略 = 程序生成；明文只在返回值里出现一次） */
  remoteCreateKey(input: { label?: string; read: boolean; write?: boolean; custom?: string }): Promise<RemoteKeyCreated>;
  /** 更新访问密钥标签/权限 */
  remotePatchKey(id: string, patch: { label?: string; read?: boolean; write?: boolean }): Promise<{ key: RemoteKeyEntry; synced: boolean }>;
  /** 吊销访问密钥 */
  remoteDeleteKey(id: string): Promise<{ synced: boolean }>;
  /** 探测同网段可配对的 hawk 设备（经本机 daemon 的 UDP 发现） */
  lanDiscover(): Promise<{ peers: LanPeer[] }>;
  /** 向目标设备发起自动配对请求，等待目标端用户确认后保存连接并切换 */
  lanPair(input: { peer: LanPeer; writable: boolean }): Promise<ConnectionDescriptor>;
  /** 连接已保存的局域网设备（刷新目标 app/info 后切换连接） */
  lanConnect(peerId: string): Promise<ConnectionDescriptor>;
  /** 已保存的局域网设备列表（不含 token） */
  lanPeers(): Promise<{ peers: LanPeerProfile[] }>;
  /** 忘记已保存的局域网设备 */
  lanForget(peerId: string): Promise<void>;
  /** 测试当前局域网连接延迟（连续请求目标 app/info，返回中位数毫秒） */
  lanSpeedTest(): Promise<{ latency_ms: number }>;
  /** 本机 daemon 上待确认的自动配对请求（主进程始终连本地 daemon，不受当前浏览目标影响） */
  lanPairRequests(): Promise<{ requests: LanPairRequest[] }>;
  /** 本机 daemon 当前活跃的局域网直连设备（目标端展示） */
  lanClients(): Promise<{ clients: LanClient[] }>;
  /** 允许/拒绝自动配对请求，并决定只读或读写 */
  lanDecidePairRequest(requestId: string, body: { allow: boolean; write: boolean }): Promise<{ status: string }>;
  /** 订阅连接变更（切换本地/远程库、断线自动回切都会推送），返回退订函数。
   *  事件与拉取双通道去重同 onServerStarted 惯例：页面晚加载时经 remoteGetConnection 拉取兜底 */
  onConnectionChanged(cb: (conn: ConnectionDescriptor) => void): () => void;
}
