// @vitest-environment jsdom
// 侧栏维度行组件的渲染测试：隐藏标记、右键菜单条目、v-if/v-else 链回归
//（FolderTreeNode 名称与重命名输入框的互斥曾因插入图标被破坏，此处固化）。
// api 层整体 mock（组件测试只关心交互分支）；stores 用真实 pinia 预置状态。
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';

// jsdom 无 matchMedia；library store 浏览器端分支（useMediaQuery）需要
window.matchMedia =
  window.matchMedia ??
  ((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addListener() {},
    removeListener() {},
    addEventListener() {},
    removeEventListener() {},
    dispatchEvent: () => false,
  }) as MediaQueryList);

const mocks = vi.hoisted(() => ({
  globalFilterSet: vi.fn(() => Promise.resolve()),
  globalFilterList: vi.fn(() => Promise.resolve({ folders: [], categories: [], tags: [] })),
}));
vi.mock('../api/endpoints', () => ({ api: mocks }));
const { globalFilterSet } = mocks;

import FolderTreeNode from './FolderTreeNode.vue';
import TaxonomyRow from './TaxonomyRow.vue';
import { useContextMenu } from '../composables/useContextMenu';
import { useLibraryStore } from '../stores/library';
import { useTaxonomyStore } from '../stores/taxonomy';
import type { FolderNode } from '../types';

function makeNode(overrides?: Partial<FolderNode>): FolderNode {
  return { path: '素材堆', name: '素材堆', children: [], modification_time: 0, count: 3, ...overrides };
}

/** 打开右键菜单并返回菜单项（模块级单例，直接读 state） */
async function openMenu(wrapper: ReturnType<typeof mount>, selector: string) {
  await wrapper.find(selector).trigger('contextmenu');
  return useContextMenu().state.items.filter((i) => !i.separator);
}

beforeEach(() => {
  setActivePinia(createPinia());
  // 菜单为模块级单例：重置 items，避免上一用例残留污染断言
  useContextMenu().open([], new MouseEvent('contextmenu'));
  useContextMenu().close();
  globalFilterSet.mockClear();
  mocks.globalFilterList.mockClear();
});

describe('TaxonomyRow', () => {
  function mountRow(props: { kind: 'category' | 'tag'; name: string }) {
    return mount(TaxonomyRow, {
      props: { count: 5, active: false, dropTarget: false, ...props },
    });
  }

  it('未隐藏时不渲染隐藏标记', () => {
    const wrapper = mountRow({ kind: 'category', name: '海报' });
    expect(wrapper.find('.tax-hidden').exists()).toBe(false);
    expect(wrapper.text()).toContain('海报');
  });

  it('已隐藏时显示 eyeOff 标记', () => {
    const taxonomy = useTaxonomyStore();
    taxonomy.globalFilter = { folders: [], categories: ['海报'], tags: [] };
    const wrapper = mountRow({ kind: 'category', name: '海报' });
    expect(wrapper.find('.tax-hidden').exists()).toBe(true);
  });

  it('右键菜单按隐藏态切换文案，动作调用 globalFilterSet', async () => {
    const wrapper = mountRow({ kind: 'tag', name: '量大' });
    const items = await openMenu(wrapper, '.tax-row');
    const hide = items.find((i) => i.label === '不在全局列表显示');
    expect(hide).toBeTruthy();
    hide!.action!();
    expect(globalFilterSet).toHaveBeenCalledWith('tag', '量大', true);

    const taxonomy = useTaxonomyStore();
    taxonomy.globalFilter = { folders: [], categories: [], tags: ['量大'] };
    const items2 = await openMenu(wrapper, '.tax-row');
    expect(items2.some((i) => i.label === '恢复在全局列表显示')).toBe(true);
  });

  it('只读 viewer 不出右键菜单', async () => {
    useLibraryStore().viewerMode = true;
    const wrapper = mountRow({ kind: 'category', name: '海报' });
    const items = await openMenu(wrapper, '.tax-row');
    expect(items).toEqual([]);
  });
});

describe('FolderTreeNode', () => {
  function mountNode(node: FolderNode) {
    return mount(FolderTreeNode, { props: { node, depth: 0 } });
  }

  it('非重命名态：显示名称与计数，不渲染重命名输入框（v-if/v-else 回归）', () => {
    const wrapper = mountNode(makeNode());
    expect(wrapper.find('.name').exists()).toBe(true);
    expect(wrapper.find('input.edit').exists()).toBe(false);
    expect(wrapper.find('.node-hidden').exists()).toBe(false);
  });

  it('已隐藏文件夹显示 eyeOff 标记，且仍不渲染输入框', () => {
    const taxonomy = useTaxonomyStore();
    taxonomy.globalFilter = { folders: ['素材堆'], categories: [], tags: [] };
    const wrapper = mountNode(makeNode());
    expect(wrapper.find('.node-hidden').exists()).toBe(true);
    expect(wrapper.find('input.edit').exists()).toBe(false);
  });

  it('重命名态：输入框替换名称（链的另一支）', async () => {
    const wrapper = mountNode(makeNode());
    const items = await openMenu(wrapper, '.node');
    items.find((i) => i.label === '重命名')!.action!();
    await wrapper.vm.$nextTick();
    expect(wrapper.find('input.edit').exists()).toBe(true);
    expect(wrapper.find('.name').exists()).toBe(false);
  });

  it('右键菜单隐藏项动作调用 globalFilterSet(folder, path)', async () => {
    const wrapper = mountNode(makeNode());
    const items = await openMenu(wrapper, '.node');
    const hide = items.find((i) => i.label === '不在全局列表显示');
    expect(hide).toBeTruthy();
    hide!.action!();
    expect(globalFilterSet).toHaveBeenCalledWith('folder', '素材堆', true);
  });
});
