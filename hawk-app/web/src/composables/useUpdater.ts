// 应用更新渲染层编排（Electron 桌面端；检查/下载/安装语义在主进程，见 main.cjs「应用更新」段）。
// - 模块级共享状态（同 useLayout 惯例）：设置面板「更新」分区与启动静默检查共用
// - phase 状态机：idle → checking →（uptodate | available | error）→ downloading → ready
// - 通道偏好存 localStorage；切换通道后旧检查结果作废，需重新检查
// - 静默检查（启动后延迟一次，App.vue 触发）：发现新版本 toast 一次，按 通道@版本 去重
import { ref } from 'vue';
import { hasShell, shell } from '../platform';
import { loadText, saveText, STORAGE_KEYS } from '../persist';
import { useLibraryStore } from '../stores/library';
import type { UpdateInfo, UpdateProgress } from '../types';

export type UpdaterPhase = 'idle' | 'checking' | 'uptodate' | 'available' | 'downloading' | 'ready' | 'error';

const channel = ref<'stable' | 'nightly'>(loadText(STORAGE_KEYS.updateChannel) === 'nightly' ? 'nightly' : 'stable');
const phase = ref<UpdaterPhase>('idle');
const update = ref<UpdateInfo | null>(null);
const error = ref<string | null>(null);
const progress = ref<{ received: number; total: number } | null>(null);
const verifying = ref(false);

if (hasShell) {
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

/** 切换更新通道并持久化；旧检查结果作废 */
function setChannel(next: 'stable' | 'nightly') {
  if (next === channel.value) {
    return;
  }
  channel.value = next;
  saveText(STORAGE_KEYS.updateChannel, next);
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
    phase.value = 'available';
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
    phase.value = 'error';
    error.value = e instanceof Error ? e.message : String(e);
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
  return { channel, phase, update, error, progress, verifying, setChannel, check, download, install };
}
