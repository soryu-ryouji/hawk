// 弹窗：连接方式与状态（本地模式零配置：端口自动发现；可选的端口/Token 覆盖收在「高级选项」里）。
import { fetchLibraryInfo, listRemoteDevices, resolveLocalTarget, type RemoteDevice } from '../../lib/api';
import { resetLocalBase } from '../../lib/local-discovery';
import { getSettings, saveSettings, targetLabel, type HawkSettings } from '../../lib/settings';

const modeSelect = document.querySelector<HTMLSelectElement>('#mode')!;
const localFields = document.querySelector<HTMLDivElement>('#local-fields')!;
const remoteFields = document.querySelector<HTMLDivElement>('#remote-fields')!;
const localPortInput = document.querySelector<HTMLInputElement>('#local-port')!;
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

/** 状态行：圆点 + 文案（文案包在 .status-text 里，超长省略而不顶破弹窗宽度） */
function setStatus(text: string, kind: 'ok' | 'err' | 'wait' | 'warn') {
  const label = statusEl.querySelector('.status-text');
  if (label) {
    label.textContent = text;
  } else {
    statusEl.textContent = text;
  }
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
      setStatus(label ? `已连接 · ${label}` : `已连接 · ${info.name}`, 'ok');
      return;
    }
    // 本地模式跟随本地 hawk 当前浏览的库（可能是它正在看的远端库）
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
    // fetch 的网络失败只有 TypeError: Failed to fetch，补上最常见的原因（本地模式排查用）
    const hint = /fetch|network/i.test(message)
      ? '连不上本机 hawk（未运行，或端口被其他程序占用）。端口在 hawk 设置 → 网络 → 本地服务里；若改到了 27371–27400 之外，请在下方「高级选项」里填上端口'
      : message;
    setStatus(`未连接 · ${hint}`, 'err');
  }
}

async function init() {
  settings = await getSettings();
  modeSelect.value = settings.mode;
  localPortInput.value = settings.localPort;
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
      localPort: localPortInput.value,
      localToken: localTokenInput.value,
      remoteUrl: remoteUrlInput.value,
      serviceKey: serviceKeyInput.value,
      remoteKey: remoteKeyInput.value,
      ...(mode === 'remote' ? target : {}),
    });
    resetLocalBase(); // 端口覆盖可能变了，丢弃发现缓存（下次请求重新探活/直连）
    await refreshStatus();
  } catch (e) {
    setStatus(`保存失败：${e instanceof Error ? e.message : String(e)}`, 'err');
  }
});

checkBtn.addEventListener('click', () => {
  void refreshStatus();
});

void init();
