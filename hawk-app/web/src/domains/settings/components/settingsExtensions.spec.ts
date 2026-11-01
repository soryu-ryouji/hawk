// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';

const apiMocks = vi.hoisted(() => ({
  visionStatus: vi.fn(),
  visionInstall: vi.fn(),
}));
vi.mock('@/shared/api/endpoints', () => ({ api: apiMocks }));

import SettingsExtensions from './SettingsExtensions.vue';

const notInstalled = {
  installed: false,
  enabled: false,
  downloading: false,
  downloaded_bytes: 0,
  total_bytes: 24446700,
  indexed_items: 0,
  total_items: 0,
  pending_items: 0,
  indexing: false,
  error: null,
};
const installed = { ...notInstalled, installed: true, enabled: true };

beforeEach(() => {
  apiMocks.visionStatus.mockReset();
  apiMocks.visionInstall.mockReset();
  apiMocks.visionStatus.mockResolvedValue(notInstalled);
  apiMocks.visionInstall.mockResolvedValue(installed);
});

describe('视觉扩展设置', () => {
  it('仅在用户主动点击后下载并启用模型', async () => {
    const wrapper = mount(SettingsExtensions);
    await Promise.resolve();
    expect(apiMocks.visionInstall).not.toHaveBeenCalled();

    await wrapper.find('button').trigger('click');
    await Promise.resolve();
    expect(apiMocks.visionInstall).toHaveBeenCalledTimes(1);
    await vi.waitFor(() => expect(wrapper.text()).toContain('已索引'));
  });
});
