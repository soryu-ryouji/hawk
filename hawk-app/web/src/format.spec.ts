// format.ts 的单元测试：单位分档边界与精度。
import { describe, expect, it } from 'vitest';
import { formatSize } from './format';

describe('formatSize', () => {
  it('单位随量级自适应：B / KB / MB / GB', () => {
    expect(formatSize(0)).toBe('0 B');
    expect(formatSize(512)).toBe('512 B');
    expect(formatSize(1024)).toBe('1.0 KB');
    expect(formatSize(1024 * 1024)).toBe('1.00 MB');
    expect(formatSize(1024 ** 3)).toBe('1.00 GB');
    expect(formatSize(2.5 * 1024 ** 3)).toBe('2.50 GB');
  });

  it('分档边界：恰好不到上一级时不进位', () => {
    expect(formatSize(1024 ** 2 - 1)).toBe('1024.0 KB');
    expect(formatSize(1024 ** 3 - 1)).toBe('1024.00 MB');
  });
});
