// 当前活动连接的响应式展示态（单例）：侧栏库名按钮的「远程」徽标（侧栏隐藏时为顶栏紧凑徽标）、
// 换库下拉远程分区与远程设置面板的数据源。连接语义本身（api_base/token 驱动 boot 重注水）在 app/startup.ts。
import { computed, ref } from 'vue';
import { shell } from '@/shared/lib/platform';
import type { ConnectionKind } from '@/shared/types';

const label = ref('我的库');
const kind = ref<ConnectionKind>('local');
/** 当前浏览的远端设备 id（本地库时为 null；换库下拉给当前远程库打勾用） */
const deviceId = ref<string | null>(null);
/** 当前浏览的局域网设备 id（kind=lan 时存在） */
const peerId = ref<string | null>(null);
const isRemote = computed(() => kind.value === 'remote');
const isLan = computed(() => kind.value === 'lan');
const isPeer = computed(() => kind.value !== 'local');
let initialized = false;

/** App 挂载时调用一次：拉取当前连接并订阅变更（事件/拉取双通道同 startup 惯例） */
export function initConnectionLabel(): void {
  if (initialized) {
    return;
  }
  initialized = true;
  const apply = (conn: { label: string; remote: boolean; kind?: ConnectionKind; device_id?: string; peer_id?: string }) => {
    label.value = conn.label;
    kind.value = conn.kind ?? (conn.remote ? 'remote' : 'local');
    deviceId.value = kind.value === 'remote' ? (conn.device_id ?? null) : null;
    peerId.value = kind.value === 'lan' ? (conn.peer_id ?? null) : null;
  };
  void shell.remoteGetConnection().then((conn) => {
    if (conn) {
      apply(conn);
    }
  });
  shell.onConnectionChanged(apply);
}

/** 侧栏库名按钮/顶栏徽标、换库下拉与远程设置面板展示：当前浏览来源（「我的库」或远端设备名）、
 *  连接形态（local/remote/lan）与对应设备 id。断开语义统一走主进程（远程会话 + 局域网直连都清理）。 */
export function useConnectionLabel() {
  async function disconnect(): Promise<void> {
    await shell.remoteDisconnect();
  }
  return { label, kind, isRemote, isLan, isPeer, deviceId, peerId, disconnect };
}
