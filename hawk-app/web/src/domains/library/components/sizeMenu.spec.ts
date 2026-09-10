// @vitest-environment jsdom
// SizeMenu 组件测试：区间输入、档位按聚焦框填充、模式下拉切换（短边 ↔ 宽高）、校验与回显、× 清除。
// SizeMenu 不依赖 store（props 入 / select 事件出），可直接挂载
import { afterEach, describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import SizeMenu from './SizeMenu.vue';
import type { SizeFilter } from '@/shared/types';

// 面板打开状态下 unmount 会残留 Teleport 到 body 的 DOM，污染后续测试的 document 查询
afterEach(() => {
  document.body.innerHTML = '';
});

/** 挂载并打开面板（chip 点击） */
async function openMenu(props: { size?: SizeFilter } = {}) {
  const wrapper = mount(SizeMenu, { props, attachTo: document.body });
  await wrapper.find('.chip .trigger').trigger('click');
  return wrapper;
}

/** Teleport 到 body 的面板节点 */
function panel(): HTMLElement {
  const el = document.querySelector('.panel');
  expect(el, '面板应已展开').toBeTruthy();
  return el as HTMLElement;
}

/** 面板直系档位项 */
function tiers(): HTMLButtonElement[] {
  return [...panel().querySelectorAll<HTMLButtonElement>(':scope > .item')];
}

function inputs(scope: string): HTMLInputElement[] {
  return [...panel().querySelectorAll<HTMLInputElement>(`${scope} input`)];
}

function setInput(el: HTMLInputElement, value: string) {
  el.value = value;
  el.dispatchEvent(new Event('input', { bubbles: true }));
}

function clickTier(text: string) {
  tiers()
    .find((e) => e.textContent?.includes(text))!
    .dispatchEvent(new MouseEvent('click', { bubbles: true }));
}

function focus(el: HTMLInputElement) {
  el.dispatchEvent(new Event('focus', { bubbles: true }));
}

describe('SizeMenu', () => {
  it('chip 文案：短边 ≥/≤/区间（档位 p 记法、自定义 px）；宽高按边组合', async () => {
    const wrapper = mount(SizeMenu, { props: {} });
    expect(wrapper.find('.chip .trigger').text()).toBe('尺寸');
    await wrapper.setProps({ size: { kind: 'side', min: 1080 } });
    expect(wrapper.find('.chip .trigger').text()).toBe('≥ 1080p');
    await wrapper.setProps({ size: { kind: 'side', min: 900 } });
    expect(wrapper.find('.chip .trigger').text()).toBe('≥ 900px');
    await wrapper.setProps({ size: { kind: 'side', max: 720 } });
    expect(wrapper.find('.chip .trigger').text()).toBe('≤ 720p');
    await wrapper.setProps({ size: { kind: 'side', min: 720, max: 1080 } });
    expect(wrapper.find('.chip .trigger').text()).toBe('720p – 1080p');
    await wrapper.setProps({ size: { kind: 'wh', minWidth: 1920, minHeight: 1080 } });
    expect(wrapper.find('.chip .trigger').text()).toBe('1920+ × 1080+');
    await wrapper.setProps({ size: { kind: 'wh', minWidth: 1280, maxWidth: 1920, minHeight: 720, maxHeight: 1080 } });
    expect(wrapper.find('.chip .trigger').text()).toBe('1280–1920 × 720–1080');
    await wrapper.setProps({ size: { kind: 'wh', minWidth: 500 } });
    expect(wrapper.find('.chip .trigger').text()).toBe('宽 500+');
  });

  it('短边模式默认面板：区间两框 + 档位列表 + 模式按钮；无「全部尺寸」；当前条件回显', async () => {
    const wrapper = await openMenu({ size: { kind: 'side', min: 720, max: 1080 } });
    const p = panel();
    const labels = [...p.querySelectorAll(':scope > .item .label')].map((e) => e.textContent);
    expect(labels).toEqual(['不限', '480p', '720p', '1080p', '1440p', '2160p', '4320p']);
    const [min, max] = inputs('.range:not(.wh)');
    expect(min.value).toBe('720');
    expect(max.value).toBe('1080');
    // 勾随匹配侧着色：下限 720 蓝勾、上限 1080 红勾
    expect(p.querySelector('.item.checked-min .label')?.textContent).toBe('720p');
    expect(p.querySelector('.item.checked-max .label')?.textContent).toBe('1080p');
    expect(p.querySelector('.mode-btn span:not(.caret)')?.textContent).toBe('按短边区间');
    wrapper.unmount();
  });

  it('档位填入提示：默认最小，聚焦最大框后变为最大', async () => {
    const wrapper = await openMenu({ size: { kind: 'side', min: 720 } });
    const p = panel();
    expect(p.querySelector('.tier-hint')?.textContent).toBe('档位填入：最小');
    expect(p.querySelector('.tier-hint')?.classList.contains('to-min')).toBe(true);
    const [, max] = inputs('.range:not(.wh)');
    focus(max);
    await Promise.resolve();
    expect(p.querySelector('.tier-hint')?.textContent).toBe('档位填入：最大');
    expect(p.querySelector('.tier-hint')?.classList.contains('to-max')).toBe(true);
    wrapper.unmount();
  });

  it('档位点击（无聚焦）：填入短边下限（上限保留）立即生效，面板不关', async () => {
    const wrapper = await openMenu({ size: { kind: 'side', max: 1080 } });
    clickTier('720p');
    await Promise.resolve();
    expect(wrapper.emitted('select')).toEqual([[{ kind: 'side', min: 720, max: 1080 }]]);
    expect(document.querySelector('.panel')).not.toBeNull(); // 面板保持打开
    wrapper.unmount();
  });

  it('档位点击（聚焦最大框）：填入上限而非下限', async () => {
    const wrapper = await openMenu({ size: { kind: 'side', min: 720 } });
    const [, max] = inputs('.range:not(.wh)');
    focus(max);
    clickTier('1440');
    await Promise.resolve();
    expect(wrapper.emitted('select')).toEqual([[{ kind: 'side', min: 720, max: 1440 }]]);
    expect(max.value).toBe('1440');
    wrapper.unmount();
  });

  it('区间输入应用：合法发出 side 区间；全空 = 清除；倒挂禁用', async () => {
    const wrapper = await openMenu({ size: { kind: 'side', min: 720 } });
    const [min, max] = inputs('.range:not(.wh)');
    const apply = () => panel().querySelector<HTMLButtonElement>('.range:not(.wh) .apply')!;
    // 倒挂禁用
    setInput(min, '1080');
    setInput(max, '720');
    await Promise.resolve();
    expect(apply().disabled).toBe(true);
    // 合法区间
    setInput(min, '720');
    await Promise.resolve();
    expect(apply().disabled).toBe(false);
    apply().dispatchEvent(new MouseEvent('click', { bubbles: true }));
    await Promise.resolve();
    expect(wrapper.emitted('select')).toEqual([[{ kind: 'side', min: 720, max: 720 }]]);
    expect(document.querySelector('.panel')).toBeNull();
    // 全空应用 = 清除（不再要求至少一框）
    const wrapper2 = await openMenu({ size: { kind: 'side', min: 720 } });
    const [min2] = inputs('.range:not(.wh)');
    setInput(min2, '');
    await Promise.resolve();
    const apply2 = () => panel().querySelector<HTMLButtonElement>('.range:not(.wh) .apply')!;
    expect(apply2().disabled).toBe(false);
    apply2().dispatchEvent(new MouseEvent('click', { bubbles: true }));
    await Promise.resolve();
    expect(wrapper2.emitted('select')).toEqual([[undefined]]);
    wrapper2.unmount();
    wrapper.unmount();
  });

  it('「不限」：side 清除整个区间；wh 只清目标轴（另一轴保留）；勾随目标轴无限制态', async () => {
    // side：点不限 → 清除；勾随无限制态（父组件回写后）
    const wrapper = await openMenu({ size: { kind: 'side', min: 720 } });
    const p = panel();
    expect(p.querySelector('.item.checked-unlimited .label')?.textContent).not.toBe('不限');
    clickTier('不限');
    await Promise.resolve();
    expect(wrapper.emitted('select')).toEqual([[undefined]]);
    await wrapper.setProps({ size: undefined });
    expect(p.querySelector('.item.checked-unlimited .label')?.textContent).toBe('不限');
    wrapper.unmount();
    // wh：宽已设，聚焦高轴点不限 → 高清空、宽保留
    const wrapper2 = await openMenu({ size: { kind: 'wh', minWidth: 1440, minHeight: 720 } });
    const wh = inputs('.range.wh');
    focus(wh[2]); // 高 · 最小
    clickTier('不限');
    await Promise.resolve();
    expect(wrapper2.emitted('select')).toEqual([[{ kind: 'wh', minWidth: 1440, maxWidth: undefined, minHeight: undefined, maxHeight: undefined }]]);
    // 高轴无限制后「不限」勾选（父组件回写后）
    await wrapper2.setProps({ size: { kind: 'wh', minWidth: 1440 } });
    expect(panel().querySelector('.item.checked-unlimited .label')?.textContent).toBe('不限');
    wrapper2.unmount();
  });

  it('模式下拉切换到宽高：四框输入且档位列表保留；档位填到聚焦的宽/高框', async () => {
    const wrapper = await openMenu();
    const p = panel();
    p.querySelector<HTMLButtonElement>('.mode-btn')!.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    await Promise.resolve();
    const opts = [...p.querySelectorAll<HTMLButtonElement>('.mode-list button')];
    expect(opts.map((o) => o.textContent)).toEqual(['按短边区间', '按宽高区间']);
    opts[1].dispatchEvent(new MouseEvent('click', { bubbles: true }));
    await Promise.resolve();
    // 宽高模式：档位列表保留（含不限首项）
    expect(tiers()).toHaveLength(7);
    const wh = inputs('.range.wh');
    expect(wh).toHaveLength(4);
    // 聚焦「高-最小」框后点档位 1080p → 填入高度下限
    focus(wh[2]);
    clickTier('1080p');
    await Promise.resolve();
    expect(wh[2].value).toBe('1080');
    expect(wrapper.emitted('select')).toEqual([[{ kind: 'wh', minWidth: undefined, maxWidth: undefined, minHeight: 1080, maxHeight: undefined }]]);
    // 再聚焦「宽-最大」点 2160p → 宽度上限
    focus(wh[1]);
    clickTier('2160p');
    await Promise.resolve();
    expect(wh[1].value).toBe('2160');
    expect(wrapper.emitted('select')!.at(-1)).toEqual([{ kind: 'wh', minWidth: undefined, maxWidth: 2160, minHeight: 1080, maxHeight: undefined }]);
    wrapper.unmount();
  });

  it('wh 条件重新打开：模式继承宽高并回显；档位勾选匹配任一框值', async () => {
    const wrapper = await openMenu({ size: { kind: 'wh', minWidth: 1920, maxHeight: 1080 } });
    const p = panel();
    expect(p.querySelector('.mode-btn span:not(.caret)')?.textContent).toBe('按宽高区间');
    const wh = inputs('.range.wh');
    expect(wh[0].value).toBe('1920');
    expect(wh[3].value).toBe('1080');
    // 1920 非档位值；高度上限 1080 为红勾 + 徽章「高 ≤」（轴橙底）
    expect(p.querySelector('.item.checked-max .label')?.textContent).toBe('1080p');
    expect(p.querySelector('.item.checked-max .badge')?.textContent).toBe('高 ≤');
    expect(p.querySelector('.item.checked-max .badge')?.classList.contains('b-h')).toBe(true);
    expect(p.querySelector('.item.checked-min')).toBeNull();
    wrapper.unmount();
  });

  it('chip 的 × 发出 select(undefined)', async () => {
    const wrapper = mount(SizeMenu, { props: { size: { kind: 'side', min: 480 } } });
    await wrapper.find('.chip .clear').trigger('click');
    expect(wrapper.emitted('select')).toEqual([[undefined]]);
    expect(document.querySelector('.panel')).toBeNull();
  });
});
