// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { mount } from '@vue/test-utils';
import { defineComponent, h } from 'vue';

const apiMocks = vi.hoisted(() => ({
  itemDelete: vi.fn((_id: string, _path?: string): Promise<void> => Promise.resolve()),
  itemRestore: vi.fn((_id: string, _path?: string): Promise<void> => Promise.resolve()),
}));
vi.mock('@/shared/api/endpoints', () => ({ api: apiMocks }));

import { useShortcuts } from './shortcuts';
import { itemKey, useLibraryStore } from '@/domains/library';
import { usePreviewStore } from '@/domains/preview';
import type { Item, SkeletonItem } from '@/shared/types';

const ShortcutHost = defineComponent({
  setup() {
    useShortcuts();
    return () => h('div');
  },
});

function fixture(id: string): Item {
  return {
    annotation: null,
    categories: [],
    ext: 'jpg',
    folders: [],
    height: 800,
    id,
    modification_time: 1,
    name: id,
    palette: [],
    path: `${id}.jpg`,
    paths: [`${id}.jpg`],
    size: 100,
    star: 0,
    tags: [],
    url: null,
    width: 1200,
  };
}

function skeleton(item: Item): SkeletonItem {
  return {
    id: item.id,
    path: item.path,
    size: item.size,
    star: item.star,
    width: item.width,
    height: item.height,
  };
}

function press(key: string) {
  window.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true }));
}

beforeEach(() => {
  setActivePinia(createPinia());
  apiMocks.itemDelete.mockClear();
  apiMocks.itemRestore.mockClear();
});

afterEach(() => {
  document.body.innerHTML = '';
});

describe('预览快捷键', () => {
  it('左右切换同步网格选择，Delete 删除当前预览并显示下一张', async () => {
    const store = useLibraryStore();
    const preview = usePreviewStore();
    const items = [fixture('a'), fixture('b'), fixture('c')];
    store.skeleton = items.map(skeleton);
    store.details = new Map(items.map((item) => [itemKey(item.id, item.path), item]));
    const wrapper = mount(ShortcutHost, { attachTo: document.body });

    preview.openPreview(itemKey('a', 'a.jpg'));
    press('ArrowRight');
    expect(preview.previewId).toBe(itemKey('b', 'b.jpg'));
    expect(store.selection).toEqual([itemKey('b', 'b.jpg')]);

    press('Delete');
    await vi.waitFor(() => expect(preview.previewId).toBe(itemKey('c', 'c.jpg')));

    expect(apiMocks.itemDelete).toHaveBeenCalledTimes(1);
    expect(apiMocks.itemDelete).toHaveBeenCalledWith('b', 'b.jpg');
    expect(store.selection).toEqual([itemKey('c', 'c.jpg')]);
    wrapper.unmount();
  });
});
