// 当前活动连接的响应式展示态（单例）：顶栏「当前浏览：xxx」标识与断开入口的数据源。
// 连接语义本身（api_base/token 驱动 boot 重注水）在 app/startup.ts；这里只维护展示用的 label。
import { ref } from 'vue';
import { shell } from '@/shared/lib/platform';

const label = ref('我的库');
const isRemote = ref(false);
let initialized = false;

/** App 挂载时调用一次：拉取当前连接并订阅变更（事件/拉取双通道同 startup 惯例） */
export function initConnectionLabel(): void {
  if (initialized) {
    return;
  }
  initialized = true;
  const apply = (conn: { label: string; remote: boolean }) => {
    label.value = conn.label;
    isRemote.value = conn.remote;
  };
  void shell.remoteGetConnection().then((conn) => {
    if (conn) {
      apply(conn);
    }
  });
  shell.onConnectionChanged(apply);
}

/** 顶栏与远程设置面板展示：当前浏览来源（「我的库」或远端设备名）与是否远程 */
export function useConnectionLabel() {
  async function disconnect(): Promise<void> {
    await shell.remoteDisconnect();
  }
  return { label, isRemote, disconnect };
}
