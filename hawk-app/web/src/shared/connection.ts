// 当前活动连接的响应式展示态（单例）：本地库 / 局域网直连。
// 连接语义本身（api_base/token 驱动 boot 重注水）在 app/startup.ts。
import { computed, ref } from 'vue';
import { shell } from '@/shared/lib/platform';
import type { ConnectionKind } from '@/shared/types';

const label = ref('我的库');
const kind = ref<ConnectionKind>('local');
const peerId = ref<string | null>(null);
const isLan = computed(() => kind.value === 'lan');
const isPeer = computed(() => kind.value !== 'local');
let initialized = false;

/** IPC invoke 错误消息剥壳：'Error invoking remote method ...: Error: <message>' → <message> */
export function ipcMessage(error: unknown): string {
  const raw = error instanceof Error ? error.message : String(error);
  return raw.replace(/^Error invoking remote method '[^']+': (Error: )?/, '');
}

export function initConnectionLabel(): void {
  if (initialized) {
    return;
  }
  initialized = true;
  const apply = (conn: { label: string; kind?: ConnectionKind; peer_id?: string }) => {
    label.value = conn.label;
    kind.value = conn.kind ?? 'local';
    peerId.value = kind.value === 'lan' ? (conn.peer_id ?? null) : null;
  };
  void shell.getConnection().then((conn) => {
    if (conn) {
      apply(conn);
    }
  });
  shell.onConnectionChanged(apply);
}

export function useConnectionLabel() {
  async function disconnect(): Promise<void> {
    await shell.disconnectConnection();
  }
  return { label, kind, isLan, isPeer, peerId, disconnect };
}
