// @vitest-environment jsdom
// 局域网预览：直接使用原图。
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { mount } from '@vue/test-utils';

const apiMocks = vi.hoisted(() => ({
  fileUrl: vi.fn((id: string) => `original-${id}`),
}));
vi.mock('@/shared/api/endpoints', () => ({ api: apiMocks }));
vi.mock('@/shared/composables/useLayout', async () => {
  const { ref } = await import('vue');
  return { useLayout: () => ({ narrow: ref(false), touch: ref(false) }) };
});

import PreviewOverlay from './PreviewOverlay.vue';
import { useLibraryStore } from '@/domains/library';
import { usePreviewStore } from '@/domains/preview';
import { itemKey } from '@/domains/library';
import type { Item, SkeletonItem } from '@/shared/types';

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

function mountPreview() {
  const store = useLibraryStore();
  const preview = usePreviewStore();
  const items = [fixture('a'), fixture('b'), fixture('c')];
  store.skeleton = items.map(skeleton);
  store.details = new Map(items.map((item) => [itemKey(item.id, item.path), item]));
  preview.openPreview(itemKey(items[1].id, items[1].path));
  const wrapper = mount(PreviewOverlay, {
    props: { item: items[1] },
    attachTo: document.body,
  });
  return { wrapper, current: items[1] };
}

beforeEach(() => {
  setActivePinia(createPinia());
  apiMocks.fileUrl.mockClear();
});

afterEach(() => {
  document.body.innerHTML = '';
});

describe('局域网预览', () => {
  it('carousel 与缩放层都使用原图，不显示低带宽加载条', () => {
    const { wrapper, current } = mountPreview();

    expect(apiMocks.fileUrl).toHaveBeenCalledWith(current.id);
    const trackImages = [...document.body.querySelectorAll<HTMLImageElement>('.track-img')];
    expect(trackImages).toHaveLength(3);
    expect(trackImages.every((img) => img.getAttribute('src')?.startsWith('original-'))).toBe(true);
    expect(document.body.querySelector('.load-status')).toBeNull();
    wrapper.unmount();
  });
});
