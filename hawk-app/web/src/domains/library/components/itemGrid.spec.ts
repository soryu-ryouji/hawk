// @vitest-environment jsdom
// 视图切换的整表加载态：远程骨架未返回时不展示上一视图，也不误报空库。
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { mount } from '@vue/test-utils';
import { nextTick } from 'vue';

vi.mock('@/shared/api/endpoints', () => ({
  api: { thumbnailUrl: (id: string) => `thumb-${id}` },
}));
vi.mock('@/shared/composables/useLayout', async () => {
  const { ref } = await import('vue');
  return { useLayout: () => ({ narrow: ref(false), touch: ref(false) }) };
});

import ItemGrid from './ItemGrid.vue';
import { itemKey, useLibraryStore } from '@/domains/library';
import { usePreviewStore } from '@/domains/preview';
import type { Item } from '@/shared/types';

beforeEach(() => {
  setActivePinia(createPinia());
  vi.stubGlobal(
    'ResizeObserver',
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('ItemGrid 整表加载', () => {
  it('加载中显示独立状态，结束后才按当前视图显示空态', async () => {
    const store = useLibraryStore();
    store.loading = true;
    const wrapper = mount(ItemGrid);

    expect(wrapper.find('.empty').text()).toBe('加载中…');
    expect(wrapper.text()).not.toContain('暂无素材');
    expect(wrapper.findAll('.card')).toHaveLength(0);

    store.loading = false;
    await nextTick();
    expect(wrapper.find('.empty').text()).toBe('暂无素材，拖入文件开始');
    wrapper.unmount();
  });

  it('预览切到视口外条目时同步选择并滚动网格', async () => {
    vi.spyOn(HTMLElement.prototype, 'clientWidth', 'get').mockReturnValue(800);
    vi.spyOn(HTMLElement.prototype, 'clientHeight', 'get').mockReturnValue(100);
    vi.spyOn(window, 'getComputedStyle').mockReturnValue({
      paddingLeft: '0px',
      paddingRight: '0px',
      paddingTop: '0px',
      paddingBottom: '0px',
    } as CSSStyleDeclaration);

    const store = useLibraryStore();
    const preview = usePreviewStore();
    const items: Item[] = Array.from({ length: 20 }, (_, i) => ({
      annotation: null,
      categories: [],
      ext: 'jpg',
      folders: [],
      height: 800,
      id: String(i),
      modification_time: 1,
      name: String(i),
      palette: [],
      path: `${i}.jpg`,
      paths: [`${i}.jpg`],
      size: 100,
      star: 0,
      tags: [],
      url: null,
      width: 1200,
    }));
    store.skeleton = items.map((item) => ({
      id: item.id,
      path: item.path,
      size: item.size,
      star: item.star,
      width: item.width,
      height: item.height,
    }));
    store.details = new Map(items.map((item) => [itemKey(item.id, item.path), item]));
    const wrapper = mount(ItemGrid, { attachTo: document.body });
    await nextTick();
    await nextTick();

    const key = itemKey('19', '19.jpg');
    preview.openPreview(key);
    await nextTick();

    expect(store.selection).toEqual([key]);
    expect((wrapper.find('.grid-scroll').element as HTMLElement).scrollTop).toBeGreaterThan(0);
    wrapper.unmount();
  });
});
