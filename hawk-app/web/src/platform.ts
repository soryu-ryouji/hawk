// 平台相关的系统称呼。真实平台由 preload 注入；纯浏览器调试时按 userAgent 兜底。

const platform =
  window.hawkShell?.platform ??
  (navigator.userAgent.includes('Mac') ? 'darwin' : navigator.userAgent.includes('Windows') ? 'win32' : 'linux');

/** 系统文件管理器称呼：macOS=Finder，Windows=资源管理器，其他=文件管理器 */
export const fileManagerName =
  platform === 'darwin' ? 'Finder' : platform === 'win32' ? '资源管理器' : '文件管理器';

/** 「在 xxx 中显示」菜单/提示文案 */
export const showInFileManagerLabel = `在${fileManagerName}中显示`;
