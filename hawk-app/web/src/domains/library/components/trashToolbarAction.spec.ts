// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { mount } from '@vue/test-utils';
import { nextTick, ref } from 'vue';

const apiMocks = vi.hoisted(() => ({
  trashClear: vi.fn((): Promise<void> => Promise.resolve()),
}));
const layout = vi.hoisted(() => ({ narrow: false }));
vi.mock('@/shared/api/endpoints', () => ({ api: apiMocks }));
vi.mock('@/shared/composables/useLayout', () => ({
  useLayout: () => ({ narrow: ref(layout.narrow), touch: ref(false) }),
}));

import TrashToolbarAction from './TrashToolbarAction.vue';
import { useLibraryStore } from '@/domains/library';

beforeEach(() => {
  setActivePinia(createPinia());
  layout.narrow = false;
  apiMocks.trashClear.mockClear();
});

afterEach(() => {
  document.body.innerHTML = '';
});

function dialogButton(text: string): HTMLButtonElement {
  return [...document.body.querySelectorAll<HTMLButtonElement>('[role="dialog"] button')].find((button) => button.textContent === text)!;
}

describe('回收站顶栏操作', () => {
  it('仅在非空且可编辑时显示，窄屏使用短文案', async () => {
    const store = useLibraryStore();
    const wrapper = mount(TrashToolbarAction);
    expect(wrapper.find('button').exists()).toBe(false);

    store.skeleton = [{ id: 'a', path: 'a.jpg', size: 1, star: 0, width: 100, height: 100 }];
    await nextTick();
    expect(wrapper.find('button').text()).toBe('清空回收站');

    layout.narrow = true;
    const narrowWrapper = mount(TrashToolbarAction);
    expect(narrowWrapper.find('button').text()).toBe('清空');
    narrowWrapper.unmount();

    store.viewerMode = true;
    await nextTick();
    expect(wrapper.find('button').exists()).toBe(false);
    wrapper.unmount();
  });

  it('应用内对话框取消不删除，确认后才清空回收站', async () => {
    const store = useLibraryStore();
    store.view = { kind: 'trash' };
    store.skeleton = [{ id: 'a', path: 'a.jpg', size: 1, star: 0, width: 100, height: 100 }];
    const wrapper = mount(TrashToolbarAction, { attachTo: document.body });

    await wrapper.find('.trash-clear').trigger('click');
    expect(document.body.querySelector('[role="dialog"]')?.textContent).toContain('1 项素材将被彻底删除');
    expect(apiMocks.trashClear).not.toHaveBeenCalled();

    dialogButton('取消').click();
    await nextTick();
    expect(document.body.querySelector('[role="dialog"]')).toBeNull();

    await wrapper.find('.trash-clear').trigger('click');
    dialogButton('确认清空').click();
    await vi.waitFor(() => expect(apiMocks.trashClear).toHaveBeenCalledTimes(1));
    expect(store.toast).toBe('回收站已清空');
    expect(document.body.querySelector('[role="dialog"]')).toBeNull();
    wrapper.unmount();
  });
});
