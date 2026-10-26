// @vitest-environment jsdom
// 低带宽预览契约：广域网默认只传压缩图，当前项放大后才请求原图；本地/LAN 行为不变。
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { mount } from '@vue/test-utils';
import { nextTick } from 'vue';

const apiMocks = vi.hoisted(() => ({
  thumbnailUrl: vi.fn((id: string, size?: number) => `thumb-${id}-${size ?? 1024}`),
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

function mountPreview(remote: boolean) {
  const store = useLibraryStore();
  const preview = usePreviewStore();
  const items = [fixture('a'), fixture('b'), fixture('c')];
  store.remoteTransport = remote;
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
  apiMocks.thumbnailUrl.mockClear();
  apiMocks.fileUrl.mockClear();
});

afterEach(() => {
  document.body.innerHTML = '';
});

describe('广域网预览流量', () => {
  it('默认仅加载压缩预览，放大后仅当前项请求原图', async () => {
    const { wrapper, current } = mountPreview(true);

    expect(apiMocks.fileUrl).not.toHaveBeenCalled();
    expect(apiMocks.thumbnailUrl).toHaveBeenCalledWith(current.id, 1024);
    const trackImages = [...document.body.querySelectorAll<HTMLImageElement>('.track-img')];
    expect(trackImages).toHaveLength(3);
    expect(trackImages.every((img) => img.getAttribute('src')?.startsWith('thumb-'))).toBe(true);
    expect(document.body.querySelector('.load-status')?.textContent).toBe('正在加载预览…');

    const currentImage = trackImages.find((img) => img.alt === current.name)!;
    currentImage.dispatchEvent(new Event('load'));
    await nextTick();
    expect(document.body.querySelector('.load-status')).toBeNull();

    document.body.querySelector('.overlay')!.dispatchEvent(new WheelEvent('wheel', { deltaY: -100, bubbles: true, cancelable: true }));
    await nextTick();

    expect(apiMocks.fileUrl).toHaveBeenCalledTimes(1);
    expect(apiMocks.fileUrl).toHaveBeenCalledWith(current.id);
    const original = document.body.querySelector<HTMLImageElement>('.zoom-layer img.pending')!;
    expect(original.getAttribute('src')).toBe(`original-${current.id}`);
    expect(document.body.querySelector('.load-status')?.textContent).toBe('正在加载原图…');
    // 原图未就绪时压缩底图仍保留，不全屏黑屏
    expect(document.body.querySelectorAll('.zoom-layer img')).toHaveLength(2);

    original.dispatchEvent(new Event('load'));
    await nextTick();
    expect(document.body.querySelectorAll('.zoom-layer img')).toHaveLength(1);
    expect(document.body.querySelector('.load-status')).toBeNull();
    wrapper.unmount();
  });

  it('本地/LAN 仍按原逻辑加载原图且不显示低带宽加载条', () => {
    const { wrapper, current } = mountPreview(false);

    expect(apiMocks.thumbnailUrl).not.toHaveBeenCalled();
    expect(apiMocks.fileUrl).toHaveBeenCalledWith(current.id);
    expect(document.body.querySelector('.load-status')).toBeNull();
    wrapper.unmount();
  });
});
