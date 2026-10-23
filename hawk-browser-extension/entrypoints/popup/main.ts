// 弹窗：本地/远程连接设置、目标素材库选择、连接状态。
import { fetchLibraryInfo, listRemoteDevices, resolveLocalTarget, type RemoteDevice } from '../../lib/api';
import { getSettings, saveSettings, targetLabel, type HawkSettings } from '../../lib/settings';

const modeSelect = document.querySelector<HTMLSelectElement>('#mode')!;
const localFields = document.querySelector<HTMLDivElement>('#local-fields')!;
const remoteFields = document.querySelector<HTMLDivElement>('#remote-fields')!;
const localUrlInput = document.querySelector<HTMLInputElement>('#local-url')!;
const localTokenInput = document.querySelector<HTMLInputElement>('#local-token')!;
const remoteUrlInput = document.querySelector<HTMLInputElement>('#remote-url')!;
const serviceKeyInput = document.querySelector<HTMLInputElement>('#service-key')!;
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
  const sk = serviceKeyInput.value.trim();
  const key = remoteKeyInput.value.trim();
  if (!url || !sk || !key) {
    if (showErrors) {
      setStatus('请先填写 hawk-server 域名、服务密钥与访问密钥', 'err');
    }
    return;
  }
  if (showErrors) {
    setStatus('验证密钥中…', 'wait');
  }
  try {
    devices = await listRemoteDevices(url, sk, key);
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
    if (settings.mode === 'remote') {
      const label = targetLabel(settings);
      setStatus(label ? `已连接：${info.name}（${label}）` : `已连接：${info.name}`, 'ok');
      return;
    }
    // 本地模式跟随本地 hawk 当前浏览的库（可能是它正在看的远端库）
    const target = await resolveLocalTarget();
    if (target.kind === 'remote') {
      setStatus(
        target.writable
          ? `已连接：${info.name}（跟随本地 hawk · ${target.label}）`
          : `只读：${info.name}（跟随本地 hawk · ${target.label}，目标未开启远程写入）`,
        target.writable ? 'ok' : 'err',
      );
      return;
    }
    setStatus(`已连接：${info.name}（本地库）`, 'ok');
  } catch (e) {
    const message = e instanceof Error ? e.message : String(e);
    // fetch 的网络失败只有 TypeError: Failed to fetch，补上最常见的原因（本地模式排查用）
    const hint = /fetch|network/i.test(message)
      ? '本地 hawk 未运行，或 daemon 端口不是 27371（端口被占用时 app 会回退随机端口，需在设置里改地址）'
      : message;
    setStatus(`未连接：${hint}`, 'err');
  }
}

async function init() {
  settings = await getSettings();
  modeSelect.value = settings.mode;
  localUrlInput.value = settings.localUrl;
  localTokenInput.value = settings.localToken;
  remoteUrlInput.value = settings.remoteUrl;
  serviceKeyInput.value = settings.serviceKey;
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
  if (settings.mode === 'remote' && settings.remoteUrl && settings.serviceKey && settings.remoteKey) {
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
      serviceKey: serviceKeyInput.value,
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
