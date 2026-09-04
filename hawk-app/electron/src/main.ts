// hawk-app Electron 主进程入口：单实例、生命周期、模块装配。
// 业务数据一律走 REST，不经 IPC（见 docs/architecture.md、docs/frontend/hawk-app.md）。
// 模块划分：window（窗口/托盘）、server（hawk-daemon 进程管理）、app-config（用户配置）、
// updater（应用更新）、lan（局域网地址）、ipc（白名单通道）。
import { app, dialog } from 'electron';
import fs from 'node:fs';
import path from 'node:path';
import { createTray, createWindow, loadMainPage, setQuitting, showMainWindow } from './window';
import { openLibraryAt, stopServer } from './server';
import { readConfig } from './app-config';
import { registerIpc } from './ipc';
import { registerUpdaterIpc } from './updater';

// Electron 会话数据（localStorage/缓存等）走平台默认位置：appData 父目录 + 固定 hawk-app 子目录。
// 不依赖运行时应用名的默认解析：打包版 productName=hawk 在 Linux 会把默认 userData 解析为
// ~/.config/hawk，与应用自有配置目录（paths.ts 的 CONFIG_DIR）相撞，Chromium 数据会混进去
app.setPath('userData', path.join(app.getPath('appData'), 'hawk-app'));

// 单实例：托盘驻留期间再次启动（双击图标/快捷方式）应唤起已有窗口，而不是拉起第二个实例
// （第二个实例会拉起第二套 hawk-daemon 进程争用同一素材库，引发索引与文件监听竞争）
if (!app.requestSingleInstanceLock()) {
  app.quit();
} else {
  app.on('second-instance', showMainWindow);
}

app.whenReady().then(async () => {
  registerIpc();
  registerUpdaterIpc();
  createWindow();
  createTray();

  const libPath = readConfig().libraryPath;
  if (!libPath || !fs.existsSync(libPath)) {
    loadMainPage(); // 素材库未配置或已失效：进应用内引导页（无连接参数）
    return;
  }

  // 端口/token 先生成、页面立即加载并显示应用内启动屏，server 后台拉起——
  // 窗口内容单页生命周期，无 loading→主界面二次导航，杜绝切换白屏
  try {
    const server = await openLibraryAt(libPath);
    loadMainPage({ address: server.address, token: server.token });
  } catch (error) {
    dialog.showErrorBox('hawk-daemon 启动失败', String(error));
    app.quit();
  }
});

// 关窗只是隐藏到托盘（close 已被拦截，正常不会走到这里）；不监听此事件的话 Electron 默认关窗即退出
app.on('window-all-closed', () => {});
// 真正退出（托盘菜单「退出」、macOS Cmd+Q）：放行 close 拦截，由 will-quit 回收 server
app.on('before-quit', setQuitting);
// macOS：关窗（隐藏到托盘）后点击 Dock 图标重新打开
app.on('activate', showMainWindow);
app.on('will-quit', stopServer);
