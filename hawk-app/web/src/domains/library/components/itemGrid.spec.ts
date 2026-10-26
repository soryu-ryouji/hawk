// @vitest-environment jsdom
// 视图切换的整表加载态：远程骨架未返回时不展示上一视图，也不误报空库。
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { mount } from '@vue/test-utils';
import { nextTick } from 'vue';

vi.mock('@/shared/composables/useLayout', async () => {
  const { ref } = await import('vue');
  return { useLayout: () => ({ narrow: ref(false), touch: ref(false) }) };
});

import ItemGrid from './ItemGrid.vue';
import { useLibraryStore } from '@/domains/library';

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
});
