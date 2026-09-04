// SSE 订阅：EventSource 断线自动重连；重连成功后回调 onReconnect 做全量对齐。
import { apiConfig } from './client';
import type { components } from './schema';
import type { Item } from '../types';

/** 后台任务进度快照（契约见 openapi.json 的 SseEvents/TaskProgress） */
export type TaskProgress = components['schemas']['TaskProgress'];

export interface EventHandlers {
  onAdded(item: Item): void;
  /** 批量入库（扫描导入合并事件，300ms 窗口）；负载为 id 列表，客户端按「有新增」信号重载骨架 */
  onItemsAdded(ids: string[]): void;
  onUpdated(item: Item): void;
  /** item.updated 的批量变体（调色板批量回写等）；负载为 Item 数组，就地替换缓存 */
  onItemsUpdated(items: Item[]): void;
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
  listen<{ ids: string[] }>('items.added', (d) => handlers.onItemsAdded(d.ids));
  listen<Item>('item.updated', handlers.onUpdated);
  listen<{ items: Item[] }>('items.updated', (d) => handlers.onItemsUpdated(d.items));
  listen<{ id: string }>('item.trashed', (d) => handlers.onTrashed(d.id));
  listen<Item>('item.restored', handlers.onRestored);
  listen<{ id: string }>('item.removed', (d) => handlers.onRemoved(d.id));
  listen<TaskProgress>('task.progress', handlers.onTaskProgress);
  listen<{ reason: string }>('folder.changed', (d) => handlers.onFolderChanged(d.reason));

  return () => source.close();
}
