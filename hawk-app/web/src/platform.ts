// 平台相关的系统称呼与 Electron shell 能力收敛。真实平台由 preload 注入；纯浏览器调试时按 userAgent 兜底。
// 全库一律经本模块访问 shell（不再散点 window.hawkShell 判断）：
// - hasShell：能力/UI 分支（如「在文件管理器中显示」仅 Electron 出现）；
// - shell：类型化通道，浏览器端为 no-op 壳（各方法返回类型化空值），调用方无需 ?. 兜底。

/** 是否存在 Electron preload 注入的 shell（false = 纯浏览器/局域网查看） */
export const hasShell = !!window.hawkShell;

/** 运行平台：darwin/win32/linux；真实平台由 preload 注入，纯浏览器调试时按 userAgent 兜底 */
export const platform =
  window.hawkShell?.platform ??
  (navigator.userAgent.includes('Mac') ? 'darwin' : navigator.userAgent.includes('Windows') ? 'win32' : 'linux');

/** 是否 macOS（红绿灯/拖拽区等 Darwin 分支） */
export const isMac = platform === 'darwin';

/** 系统文件管理器称呼：macOS=Finder，Windows=资源管理器，其他=文件管理器 */
export const fileManagerName =
  platform === 'darwin' ? 'Finder' : platform === 'win32' ? '资源管理器' : '文件管理器';

/** 「在 xxx 中显示」菜单/提示文案 */
export const showInFileManagerLabel = `在${fileManagerName}中显示`;

/** 浏览器端 no-op 壳：每个方法返回类型化空值，语义同「能力不存在」 */
const noopShell: NonNullable<Window['hawkShell']> = {
  platform,
  selectLibrary: async () => false,
  listLibraries: async () => ({ current: null, libraries: [] }),
  openLibrary: async () => false,
  openLibraryFolder: async () => {},
  removeLibrary: async () => ({ current: null, libraries: [] }),
  lanAddresses: async () => [],
  showInFinder: async () => {},
  openFolder: async () => {},
  getCacheDir: async () => ({ current: '', isDefault: true }),
  pickCacheDir: async () => null,
  changeCacheDir: async () => '浏览器端不支持',
  copyPath: async () => {},
  getPathForFile: () => '',
  minimizeWindow: async () => {},
  toggleMaximizeWindow: async () => false,
  closeWindow: async () => {},
  quitApp: async () => {},
  getAppVersion: async () => ({ version: '', sha: 'dev' }),
  checkUpdate: async () => null,
  getUpdateChannel: async () => 'stable' as const,
  setUpdateChannel: async () => {},
  downloadUpdate: async () => {},
  cancelUpdate: async () => {},
  installUpdate: async () => {},
  onUpdateProgress: () => () => {},
  onServerStarted: () => () => {},
  getServerConn: async () => null,
  onServerError: () => () => {},
  onServerRestarting: () => () => {},
  onServerProgress: () => () => {},
  onWindowMaximized: () => () => {},
};

/** 类型化 shell 通道：Electron 走 preload 注入，浏览器走 no-op 壳 */
export const shell = window.hawkShell ?? noopShell;
