// SSE 订阅：EventSource 断线自动重连；重连成功后回调 onReconnect 做全量对齐。
import { apiConfig } from './client';
import type { Item } from '../types';

export interface TaskProgress {
  task: string;
  pending: number;
  active: number;
}

export interface EventHandlers {
  onAdded(item: Item): void;
  onUpdated(item: Item): void;
  onTrashed(id: string): void;
  onRestored(item: Item): void;
  onRemoved(id: string): void;
  onTaskProgress(progress: TaskProgress): void;
  /** 目录结构变化:本端文件夹操作、外部进程改动、对账扫描兜底;reason 恒为 external,忽略取值 */
  onFolderChanged(reason: string): void;
  onReconnect(): void;
}

export function connectEvents(handlers: EventHandlers): () => void {
  const { api, token } = apiConfig();
  const source = new EventSource(`${api}/api/v1/events?token=${encodeURIComponent(token)}`);
  let hadError = false;

  source.onopen = () => {
    if (hadError) {
      handlers.onReconnect();
    }
    hadError = false;
  };
  source.onerror = () => {
    hadError = true;
  };

  const listen = <T>(type: string, fn: (data: T) => void) =>
    source.addEventListener(type, (e) => fn(JSON.parse((e as MessageEvent).data as string) as T));

  listen<Item>('item.added', handlers.onAdded);
  listen<Item>('item.updated', handlers.onUpdated);
  listen<{ id: string }>('item.trashed', (d) => handlers.onTrashed(d.id));
  listen<Item>('item.restored', handlers.onRestored);
  listen<{ id: string }>('item.removed', (d) => handlers.onRemoved(d.id));
  listen<TaskProgress>('task.progress', handlers.onTaskProgress);
  listen<{ reason: string }>('folder.changed', (d) => handlers.onFolderChanged(d.reason));

  return () => source.close();
}
