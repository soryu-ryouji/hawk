// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import PairingCodeInput from './PairingCodeInput.vue';

describe('PairingCodeInput', () => {
  it('renders eight boxes, normalizes user input and emits submit on Enter', async () => {
    const wrapper = mount(PairingCodeInput, { props: { modelValue: '' } });
    expect(wrapper.findAll('.code-box')).toHaveLength(8);

    const input = wrapper.get('input');
    (input.element as HTMLInputElement).value = 'ab-12cd34';
    await input.trigger('input');
    expect(wrapper.emitted('update:modelValue')?.at(-1)).toEqual(['AB12CD34']);

    await input.trigger('keydown', { key: 'Enter' });
    expect(wrapper.emitted('submit')).toHaveLength(1);
  });

  it('renders readonly code without a native input', () => {
    const wrapper = mount(PairingCodeInput, { props: { modelValue: 'ABCD2345', readonly: true } });
    expect(wrapper.find('input').exists()).toBe(false);
    expect(wrapper.findAll('.code-box').map((box) => box.text())).toEqual(['A', 'B', 'C', 'D', '2', '3', '4', '5']);
  });
});
