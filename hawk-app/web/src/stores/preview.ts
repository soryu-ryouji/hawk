// 预览/图片编辑域子 store：预览浮层的打开/关闭/滑动导航（sticky item 与相邻预取所需索引）
// 与图片编辑窗口（全局单例目标 + 客户端重编码保存）。
// 引用规则：可读主 store 的 skeleton/details（只读）、调 ensureWindow/showToast；主 store 不反向依赖本 store。
import { computed, ref } from 'vue';
import { defineStore } from 'pinia';
import { api } from '../api/endpoints';
import { ApiError } from '../api/client';
import { blobToBase64, rotateImage, type RotateAngle } from '../imageEdit';
import { loadText, saveText, STORAGE_KEYS } from '../persist';
import { useLibraryStore } from './library';
import { itemKey } from '../viewLogic';
import { errorText } from './util';
import type { Item } from '../types';

export const usePreviewStore = defineStore('preview', () => {
  const library = useLibraryStore();

  /** 预览目标（条目 key：同内容多位置时定位到具体位置条目） */
  const previewId = ref<string | null>(null);

  // 预览关闭按钮显隐偏好（设置面板外观分区开关，所有端即时生效）：默认显示；
  // localStorage 记忆（web 与 Electron 均持久，同 panelWidths 的全局键）
  const hidePreviewClose = ref(loadText(STORAGE_KEYS.hidePreviewClose) === '1');

  function setHidePreviewClose(on: boolean) {
    hidePreviewClose.value = on;
    saveText(STORAGE_KEYS.hidePreviewClose, on ? '1' : '0');
  }

  /** 预览浮层 sticky item：详情未加载时不置空（浮层不卸载，滑动切换动画与状态不丢）；关闭时随 previewId 归零 */
  let lastPreviewItem: Item | null = null;
  const previewItem = computed(() => {
    const current = previewId.value ? (library.details.get(previewId.value) ?? null) : null;
    if (current) {
      lastPreviewItem = current;
    }
    // sticky:详情未加载时不置空——避免浮层卸载重建导致滑动切换动画与状态丢失；关闭时随 previewId 归零
    return current ?? (previewId.value ? lastPreviewItem : null);
  });
  const previewIndex = computed(() => library.skeleton.findIndex((i) => itemKey(i.id, i.path) === previewId.value));
  const previewNavId = (step: 1 | -1) => {
    const next = previewIndex.value >= 0 ? library.skeleton[previewIndex.value + step] : undefined;
    return next ? itemKey(next.id, next.path) : null;
  };

  function openPreview(key: string) {
    previewId.value = key;
    // 详情可能未加载（如键盘导航跳到视口外项）：按骨架索引补拉，到位后浮层即出现
    const idx = library.skeleton.findIndex((s) => itemKey(s.id, s.path) === key);
    if (idx >= 0) {
      void library.ensureWindow(idx, idx + 1);
    }
  }

  function closePreview() {
    previewId.value = null;
  }

  function navigatePreview(step: 1 | -1) {
    const next = previewNavId(step);
    if (next) {
      openPreview(next);
    }
  }

  /** 图片编辑窗口的目标 item(全局单例):网格/预览浮层右键「编辑图片…」均可打开 */
  const editorTarget = ref<Item | null>(null);

  function openEditor(item: Item) {
    editorTarget.value = item;
  }

  function closeEditor() {
    editorTarget.value = null;
  }

  /**
   * 编辑窗口保存:解码/旋转/重编码在客户端完成(编辑计算归客户端),经 item/replace 提交存储层。
   * 内容哈希变化导致 id 漂移:新条目就地替换详情;预览若正打开该条目则跟随新 key;
   * 骨架/选择的旧条目由 SSE item.removed 清理。返回是否成功,调用方据此关闭编辑窗口。
   */
  async function saveImageEdit(key: string, angle: RotateAngle): Promise<boolean> {
    const item = library.details.get(key);
    if (!item) {
      return false;
    }
    try {
      // no-store:item/file 带 Cache-Control immutable,<img> 加载会把无 ACAO 的响应存进磁盘缓存,
      // 默认 cache 模式的 fetch 复用该缓存条目会被 CORS 拒绝(浏览器对 <img> 请求不携 Origin,服务端不返回 ACAO)
      const res = await fetch(api.fileUrl(item.id), { cache: 'no-store' });
      if (!res.ok) {
        throw new Error('原图获取失败');
      }
      const rotated = await rotateImage(await res.blob(), angle, item.ext);
      // replace 按 id+path 定位写回该位置（同内容多位置时只改当前条目对应的文件）
      const updated = await api.itemReplace(item.id, await blobToBase64(rotated), item.path);
      const map = new Map(library.details);
      map.delete(key);
      map.set(itemKey(updated.id, updated.path), updated);
      library.details = map;
      if (previewId.value === key) {
        previewId.value = itemKey(updated.id, updated.path);
      }
      library.showToast('已保存');
      return true;
    } catch (e) {
      // ApiError 走错误码翻译(如 UNSUPPORTED_FORMAT),本地 Error 直接取 message
      library.showToast(e instanceof ApiError ? errorText(e) : e instanceof Error ? e.message : String(e));
      return false;
    }
  }

  return {
    previewId, previewItem, previewIndex, previewNavId, openPreview, closePreview, navigatePreview,
    hidePreviewClose, setHidePreviewClose,
    editorTarget, openEditor, closeEditor, saveImageEdit,
  };
});
