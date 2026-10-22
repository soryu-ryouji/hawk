// 当前活动连接的响应式展示态（单例）：顶栏「当前浏览：xxx」标识、换库下拉远程分区与
// 远程设置面板的数据源。连接语义本身（api_base/token 驱动 boot 重注水）在 app/startup.ts。
import { ref } from 'vue';
import { shell } from '@/shared/lib/platform';

const label = ref('我的库');
const isRemote = ref(false);
/** 当前浏览的远端设备 id（本地库时为 null；换库下拉给当前远程库打勾用） */
const deviceId = ref<string | null>(null);
let initialized = false;

/** App 挂载时调用一次：拉取当前连接并订阅变更（事件/拉取双通道同 startup 惯例） */
export function initConnectionLabel(): void {
  if (initialized) {
    return;
  }
  initialized = true;
  const apply = (conn: { label: string; remote: boolean; device_id?: string }) => {
    label.value = conn.label;
    isRemote.value = conn.remote;
    deviceId.value = conn.remote ? (conn.device_id ?? null) : null;
  };
  void shell.remoteGetConnection().then((conn) => {
    if (conn) {
      apply(conn);
    }
  });
  shell.onConnectionChanged(apply);
}

/** 顶栏、换库下拉与远程设置面板展示：当前浏览来源（「我的库」或远端设备名）、是否远程与远端设备 id */
export function useConnectionLabel() {
  async function disconnect(): Promise<void> {
    await shell.remoteDisconnect();
  }
  return { label, isRemote, deviceId, disconnect };
}
