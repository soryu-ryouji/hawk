// 弹窗：本地/远程连接设置、目标素材库选择、连接状态。
import { fetchLibraryInfo, listRemoteDevices, type RemoteDevice } from '../../lib/api';
import { getSettings, saveSettings, targetLabel, type HawkSettings } from '../../lib/settings';

const modeSelect = document.querySelector<HTMLSelectElement>('#mode')!;
const localFields = document.querySelector<HTMLDivElement>('#local-fields')!;
const remoteFields = document.querySelector<HTMLDivElement>('#remote-fields')!;
const localUrlInput = document.querySelector<HTMLInputElement>('#local-url')!;
const localTokenInput = document.querySelector<HTMLInputElement>('#local-token')!;
const remoteUrlInput = document.querySelector<HTMLInputElement>('#remote-url')!;
const remoteKeyInput = document.querySelector<HTMLInputElement>('#remote-key')!;
const targetSelect = document.querySelector<HTMLSelectElement>('#target')!;
const statusEl = document.querySelector<HTMLParagraphElement>('#status')!;
const saveBtn = document.querySelector<HTMLButtonElement>('#save')!;
const checkBtn = document.querySelector<HTMLButtonElement>('#check')!;
const refreshTargetsBtn = document.querySelector<HTMLButtonElement>('#refresh-targets')!;

let devices: RemoteDevice[] = [];
let settings: HawkSettings;

function setStatus(text: string, kind: 'ok' | 'err' | 'wait') {
  statusEl.textContent = text;
  statusEl.dataset.kind = kind;
}

function showMode() {
  const remote = modeSelect.value === 'remote';
  localFields.hidden = remote;
  remoteFields.hidden = !remote;
}

/** 当前选中的素材库元信息（下拉项 data-* 保存展示名，服务端列表不可用时仍可保存） */
function selectedTarget(): Pick<HawkSettings, 'targetDeviceId' | 'targetName' | 'targetLibraryName'> {
  const option = targetSelect.selectedOptions[0];
  return {
    targetDeviceId: targetSelect.value,
    targetName: option?.dataset.name ?? '',
    targetLibraryName: option?.dataset.library ?? '',
  };
}

function addTargetOption(deviceId: string, label: string, name: string, libraryName: string) {
  const option = new Option(label, deviceId);
  option.dataset.name = name;
  option.dataset.library = libraryName;
  targetSelect.add(option);
}

/** 设备列表填充：只展示在线、允许远程访问、允许广域网写入的素材库 */
function fillTargets(preferredId: string) {
  targetSelect.innerHTML = '';
  const writable = devices.filter((d) => d.online && d.allow_remote && d.allow_write);
  if (writable.length === 0) {
    targetSelect.disabled = true;
    targetSelect.add(new Option('没有可写入的在线素材库', ''));
    return;
  }
  targetSelect.disabled = false;
  for (const d of writable) {
    addTargetOption(d.device_id, `${d.name} · ${d.library_name}`, d.name, d.library_name);
  }
  targetSelect.value = writable.some((d) => d.device_id === preferredId) ? preferredId : (writable[0]?.device_id ?? '');
}

async function refreshDevices(showErrors = true): Promise<void> {
  const url = remoteUrlInput.value.trim();
  const key = remoteKeyInput.value.trim();
  if (!url || !key) {
    if (showErrors) {
      setStatus('请先填写 hawk-server 域名和写入密钥', 'err');
    }
    return;
  }
  if (showErrors) {
    setStatus('加载素材库列表…', 'wait');
  }
  try {
    devices = await listRemoteDevices(url, key);
    fillTargets(targetSelect.value || settings.targetDeviceId);
    const writable = devices.filter((d) => d.online && d.allow_remote && d.allow_write);
    setStatus(
      writable.length > 0 ? `找到 ${writable.length} 个可写入素材库` : '没有在线且开启广域网写入的素材库',
      writable.length > 0 ? 'ok' : 'err',
    );
  } catch (e) {
    if (showErrors) {
      setStatus(`加载素材库失败：${e instanceof Error ? e.message : String(e)}`, 'err');
    }
  }
}

async function refreshStatus() {
  setStatus('检查连接中…', 'wait');
  try {
    const info = await fetchLibraryInfo();
    const label = targetLabel(await getSettings());
    setStatus(label ? `已连接：${info.name}（${label}）` : `已连接：${info.name}`, 'ok');
  } catch (e) {
    setStatus(`未连接：${e instanceof Error ? e.message : String(e)}`, 'err');
  }
}

async function init() {
  settings = await getSettings();
  modeSelect.value = settings.mode;
  localUrlInput.value = settings.localUrl;
  localTokenInput.value = settings.localToken;
  remoteUrlInput.value = settings.remoteUrl;
  remoteKeyInput.value = settings.remoteKey;
  showMode();

  if (settings.targetDeviceId) {
    addTargetOption(
      settings.targetDeviceId,
      targetLabel(settings),
      settings.targetName,
      settings.targetLibraryName,
    );
  } else {
    targetSelect.add(new Option('请先刷新素材库列表', ''));
    targetSelect.disabled = true;
  }

  void refreshStatus();
  if (settings.mode === 'remote' && settings.remoteUrl && settings.remoteKey) {
    void refreshDevices(false);
  }
}

modeSelect.addEventListener('change', () => {
  showMode();
  if (modeSelect.value === 'remote' && devices.length === 0) {
    void refreshDevices(false);
  }
});

refreshTargetsBtn.addEventListener('click', () => {
  void refreshDevices(true);
});

saveBtn.addEventListener('click', async () => {
  const mode = modeSelect.value === 'remote' ? 'remote' : 'local';
  const target = selectedTarget();
  if (mode === 'remote' && !target.targetDeviceId) {
    setStatus('请先刷新并选择一个可写素材库', 'err');
    return;
  }
  try {
    settings = await saveSettings({
      mode,
      localUrl: localUrlInput.value,
      localToken: localTokenInput.value,
      remoteUrl: remoteUrlInput.value,
      remoteKey: remoteKeyInput.value,
      ...(mode === 'remote' ? target : {}),
    });
    await refreshStatus();
  } catch (e) {
    setStatus(`保存失败：${e instanceof Error ? e.message : String(e)}`, 'err');
  }
});

checkBtn.addEventListener('click', () => {
  void refreshStatus();
});

void init();
