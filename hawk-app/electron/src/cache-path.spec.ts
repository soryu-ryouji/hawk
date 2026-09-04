// 缓存父目录路径规则：分隔符归一、大小写敏感性、默认父目录、包含校验、搬迁原语
import { mkdtempSync, existsSync, mkdirSync, readdirSync, rmSync, writeFileSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { describe, expect, it, afterEach } from 'vitest';
import { cleanupPartial, defaultCacheParent, migrateDir, normalizeSlashes, pathKey, validateCacheParent } from './cache-path';

describe('normalizeSlashes', () => {
  it('统一分隔符并去尾斜杠', () => {
    expect(normalizeSlashes('D:\\cache\\hawk\\')).toBe('D:/cache/hawk');
    expect(normalizeSlashes('/data/cache/')).toBe('/data/cache');
  });
});

describe('pathKey', () => {
  it('win32 大小写不敏感', () => {
    expect(pathKey('D:/Cache', 'win32')).toBe(pathKey('d:/cache/', 'win32'));
  });
  it('linux 大小写敏感', () => {
    expect(pathKey('/Data/Cache', 'linux')).not.toBe(pathKey('/data/cache', 'linux'));
  });
});

describe('defaultCacheParent', () => {
  it('win32 走 LOCALAPPDATA', () => {
    expect(defaultCacheParent('win32', { LOCALAPPDATA: 'C:\\Users\\u\\AppData\\Local' } as NodeJS.ProcessEnv)).toBe(
      'C:/Users/u/AppData/Local/hawk/cache',
    );
  });
  it('macOS / linux 平台路径', () => {
    expect(defaultCacheParent('darwin', { HOME: '/Users/u' } as NodeJS.ProcessEnv)).toBe(
      '/Users/u/Library/Application Support/hawk/cache',
    );
    expect(defaultCacheParent('linux', { HOME: '/home/u' } as NodeJS.ProcessEnv)).toBe('/home/u/.local/share/hawk/cache');
    expect(defaultCacheParent('linux', { XDG_DATA_HOME: '/xdg' } as NodeJS.ProcessEnv)).toBe('/xdg/hawk/cache');
  });
});

describe('validateCacheParent', () => {
  const current = 'C:/Users/u/AppData/Local/hawk/cache';

  it('合法路径返回 null', () => {
    expect(validateCacheParent('D:/hawk-cache', current, 'D:/Materials', 'win32')).toBeNull();
  });

  it('空值与当前目录相同被拒', () => {
    expect(validateCacheParent('  ', current, 'D:/Materials', 'win32')).toContain('请选择');
    expect(validateCacheParent(current + '/', current, 'D:/Materials', 'win32')).toContain('相同');
  });

  it('缓存落在库内被拒（含大小写与分隔符变体）', () => {
    expect(validateCacheParent('D:/Materials/.cache', current, 'D:/Materials', 'win32')).toContain('素材库内');
    expect(validateCacheParent('d:/materials\\cache', current, 'D:/Materials', 'win32')).toContain('素材库内');
  });

  it('库在缓存父目录内合法（拼接的库子目录与库同级，无包含）', () => {
    expect(validateCacheParent('D:/', current, 'D:/Materials', 'win32')).toBeNull();
  });

  it('linux 下大小写不同的同名路径不判包含', () => {
    expect(validateCacheParent('/data/Materials/cache', '/cache', '/data/materials', 'linux')).toBeNull();
  });
});

// 搬迁原语用真实临时目录（系统 tmp）验证：复制完整性、先复制后删除、半成品清理
const tmpRoots: string[] = [];
function tmpRoot(): string {
  const dir = mkdtempSync(path.join(tmpdir(), 'hawk-cache-test-'));
  tmpRoots.push(dir);
  return dir;
}
afterEach(() => {
  while (tmpRoots.length) {
    rmSync(tmpRoots.pop()!, { recursive: true, force: true });
  }
});

function seedLibraryCache(parent: string, name: string, files: Record<string, string>): void {
  const dir = path.join(parent, name);
  for (const [rel, content] of Object.entries(files)) {
    const abs = path.join(dir, rel);
    mkdirSync(path.dirname(abs), { recursive: true });
    writeFileSync(abs, content);
  }
}

describe('migrateDir', () => {
  it('逐子目录复制后删除旧内容，进度帧递增', () => {
    const base = tmpRoot();
    const from = path.join(base, 'old');
    const to = path.join(base, 'new');
    seedLibraryCache(from, 'lib_abc123', { 'index.db': 'sqlite', 'thumbnails/x.webp': 'img' });
    seedLibraryCache(from, 'photo_def456', { 'index.db': 'sqlite2' });

    const frames: Array<[number, number]> = [];
    migrateDir(from, to, (p, t) => frames.push([p, t]));

    expect(readFileSync(path.join(to, 'lib_abc123/index.db'), 'utf8')).toBe('sqlite');
    expect(readFileSync(path.join(to, 'lib_abc123/thumbnails/x.webp'), 'utf8')).toBe('img');
    expect(readFileSync(path.join(to, 'photo_def456/index.db'), 'utf8')).toBe('sqlite2');
    expect(readdirSync(from)).toEqual([]); // 旧内容已删
    expect(frames).toEqual([
      [0, 2],
      [1, 2],
      [2, 2],
    ]);
  });

  it('旧目录不存在时只建目标目录', () => {
    const base = tmpRoot();
    const to = path.join(base, 'new');
    migrateDir(path.join(base, 'nonexistent'), to, () => {});
    expect(existsSync(to)).toBe(true);
  });
});

describe('cleanupPartial', () => {
  it('清空半成品目录；目录不存在时为空操作', () => {
    const base = tmpRoot();
    const partial = path.join(base, 'partial');
    seedLibraryCache(partial, 'lib_abc123', { 'index.db': 'x' });
    cleanupPartial(partial);
    expect(readdirSync(partial)).toEqual([]);
    cleanupPartial(path.join(base, 'nonexistent')); // 不抛错
  });
});
