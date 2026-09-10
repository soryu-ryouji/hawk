// item 写操作服务层（编排层）：调 api → toast → 触发防抖重载与分类维度刷新。
// 普通函数模块而非 Pinia store——无自身状态，Pinia 文档认可的 store 外逻辑组织方式。
// 私有依赖全部走 store 公共 API（requestSkeletonReload / refreshSelectionAggregate / applyEvent，
// updateItem 响应与 SSE 事件走同一 applyEvent 入口）；taxonomy 刷新走其防抖公共入口，
// 与 SSE hooks 是同一个函数——防抖行为与拆分前零漂移。
import { api } from '@/shared/api/endpoints';
import { errorText } from '@/shared/lib/storeUtil';
import { splitKey, selectionUniqueIds } from './logic/viewLogic';
import { useLibraryStore } from './store';
import { useTaxonomyStore } from '@/domains/taxonomy';

/** 更新素材参数（名称/标签/评分/备注/URL/移动）：响应经 applyEvent 就地应用 */
export async function updateItem(id: string, patch: Parameters<typeof api.itemUpdate>[1], path?: string) {
  const store = useLibraryStore();
  try {
    const updated = await api.itemUpdate(id, patch, path);
    store.applyEvent('item.updated', updated);
  } catch (e) {
    store.showToast(errorText(e));
  }
}

/** 选中项逐个位置移入回收站（每张卡片即一个位置，删除只动该位置，其余位置保留） */
export async function trashSelected() {
  const store = useLibraryStore();
  const keys = [...store.selection];
  for (const key of keys) {
    const { id, path } = splitKey(key);
    try {
      await api.itemDelete(id, path);
    } catch (e) {
      store.showToast(errorText(e));
    }
  }
  store.clearSelection();
  store.requestSkeletonReload();
}

/** 删除单个文件位置（Inspector 文件位置列表）：item 其余位置保留；
 *  删除后经 SSE item.updated 就地刷新，最后一个库内位置被删时按整项回收 */
export async function deleteLocation(id: string, path: string) {
  const store = useLibraryStore();
  try {
    await api.itemDelete(id, path);
  } catch (e) {
    store.showToast(errorText(e));
  }
}

export async function restoreSelected() {
  const store = useLibraryStore();
  const keys = [...store.selection];
  let failed = 0;
  for (const key of keys) {
    const { id, path } = splitKey(key);
    try {
      await api.itemRestore(id, path);
    } catch (e) {
      failed++;
      store.showToast(errorText(e));
    }
  }
  store.clearSelection();
  if (failed === 0) {
    store.showToast('已恢复');
  }
  store.requestSkeletonReload();
}

export async function clearTrash() {
  const store = useLibraryStore();
  try {
    await api.trashClear();
    store.showToast('回收站已清空');
    if (store.isTrash) {
      store.requestSkeletonReload();
    }
  } catch (e) {
    store.showToast(errorText(e));
  }
}

/** 改库显示名（当前库）：写库内 config.toml 的 name；成功后就地更新库信息并返回 true */
export async function renameLibrary(name: string): Promise<boolean> {
  const store = useLibraryStore();
  try {
    store.library = await api.libraryRename(name);
    return true;
  } catch (e) {
    store.showToast(errorText(e));
    return false;
  }
}

/** 周期兜底重扫开关（库级设置，写 .hawk/config.toml 的 [scan]）：保存即热生效 */
export async function setPeriodicRescan(periodic: boolean) {
  const store = useLibraryStore();
  try {
    store.library = await api.libraryScanSet({ periodic });
    store.showToast(periodic ? '已开启周期兜底重扫' : '已关闭周期兜底重扫（仍可手动重新扫描）');
  } catch (e) {
    store.showToast(errorText(e));
  }
}

/** 手动「重新扫描」：强制遍历文件做复用判定（不读文件内容），拾取监听漏掉的新增/删除；
 *  path 缺省 = 整库，指定时只重扫该文件夹子树 */
export async function rescanFiles(path?: string, label?: string) {
  const store = useLibraryStore();
  try {
    await api.rescan(path);
    store.showToast(path ? `正在重新扫描「${label ?? path}」…` : '正在重新扫描素材库…');
  } catch (e) {
    store.showToast(errorText(e));
  }
}

/** 按范围刷新派生缓存（补缺失模式）：修复 0 × 0 宽高、缺失缩略图/调色板；修复项经 item.updated 自动刷新。
 *  附带消失对账：源文件已删但索引残留的失效位置会被清除（watcher 漏事件时的手动收敛入口） */
export async function refreshCache(type: 'folder' | 'category' | 'tag' | 'library', value?: string, label?: string) {
  const store = useLibraryStore();
  try {
    const res = await api.refreshCache(type, value);
    const parts: string[] = [];
    if (res.removed > 0) {
      parts.push(`已清除 ${res.removed} 个失效位置`);
    }
    parts.push(res.dispatched > 0 ? `正在刷新「${label ?? type}」缓存（${res.dispatched} 项）` : `「${label ?? type}」派生缓存完好，无需修复`);
    store.showToast(parts.join('，'));
    if (res.removed > 0) {
      store.requestSkeletonReload();
    }
  } catch (e) {
    store.showToast(errorText(e));
  }
}

/** 索引体检：清除不该在索引里的条目（隐藏文件 / ignore 命中 / 源文件已删的残留），
 *  只清索引不动磁盘文件；清除项经 item 丢失事件自动从界面消失 */
export async function cleanupIndex() {
  const store = useLibraryStore();
  try {
    const res = await api.cleanupIndex();
    const parts: string[] = [];
    if (res.removed > 0) {
      const detail = [
        res.hidden > 0 ? `隐藏文件 ${res.hidden}` : '',
        res.ignored > 0 ? `ignore 命中 ${res.ignored}` : '',
        res.missing > 0 ? `已删文件残留 ${res.missing}` : '',
      ]
        .filter(Boolean)
        .join('、');
      parts.push(`已清除 ${res.removed} 个错误条目（${detail}）`);
      store.requestSkeletonReload();
    } else {
      parts.push(`索引完好（检查 ${res.checked} 项，无需清理）`);
    }
    store.showToast(parts.join('，'));
  } catch (e) {
    store.showToast(errorText(e));
  }
}

/** 为全部选中项追加分类(内容级：同 hash 多位置只应用一次)。
 *  已有该分类的 id 从已加载详情一次构建（未加载的由服务端空操作跳过，不为过滤拉全量详情） */
export async function addCategoryToSelected(name: string) {
  const store = useLibraryStore();
  const existing = new Set([...store.details.values()].filter((i) => i.categories.includes(name)).map((i) => i.id));
  const ids = selectionUniqueIds(store.selection).filter((id) => !existing.has(id));
  await batchUpdate(ids, { add_categories: [name] }, '已添加分类');
  useTaxonomyStore().refreshTaxonomySoon();
}

/** 为全部选中项追加标签(同 addCategoryToSelected 的过滤策略) */
export async function addTagToSelected(tag: string) {
  const store = useLibraryStore();
  const existing = new Set([...store.details.values()].filter((i) => i.tags.includes(tag)).map((i) => i.id));
  const ids = selectionUniqueIds(store.selection).filter((id) => !existing.has(id));
  await batchUpdate(ids, { add_tags: [tag] }, '已添加标签');
  useTaxonomyStore().refreshTaxonomySoon();
}

/** 将全部选中项移动到目标文件夹(位置级：每位置各移;空字符串为根目录);已在目标文件夹的位置跳过;完成后立即刷新文件夹树 */
export async function moveSelectedToFolder(path: string) {
  const store = useLibraryStore();
  const targets = store.selection
    .map((key) => splitKey(key))
    .filter(({ path: p }) => {
      const dir = p.includes('/') ? p.slice(0, p.lastIndexOf('/')) : '';
      return dir !== path;
    });
  await batchUpdate(
    targets.map((t) => t.id),
    { paths: targets.map((t) => t.path), folder_path: path },
    '已移动',
  );
  useTaxonomyStore().refreshFoldersSoon();
}

/** 批量设置选中项评分(内容级去重;多选面板与右键菜单共用) */
export async function setStarForSelected(star: number) {
  const store = useLibraryStore();
  await batchUpdate(selectionUniqueIds(store.selection), { star }, '已设置评分');
}

/** 从全部选中项移除标签（共有标签 × 摘除）；完成后立即刷新聚合与计数 */
export async function removeTagFromSelected(tag: string) {
  const store = useLibraryStore();
  await batchUpdate(selectionUniqueIds(store.selection), { remove_tags: [tag] }, '已移除标签');
  useTaxonomyStore().refreshTaxonomySoon();
}

/** 从全部选中项移除分类（共有分类 × 摘除） */
export async function removeCategoryFromSelected(name: string) {
  const store = useLibraryStore();
  await batchUpdate(selectionUniqueIds(store.selection), { remove_categories: [name] }, '已移除分类');
  useTaxonomyStore().refreshTaxonomySoon();
}

/** 批量端点统一入口:missing(内容不存在/移动冲突)在结果中提示,不整体失败 */
async function batchUpdate(ids: string[], patch: Parameters<typeof api.itemBatchUpdate>[1], doneText: string) {
  const store = useLibraryStore();
  if (ids.length === 0) {
    return;
  }
  try {
    const res = await api.itemBatchUpdate(ids, patch);
    const parts: string[] = [];
    if (res.conflicts?.length) {
      // 同名冲突跳过的项：给出具体文件名与原因，不再只说「未处理」
      const names = res.conflicts.slice(0, 3).join('、');
      const more = res.conflicts.length > 3 ? ` 等 ${res.conflicts.length} 个` : '';
      parts.push(`${names}${more} 因目标文件夹已存在同名文件未移动`);
    }
    const skipped = res.missing_ids.length;
    if (skipped > 0) {
      parts.push(`${skipped} 个未处理`);
    }
    store.showToast(parts.length > 0 ? `${doneText}（${parts.join('，')}）` : doneText);
    // 批量写可能改变了选择集的共有特性（加/摘标签分类）→ 立即重拉聚合（不等防抖）
    if (store.selection.length > 1) {
      store.refreshSelectionAggregate();
    }
  } catch (e) {
    store.showToast(errorText(e));
  }
}
