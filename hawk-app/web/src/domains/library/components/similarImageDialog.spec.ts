// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';

const apiMocks = vi.hoisted(() => ({
  itemList: vi.fn(),
  itemSkeleton: vi.fn(() => Promise.resolve({ items: [], total_size: 0 })),
  thumbnailUrl: vi.fn((id: string) => `thumb-${id}`),
}));
vi.mock('@/shared/api/endpoints', () => ({ api: apiMocks }));

import SimilarImageDialog from './SimilarImageDialog.vue';
import { useContextMenu } from '@/shared/composables/useContextMenu';
import { useLibraryStore } from '@/domains/library';
import type { Item } from '@/shared/types';

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

async function flush(ticks = 20) {
  for (let i = 0; i < ticks; i++) {
    await Promise.resolve();
  }
}

beforeEach(() => {
  localStorage.clear();
  setActivePinia(createPinia());
  apiMocks.itemList.mockReset();
  apiMocks.itemSkeleton.mockClear();
  apiMocks.thumbnailUrl.mockClear();
});

afterEach(() => {
  document.body.innerHTML = '';
});

describe('图片查找弹窗', () => {
  it('默认严格查询，用户可通过滑块连续调整阈值', async () => {
    const source = fixture('source');
    const result = { ...fixture('result'), similarity_distance: 2 };
    apiMocks.itemList.mockResolvedValue({ items: [result], total: 1, offset: 0, limit: 60, total_size: 100 });
    const wrapper = mount(SimilarImageDialog, { props: { source }, attachTo: document.body });
    await flush();

    expect(apiMocks.itemList).toHaveBeenLastCalledWith(expect.objectContaining({ similar_to: 'source', max_distance: 10, order_by: 'similarity' }));
    expect(document.body.textContent).toContain('result.jpg');
    expect(document.body.textContent).toContain('差异 2');

    const slider = document.body.querySelector<HTMLInputElement>('input[type="range"]')!;
    slider.value = '25';
    slider.dispatchEvent(new Event('input', { bubbles: true }));
    await flush();
    expect(apiMocks.itemList).toHaveBeenCalledTimes(1);
    slider.dispatchEvent(new Event('change', { bubbles: true }));
    await flush();
    expect(apiMocks.itemList).toHaveBeenLastCalledWith(expect.objectContaining({ similar_to: 'source', max_distance: 15 }));

    slider.value = '90';
    slider.dispatchEvent(new Event('input', { bubbles: true }));
    await flush();
    expect(apiMocks.itemList).toHaveBeenCalledTimes(2);
    slider.dispatchEvent(new Event('change', { bubbles: true }));
    await flush();
    expect(apiMocks.itemList).toHaveBeenCalledTimes(3);
    expect(apiMocks.itemList).toHaveBeenLastCalledWith(expect.objectContaining({ max_distance: 2 }));

    document.body.querySelector('.dialog')!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    expect(wrapper.emitted('close')).toHaveLength(1);
    wrapper.unmount();
  });

  it('记住相似度设置并在下次打开时使用', async () => {
    const source = fixture('source');
    apiMocks.itemList.mockResolvedValue({ items: [], total: 0, offset: 0, limit: 60, total_size: 0 });
    const wrapper = mount(SimilarImageDialog, { props: { source }, attachTo: document.body });
    await flush();
    const slider = document.body.querySelector<HTMLInputElement>('input[type="range"]')!;
    slider.value = '61';
    slider.dispatchEvent(new Event('input', { bubbles: true }));
    slider.dispatchEvent(new Event('change', { bubbles: true }));
    await flush();
    expect(localStorage.getItem('hawk:similarityThreshold')).toBe('61');
    wrapper.unmount();

    mount(SimilarImageDialog, { props: { source }, attachTo: document.body });
    await flush();
    expect(apiMocks.itemList).toHaveBeenLastCalledWith(expect.objectContaining({ max_distance: 8 }));
    expect(document.body.querySelector<HTMLInputElement>('input[type="range"]')!.value).toBe('61');
  });

  it('结果卡片右键可跳转到图片所在文件夹', async () => {
    const source = fixture('source');
    const result = { ...fixture('result'), path: 'folder/result.jpg', paths: ['folder/result.jpg'], similarity_distance: 2 };
    apiMocks.itemList.mockResolvedValue({ items: [result], total: 1, offset: 0, limit: 60, total_size: 100 });
    const wrapper = mount(SimilarImageDialog, { props: { source }, attachTo: document.body });
    await flush();

    document.body.querySelector('.result-card')!.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }));
    const entry = useContextMenu().state.items.find((item) => item.label === '跳转到所在文件夹');
    expect(entry).toBeDefined();
    entry?.action?.();

    expect(wrapper.emitted('close')).toHaveLength(1);
    expect(useLibraryStore().view).toEqual({ kind: 'folder', path: 'folder' });
    wrapper.unmount();
  });
});
