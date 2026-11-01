// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';
import { nextTick } from 'vue';

const apiMocks = vi.hoisted(() => ({
  itemList: vi.fn(),
  itemSkeleton: vi.fn(() => Promise.resolve({ items: [], total_size: 0 })),
  visionStatus: vi.fn(() =>
    Promise.resolve({
      installed: true,
      enabled: true,
      downloading: false,
      downloaded_bytes: 0,
      total_bytes: 0,
      indexed_items: 1,
      total_items: 1,
      pending_items: 0,
      indexing: false,
      error: null,
    }),
  ),
  visionSearch: vi.fn(),
  thumbnailUrl: vi.fn((id: string) => `thumb-${id}`),
}));
vi.mock('@/shared/api/endpoints', () => ({ api: apiMocks }));

import SimilarImageDialog from './SimilarImageDialog.vue';
import { useContextMenu } from '@/shared/composables/useContextMenu';
import { useLibraryStore } from '@/domains/library';
import { ApiError } from '@/shared/api/client';
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
  apiMocks.visionStatus.mockClear();
  apiMocks.visionSearch.mockReset();
  apiMocks.thumbnailUrl.mockClear();
});

afterEach(() => {
  vi.useRealTimers();
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

  it('缩略图缺失时显示生成中并自动重试', async () => {
    vi.useFakeTimers();
    const source = fixture('source');
    const result = { ...fixture('result'), similarity_distance: 2 };
    apiMocks.itemList.mockResolvedValue({ items: [result], total: 1, offset: 0, limit: 60, total_size: 100 });
    const wrapper = mount(SimilarImageDialog, { props: { source }, attachTo: document.body });
    await flush();

    document.body.querySelector('.result-thumb')!.dispatchEvent(new Event('error'));
    await nextTick();
    expect(document.body.querySelector('.thumb-placeholder')?.textContent).toContain('缩略图生成中');
    await vi.advanceTimersByTimeAsync(500);
    const retry = document.body.querySelector<HTMLImageElement>('.result-thumb')!;
    expect(retry.src).toContain('retry=1');
    retry.dispatchEvent(new Event('load'));
    expect(document.body.querySelector('.thumb-placeholder')).toBeNull();
    wrapper.unmount();
  });

  it('相似构图使用独立向量查询并显示余弦相似度', async () => {
    const source = fixture('source');
    const result = { ...fixture('result'), similarity_distance: 2 };
    apiMocks.visionSearch.mockResolvedValue({ items: [{ item: result, similarity_score: 0.82 }], total: 1, offset: 0, limit: 60 });
    const wrapper = mount(SimilarImageDialog, { props: { source }, attachTo: document.body });
    await flush();
    const compositionTab = [...document.body.querySelectorAll<HTMLButtonElement>('[role="tab"]')].find((button) => button.textContent?.trim() === '相似构图')!;
    compositionTab.click();
    await flush();

    expect(apiMocks.visionSearch).toHaveBeenCalledWith(expect.objectContaining({ id: 'source', min_similarity: 0.7 }));
    expect(document.body.textContent).toContain('相似度 82%');
    wrapper.unmount();
  });

  it('构图分析未就绪时自动等待并重试', async () => {
    vi.useFakeTimers();
    const source = fixture('source');
    const result = { ...fixture('result'), similarity_distance: 2 };
    apiMocks.visionSearch
      .mockRejectedValueOnce(new ApiError('NOT_READY', 'pending', 409))
      .mockResolvedValue({ items: [{ item: result, similarity_score: 0.82 }], total: 1, offset: 0, limit: 60 });
    const wrapper = mount(SimilarImageDialog, { props: { source }, attachTo: document.body });
    await flush();
    const compositionTab = [...document.body.querySelectorAll<HTMLButtonElement>('[role="tab"]')].find((button) => button.textContent?.trim() === '相似构图')!;
    compositionTab.click();
    await flush();
    expect(apiMocks.visionSearch).toHaveBeenCalledTimes(1);

    await vi.advanceTimersByTimeAsync(1200);
    await flush();
    expect(apiMocks.visionSearch).toHaveBeenCalledTimes(2);
    expect(document.body.textContent).toContain('相似度 82%');
    wrapper.unmount();
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
