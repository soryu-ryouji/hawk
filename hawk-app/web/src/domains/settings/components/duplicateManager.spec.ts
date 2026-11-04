// @vitest-environment jsdom
// 重复图片管理：按内容 hash 分组、默认无选择、位置级删除、部分失败与跨页选择。
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { mount } from '@vue/test-utils';

const apiMocks = vi.hoisted(() => ({
  itemDuplicates: vi.fn(),
  itemDelete: vi.fn(),
  thumbnailUrl: vi.fn((id: string) => `thumb-${id}`),
}));
vi.mock('@/shared/api/endpoints', () => ({ api: apiMocks }));

import DuplicateManager from './DuplicateManager.vue';
import { useLibraryStore } from '@/domains/library';
import type { DuplicateEntry, DuplicateGroup, DuplicateScanResponse } from '@/shared/types';

function entry(id: string, path: string): DuplicateEntry {
  return {
    item: {
      annotation: null,
      categories: [],
      ext: 'png',
      folders: [],
      height: 1080,
      id,
      modification_time: 1,
      name: id,
      palette: [],
      path,
      paths: [path],
      size: 1024,
      star: 0,
      tags: [],
      url: null,
      width: 1920,
    },
  };
}

function group(id: string, entries: DuplicateEntry[]): DuplicateGroup {
  return { id, entries };
}

function response(groups: DuplicateGroup[], total = groups.length): DuplicateScanResponse {
  return { groups, total, offset: 0, limit: 50 };
}

async function flush(ticks = 20) {
  for (let i = 0; i < ticks; i++) {
    await Promise.resolve();
  }
}

function button(text: string): HTMLButtonElement {
  const found = [...document.body.querySelectorAll<HTMLButtonElement>('button')].find((candidate) => candidate.textContent?.trim() === text);
  expect(found, `按钮 ${text}`).toBeDefined();
  return found!;
}

async function clickButton(text: string) {
  button(text).click();
  await flush();
}

function checkboxes(): HTMLInputElement[] {
  return [...document.body.querySelectorAll<HTMLInputElement>('input[type="checkbox"]')];
}

async function toggleCheckbox(index: number) {
  const box = checkboxes()[index];
  expect(box).toBeDefined();
  box.checked = true;
  box.dispatchEvent(new Event('change', { bubbles: true }));
  await flush();
}

function bodyText(): string {
  return document.body.textContent ?? '';
}

async function clickDialogButton(text: string) {
  await clickButton(text);
}
beforeEach(() => {
  setActivePinia(createPinia());
  apiMocks.itemDuplicates.mockReset();
  apiMocks.itemDelete.mockReset();
  apiMocks.itemDelete.mockResolvedValue(undefined);
  apiMocks.thumbnailUrl.mockClear();
});

afterEach(() => {
  document.body.innerHTML = '';
});

describe('DuplicateManager', () => {
  it('扫描只按内容 hash 分组，不传相似度参数', async () => {
    apiMocks.itemDuplicates.mockResolvedValue(response([]));
    const wrapper = mount(DuplicateManager);
    await clickButton('开始扫描');
    expect(apiMocks.itemDuplicates).toHaveBeenLastCalledWith({ offset: 0, limit: 50 });
    expect(document.body.querySelector('input[type="range"]')).toBeNull();
    wrapper.unmount();
  });

  it('删除按钮固定在弹窗顶部，默认不选中并展示元数据', async () => {
    const groups = [group('g', [entry('a', 'folder/a.png'), entry('b', 'folder/b.png')])];
    apiMocks.itemDuplicates.mockResolvedValue(response(groups));
    const wrapper = mount(DuplicateManager);

    await clickButton('开始扫描');
    expect(bodyText()).toContain('1.0 KB');
    expect(bodyText()).toContain('1920 × 1080');
    expect(bodyText()).toContain('文件夹：folder');
    const boxes = checkboxes();
    expect(boxes).toHaveLength(2);
    expect(boxes.every((box) => !box.checked)).toBe(true);
    expect(bodyText()).toContain('已选择 0 个文件位置');
    expect(bodyText()).toContain('删除选中');

    await toggleCheckbox(0);
    expect(bodyText()).toContain('已选择 1 个文件位置');
    await clickButton('删除选中');
    expect(document.body.querySelector('[role="dialog"]')).not.toBeNull();
    await clickDialogButton('确认移入回收站');

    expect(apiMocks.itemDelete).toHaveBeenCalledTimes(1);
    expect(apiMocks.itemDelete).toHaveBeenCalledWith('a', 'folder/a.png');
    expect(useLibraryStore().toast).toBe('已将 1 个文件位置移入回收站');
    wrapper.unmount();
  });

  it('缩略图加载失败时显示占位并自动重试', async () => {
    vi.useFakeTimers();
    apiMocks.itemDuplicates.mockResolvedValue(response([group('g', [entry('a', 'a.png')])]));
    const wrapper = mount(DuplicateManager);
    await clickButton('开始扫描');
    const image = document.body.querySelector<HTMLImageElement>('img.duplicate-thumb');
    expect(image).not.toBeNull();

    image!.dispatchEvent(new Event('error'));
    await flush();
    expect(document.body.querySelector('.thumb-placeholder')?.textContent).toContain('缩略图生成中');

    await vi.advanceTimersByTimeAsync(500);
    await flush();
    expect(document.body.querySelector<HTMLImageElement>('img.duplicate-thumb')?.src).toContain('&retry=1');
    wrapper.unmount();
  });

  it('部分删除失败保留失败项选中并提示', async () => {
    apiMocks.itemDuplicates.mockResolvedValue(response([group('g', [entry('a', 'a.png')])]));
    apiMocks.itemDelete.mockRejectedValue(new Error('占用中'));
    const wrapper = mount(DuplicateManager);
    await clickButton('开始扫描');
    await toggleCheckbox(0);
    await clickButton('删除选中');
    await clickDialogButton('确认移入回收站');

    expect(bodyText()).toContain('1 个失败');
    expect(checkboxes()[0].checked).toBe(true);
    wrapper.unmount();
  });

  it('跨页保留选择并逐位置提交', async () => {
    apiMocks.itemDuplicates.mockImplementation(({ offset = 0 }: { offset?: number }) => {
      return Promise.resolve(offset === 0 ? response([group('g1', [entry('a', 'a.png')])], 51) : response([group('g2', [entry('b', 'b.png')])], 51));
    });
    const wrapper = mount(DuplicateManager);
    await clickButton('开始扫描');
    await toggleCheckbox(0);
    await clickButton('下一页');
    await toggleCheckbox(0);
    expect(bodyText()).toContain('已选择 2 个文件位置');

    await clickButton('删除选中');
    await clickDialogButton('确认移入回收站');
    expect(apiMocks.itemDelete).toHaveBeenCalledWith('a', 'a.png');
    expect(apiMocks.itemDelete).toHaveBeenCalledWith('b', 'b.png');
    wrapper.unmount();
  });
});
