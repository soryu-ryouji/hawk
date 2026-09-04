// 应用更新渲染层编排（Electron 桌面端；检查/下载/安装语义在主进程，见 electron/src/updater.ts）。
// - 模块级共享状态（同 useLayout 惯例）：设置面板「更新」分区与启动静默检查共用
// - phase 状态机：idle → checking →（uptodate | available | error）→ downloading → ready
// - 通道偏好存主进程 config.toml（IPC 读写，主进程为唯一事实源）；切换通道后旧检查结果作废，需重新检查
// - 静默检查（启动后延迟一次，App.vue 触发）：发现新版本 toast 一次，按 通道@版本 去重
import { ref } from 'vue';
import { hasShell, shell } from '../platform';
import { loadText, saveText, STORAGE_KEYS } from '../persist';
import { useLibraryStore } from '../stores/library';
import { UPDATE_CANCELLED, type UpdateChannel, type UpdateInfo, type UpdateProgress } from '../types';

export type UpdaterPhase = 'idle' | 'checking' | 'uptodate' | 'available' | 'downloading' | 'ready' | 'error';

const channel = ref<UpdateChannel>('stable');
/** 通道偏好加载完成信号（check 前等待，避免启动竞态按默认 stable 误检） */
let channelReady: Promise<void> = Promise.resolve();
/** 用户是否已手动切过通道（切换先于偏好加载完成时，加载结果不得覆盖内存态） */
let channelTouched = false;
const phase = ref<UpdaterPhase>('idle');
const update = ref<UpdateInfo | null>(null);
const error = ref<string | null>(null);
const progress = ref<{ received: number; total: number } | null>(null);
const verifying = ref(false);

if (hasShell) {
  channelReady = shell.getUpdateChannel().then((c) => {
    if (!channelTouched) {
      channel.value = c;
    }
  });
  shell.onUpdateProgress((p: UpdateProgress) => {
    if (p.phase === 'downloading') {
      phase.value = 'downloading';
      verifying.value = false;
      progress.value = { received: p.received, total: p.total };
    } else if (p.phase === 'verifying') {
      verifying.value = true;
    } else if (p.phase === 'ready') {
      phase.value = 'ready';
    }
  });
}

/** 切换更新通道并持久化到主进程 config.toml；旧检查结果作废 */
function setChannel(next: UpdateChannel) {
  if (next === channel.value) {
    return;
  }
  channelTouched = true;
  channel.value = next;
  if (hasShell) {
    // 写失败静默（同旧 localStorage 行为）：内存态已生效，重启后按上次持久化值
    shell.setUpdateChannel(next).catch(() => {});
  }
  phase.value = 'idle';
  update.value = null;
  error.value = null;
  progress.value = null;
  verifying.value = false;
}

/** 检查当前通道更新。silent=true 供启动静默检查：失败静默，发现新版本 toast（同一版本只提示一次） */
async function check(silent = false): Promise<UpdateInfo | null> {
  if (!hasShell || phase.value === 'checking' || phase.value === 'downloading') {
    return null;
  }
  await channelReady; // 偏好加载完成再查，杜绝启动瞬间按默认 stable 误检
  phase.value = 'checking';
  error.value = null;
  try {
    const info = await shell.checkUpdate(channel.value);
    if (!info) {
      phase.value = 'uptodate';
      update.value = null;
      return null;
    }
    update.value = info;
    // 磁盘缓存命中（同版本已下载校验通过，如「下完未装就退出」）：跳过下载直接可安装
    phase.value = info.downloaded ? 'ready' : 'available';
    if (silent) {
      const id = `${info.channel}@${info.version}`;
      if (loadText(STORAGE_KEYS.lastUpdateNotice) !== id) {
        saveText(STORAGE_KEYS.lastUpdateNotice, id);
        useLibraryStore().showToast(`发现新版本 ${info.channel === 'nightly' ? `nightly ${info.version}` : `v${info.version}`}，设置 → 更新 可安装`);
      }
    }
    return info;
  } catch (e) {
    phase.value = 'error';
    error.value = e instanceof Error ? e.message : String(e);
    return null;
  }
}

/** 下载并校验当前检查到的更新；完成（phase=ready）经 onUpdateProgress 事件到达 */
async function download() {
  if (phase.value !== 'available') {
    return;
  }
  phase.value = 'downloading';
  error.value = null;
  progress.value = null;
  verifying.value = false;
  try {
    await shell.downloadUpdate();
    // ready 事件通常先于 invoke resolve 到达；此处兜底（如主进程幂等直接返回）
    if (phase.value === 'downloading') {
      phase.value = 'ready';
    }
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    // 用户取消不是错误：cancel() 已切回 available，此处只兜底时序竞态
    if (msg.includes(UPDATE_CANCELLED)) {
      if (phase.value === 'downloading') {
        phase.value = 'available';
      }
      progress.value = null;
      verifying.value = false;
      return;
    }
    phase.value = 'error';
    error.value = msg;
  }
}

/** 取消进行中的下载（主进程 abort 并清半成品）；回 available 可直接重新下载 */
async function cancel() {
  if (phase.value !== 'downloading') {
    return;
  }
  // 先切态：download() await 链的 ready 兜底（resolve 时 phase==='downloading'）就不会误触发
  phase.value = 'available';
  progress.value = null;
  verifying.value = false;
  try {
    await shell.cancelUpdate();
  } catch {
    // 与下载自然结束的竞态：忽略
  }
}

/** 重启并安装；成功后应用退出不再返回 */
async function install() {
  if (phase.value !== 'ready') {
    return;
  }
  try {
    await shell.installUpdate();
  } catch (e) {
    phase.value = 'error';
    error.value = e instanceof Error ? e.message : String(e);
  }
}

/** 启动静默检查（每会话一次）：主界面就绪后延迟触发，避免与启动链路抢资源 */
let autoChecked = false;

export function startupAutoCheck() {
  if (autoChecked || !hasShell) {
    return;
  }
  autoChecked = true;
  setTimeout(() => {
    void check(true);
  }, 8000);
}

export function useUpdater() {
  return { channel, phase, update, error, progress, verifying, setChannel, check, download, cancel, install };
}
