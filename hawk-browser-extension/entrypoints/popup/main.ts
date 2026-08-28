// 弹窗：连接状态展示 + 服务器地址/Token 设置
import { fetchLibraryInfo } from '../../lib/api';
import { getSettings, saveSettings } from '../../lib/settings';

const serverUrlInput = document.querySelector<HTMLInputElement>('#server-url')!;
const tokenInput = document.querySelector<HTMLInputElement>('#token')!;
const statusEl = document.querySelector<HTMLParagraphElement>('#status')!;
const saveBtn = document.querySelector<HTMLButtonElement>('#save')!;
const checkBtn = document.querySelector<HTMLButtonElement>('#check')!;

function setStatus(text: string, kind: 'ok' | 'err' | 'wait') {
  statusEl.textContent = text;
  statusEl.dataset.kind = kind;
}

async function refreshStatus() {
  setStatus('检查连接中…', 'wait');
  try {
    const info = await fetchLibraryInfo();
    setStatus(`已连接：${info.name}`, 'ok');
  } catch (e) {
    setStatus(`未连接：${e instanceof Error ? e.message : String(e)}`, 'err');
  }
}

async function init() {
  const settings = await getSettings();
  serverUrlInput.value = settings.serverUrl;
  tokenInput.value = settings.token;
  void refreshStatus();
}

saveBtn.addEventListener('click', async () => {
  await saveSettings({ serverUrl: serverUrlInput.value, token: tokenInput.value.trim() });
  await refreshStatus();
});

checkBtn.addEventListener('click', refreshStatus);

void init();
