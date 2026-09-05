// @vitest-environment jsdom
// ItemCard 长按检测（移动端条目菜单入口）：触屏按住 500ms 触发 menu、移动/抬手/鼠标不触发、
// 长按后松手的首个 click 被吞（防触屏单击语义在菜单背后再触发）。
// jsdom 无 PointerEvent：用 MouseEvent 补 pointerType 属性构造。
import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';

// api 层整体 mock（组件测试只关心交互分支）
vi.mock('../api/endpoints', () => ({ api: { thumbnailUrl: () => 'thumb', fileUrl: () => 'file' } }));

import ItemCard from './ItemCard.vue';
import type { Item } from '../types';

const item = {
  id: 'a1',
  path: 'a1',
  name: 'img',
  ext: 'jpg',
  star: 0,
  width: 100,
  height: 100,
} as unknown as Item;

/** jsdom 无 PointerEvent：MouseEvent 补 pointerType */
function pointer(type: string, pointerType: string, x = 0, y = 0): PointerEvent {
  const e = new MouseEvent(type, { bubbles: true, cancelable: true, clientX: x, clientY: y });
  Object.defineProperty(e, 'pointerType', { value: pointerType });
  return e as unknown as PointerEvent;
}

function mountCard() {
  return mount(ItemCard, { props: { item, selected: false } });
}

async function pressAndHold(wrapper: ReturnType<typeof mountCard>, pointerType = 'touch') {
  const card = wrapper.find('.card');
  card.element.dispatchEvent(pointer('pointerdown', pointerType, 10, 10));
  await wrapper.vm.$nextTick();
  vi.advanceTimersByTime(500);
  await wrapper.vm.$nextTick();
}

describe('ItemCard 长按', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    setActivePinia(createPinia());
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('触屏按住 500ms 触发 menu（携带 item 与事件）', async () => {
    const wrapper = mountCard();
    await pressAndHold(wrapper);
    const events = wrapper.emitted('menu') as unknown as Array<[Item, PointerEvent]> | undefined;
    expect(events).toHaveLength(1);
    expect(events![0][0]).toStrictEqual(item);
    expect(events![0][1].clientX).toBe(10);
  });

  it('移动超阈值取消（滚动/滑动意图优先）', async () => {
    const wrapper = mountCard();
    const card = wrapper.find('.card');
    card.element.dispatchEvent(pointer('pointerdown', 'touch', 10, 10));
    await wrapper.vm.$nextTick();
    card.element.dispatchEvent(pointer('pointermove', 'touch', 40, 10));
    vi.advanceTimersByTime(500);
    await wrapper.vm.$nextTick();
    expect(wrapper.emitted('menu')).toBeUndefined();
  });

  it('500ms 内抬手不触发', async () => {
    const wrapper = mountCard();
    const card = wrapper.find('.card');
    card.element.dispatchEvent(pointer('pointerdown', 'touch', 10, 10));
    vi.advanceTimersByTime(100);
    card.element.dispatchEvent(pointer('pointerup', 'touch', 10, 10));
    vi.advanceTimersByTime(500);
    await wrapper.vm.$nextTick();
    expect(wrapper.emitted('menu')).toBeUndefined();
  });

  it('鼠标不参与长按（右键走 contextmenu）', async () => {
    const wrapper = mountCard();
    await pressAndHold(wrapper, 'mouse');
    expect(wrapper.emitted('menu')).toBeUndefined();
  });

  it('长按触发后松手的首个 click 被吞，后续 click 正常', async () => {
    const wrapper = mountCard();
    await pressAndHold(wrapper);
    const card = wrapper.find('.card');
    card.element.dispatchEvent(pointer('pointerup', 'touch', 10, 10));
    card.element.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    await wrapper.vm.$nextTick();
    expect(wrapper.emitted('select')).toBeUndefined();
    card.element.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    await wrapper.vm.$nextTick();
    expect(wrapper.emitted('select')).toHaveLength(1);
  });
});
