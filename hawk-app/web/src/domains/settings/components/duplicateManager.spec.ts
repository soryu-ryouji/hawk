// @vitest-environment jsdom
// 重复图片管理：默认无选择、元数据展示、位置级删除、部分失败与跨页选择。
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { mount, type VueWrapper } from '@vue/test-utils';

const apiMocks = vi.hoisted(() => ({
  itemDuplicates: vi.fn(),
  itemDelete: vi.fn(),
  thumbnailUrl: vi.fn((id: string) => `thumb-${id}`),
}));
vi.mock('@/shared/api/endpoints', () => ({ api: apiMocks }));

import DuplicateManager from './DuplicateManager.vue';
import { useLibraryStore } from '@/domains/library';
import type { DuplicateEntry, DuplicateGroup, DuplicateScanResponse } from '@/shared/types';

function entry(id: string, path: string, distance = 1): DuplicateEntry {
  return {
    similarity_distance: distance,
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

function group(id: string, entries: DuplicateEntry[], kind: 'similar' | 'exact' = 'similar'): DuplicateGroup {
  return { id, kind, entries };
}

function response(groups: DuplicateGroup[], total = groups.length): DuplicateScanResponse {
  return { groups, total, offset: 0, limit: 50 };
}

async function flush(ticks = 20) {
  for (let i = 0; i < ticks; i++) {
    await Promise.resolve();
  }
}

function button(wrapper: VueWrapper, text: string) {
  const found = wrapper.findAll('button').find((candidate) => candidate.text() === text);
  expect(found, `按钮 ${text}`).toBeDefined();
  return found!;
}

async function clickButton(wrapper: VueWrapper, text: string) {
  await button(wrapper, text).trigger('click');
  await flush();
}

beforeEach(() => {
  setActivePinia(createPinia());
  apiMocks.itemDuplicates.mockReset();
  apiMocks.itemDelete.mockReset();
  apiMocks.itemDelete.mockResolvedValue(undefined);
  apiMocks.thumbnailUrl.mockClear();
});

describe('DuplicateManager', () => {
  it('默认不选中，展示元数据并仅删除选中位置', async () => {
    const groups = [group('g', [entry('a', 'folder/a.png'), entry('b', 'folder/b.png')])];
    apiMocks.itemDuplicates.mockResolvedValue(response(groups));
    const wrapper = mount(DuplicateManager);

    await clickButton(wrapper, '开始扫描');
    expect(wrapper.text()).toContain('1.0 KB');
    expect(wrapper.text()).toContain('1920 × 1080');
    expect(wrapper.text()).toContain('文件夹：folder');
    const checkboxes = wrapper.findAll('input[type="checkbox"]');
    expect(checkboxes).toHaveLength(2);
    expect(checkboxes.every((box) => !(box.element as HTMLInputElement).checked)).toBe(true);
    expect(wrapper.text()).toContain('已选择 0 个文件位置');

    await checkboxes[0].setValue(true);
    expect(wrapper.text()).toContain('已选择 1 个文件位置');
    await clickButton(wrapper, '移入回收站');
    expect(wrapper.find('.duplicate-confirm').exists()).toBe(true);
    await clickButton(wrapper, '确认移入回收站');

    expect(apiMocks.itemDelete).toHaveBeenCalledTimes(1);
    expect(apiMocks.itemDelete).toHaveBeenCalledWith('a', 'folder/a.png');
    expect(useLibraryStore().toast).toBe('已将 1 个文件位置移入回收站');
  });

  it('部分删除失败保留失败项选中并提示', async () => {
    apiMocks.itemDuplicates.mockResolvedValue(response([group('g', [entry('a', 'a.png')])]));
    apiMocks.itemDelete.mockRejectedValue(new Error('占用中'));
    const wrapper = mount(DuplicateManager);
    await clickButton(wrapper, '开始扫描');
    await wrapper.find('input[type="checkbox"]').setValue(true);
    await clickButton(wrapper, '移入回收站');
    await clickButton(wrapper, '确认移入回收站');

    expect(wrapper.text()).toContain('1 个失败');
    expect((wrapper.find('input[type="checkbox"]').element as HTMLInputElement).checked).toBe(true);
  });

  it('跨页保留选择并逐位置提交', async () => {
    apiMocks.itemDuplicates.mockImplementation(({ offset = 0 }: { offset?: number }) => {
      return Promise.resolve(offset === 0 ? response([group('g1', [entry('a', 'a.png')])], 51) : response([group('g2', [entry('b', 'b.png')])], 51));
    });
    const wrapper = mount(DuplicateManager);
    await clickButton(wrapper, '开始扫描');
    await wrapper.find('input[type="checkbox"]').setValue(true);
    await clickButton(wrapper, '下一页');
    await wrapper.find('input[type="checkbox"]').setValue(true);
    expect(wrapper.text()).toContain('已选择 2 个文件位置');

    await clickButton(wrapper, '移入回收站');
    await clickButton(wrapper, '确认移入回收站');
    expect(apiMocks.itemDelete).toHaveBeenCalledWith('a', 'a.png');
    expect(apiMocks.itemDelete).toHaveBeenCalledWith('b', 'b.png');
  });
});
