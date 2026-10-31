// 弹窗：本机 hawk 与当前连接。公网 viewer 已删除，远程保存统一经本机 hawk。
import { fetchLibraryInfo, resolveLocalTarget } from '../../lib/api';
import { resetLocalBase } from '../../lib/local-discovery';
import { getSettings, saveSettings, type HawkSettings } from '../../lib/settings';

const localPortInput = document.querySelector<HTMLInputElement>('#local-port')!;
const localTokenInput = document.querySelector<HTMLInputElement>('#local-token')!;
const statusEl = document.querySelector<HTMLParagraphElement>('#status')!;
const saveBtn = document.querySelector<HTMLButtonElement>('#save')!;
const checkBtn = document.querySelector<HTMLButtonElement>('#check')!;
let settings: HawkSettings;

function setStatus(text: string, kind: 'ok' | 'err' | 'wait' | 'warn') {
  const label = statusEl.querySelector('.status-text');
  if (label) {
    label.textContent = text;
  } else {
    statusEl.textContent = text;
  }
  statusEl.dataset.kind = kind;
}

async function refreshStatus() {
  setStatus('检查连接中…', 'wait');
  try {
    const info = await fetchLibraryInfo();
    const target = await resolveLocalTarget();
    if (target.kind === 'remote') {
      setStatus(
        target.writable ? `已连接 · ${target.label || info.name}` : `只读 · ${target.label || info.name}`,
        target.writable ? 'ok' : 'warn',
      );
      return;
    }
    setStatus(`已连接 · ${info.name}`, 'ok');
  } catch (e) {
    const message = e instanceof Error ? e.message : String(e);
    const hint = /fetch|network/i.test(message)
      ? '连不上本机 hawk（未运行，或端口被其他程序占用）。端口在 hawk 设置 → 网络 → 本地服务里；若改到了 27371–27400 之外，请在下方「高级选项」里填上端口'
      : message;
    setStatus(`未连接 · ${hint}`, 'err');
  }
}

async function init() {
  settings = await getSettings();
  localPortInput.value = settings.localPort;
  localTokenInput.value = settings.localToken;
  await refreshStatus();
}

saveBtn.addEventListener('click', async () => {
  try {
    settings = await saveSettings({
      mode: 'local',
      localPort: localPortInput.value,
      localToken: localTokenInput.value,
    });
    resetLocalBase();
    await refreshStatus();
  } catch (e) {
    setStatus(`保存失败：${e instanceof Error ? e.message : String(e)}`, 'err');
  }
});

checkBtn.addEventListener('click', () => {
  void refreshStatus();
});

void init();
