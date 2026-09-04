// 缓存父目录迁移（整体搬迁：先复制后删除）：停 server 解除文件锁 → 逐库子目录复制 →
// 全部成功后删旧内容 → 写配置 → 重启 server。任一环节失败不切换配置（旧目录保持完整）。
import fs from 'node:fs';
import { getLibraryRoot, readConfig, writeConfig } from './app-config';
import { cleanupPartial, defaultCacheParent, migrateDir, normalizeSlashes, validateCacheParent } from './cache-path';
import { getMainWindow } from './window';
import { openLibraryAt, stopServer } from './server';
import { IPC } from './ipc-contract';

/** 当前缓存父目录（未配置时为系统默认） */
export function currentCacheParent(): { current: string; isDefault: boolean } {
  const configured = readConfig().cacheParent;
  return configured
    ? { current: configured, isDefault: false }
    : { current: defaultCacheParent(process.platform, process.env), isDefault: true };
}

/** 迁移缓存父目录：返回 null 表示成功；返回错误文案表示失败（配置未切换，server 已按原路径重启） */
export async function changeCacheParent(newParent: string): Promise<string | null> {
  const libRoot = getLibraryRoot();
  if (!libRoot) {
    return '当前未打开素材库';
  }
  const { current } = currentCacheParent();
  const target = normalizeSlashes(newParent.trim());
  const invalid = validateCacheParent(target, current, libRoot, process.platform);
  if (invalid) {
    return invalid;
  }
  if (fs.existsSync(target) && fs.readdirSync(target).length > 0) {
    return '目标目录非空，请选择空目录或不存在的目录';
  }

  // 前端立即切启动屏（旧 server 即将停止，主界面 API 全部失效）
  getMainWindow()?.webContents.send(IPC.serverRestarting);
  stopServer();

  // 启动屏进度：migrate 伪帧（daemon 未运行，由主进程代发）
  const progress = (processed: number, total: number) =>
    getMainWindow()?.webContents.send(IPC.serverProgress, { phase: 'migrate', processed, total });

  try {
    migrateDir(current, target, progress);
  } catch (error) {
    // 搬迁失败：清理本次半成品，旧目录完整保留，按原配置重启
    cleanupPartial(target);
    await openLibraryAt(libRoot);
    return `迁移失败: ${error instanceof Error ? error.message : error}`;
  }

  writeConfig({ cacheParent: target });
  await openLibraryAt(libRoot);
  return null;
}
