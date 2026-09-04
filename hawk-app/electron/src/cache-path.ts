// 缓存父目录的路径规则与搬迁原语（纯函数/纯 fs，不引 electron；vitest 覆盖）。
// 与 hawk-daemon 的 core/paths.rs 对齐：默认 <系统缓存>/hawk/cache，库子目录 <库名>_<哈希16位> 拼接其下。
import fs from 'node:fs';
import path from 'node:path';

/** 统一分隔符并去尾斜杠（存储与拼接用；不改变大小写） */
export function normalizeSlashes(p: string): string {
  return path.posix.normalize(p.replace(/\\/g, '/')).replace(/\/+$/, '');
}

/** 包含关系比较键（win32/darwin 默认大小写不敏感） */
export function pathKey(p: string, platform: string): string {
  const n = normalizeSlashes(p);
  return platform === 'linux' ? n : n.toLowerCase();
}

/** 平台默认缓存父目录（<系统缓存>/hawk/cache，与 daemon 的 default_cache_parent 对齐） */
export function defaultCacheParent(platform: string, env: NodeJS.ProcessEnv): string {
  if (platform === 'win32') {
    const base = env.LOCALAPPDATA || `${env.USERPROFILE || ''}/AppData/Local`;
    return `${normalizeSlashes(base)}/hawk/cache`;
  }
  if (platform === 'darwin') {
    return `${normalizeSlashes(env.HOME || '')}/Library/Application Support/hawk/cache`;
  }
  return `${normalizeSlashes(env.XDG_DATA_HOME || `${env.HOME || ''}/.local/share`)}/hawk/cache`;
}

/** 新缓存父目录校验：返回错误文案或 null（合法）。
 *  只拦「缓存在库内」（必然污染索引）；「库在缓存父目录内」无害（拼接的库子目录与库同级），
 *  与 daemon 的 cache_location_error（拼接后判定）语义对齐 */
export function validateCacheParent(
  newParent: string,
  currentParent: string,
  libraryRoot: string | null,
  platform: string,
): string | null {
  const target = normalizeSlashes(newParent.trim());
  if (!target || target === '.') {
    return '请选择缓存目录';
  }
  if (pathKey(target, platform) === pathKey(currentParent, platform)) {
    return '新目录与当前缓存目录相同';
  }
  if (libraryRoot) {
    const rk = pathKey(libraryRoot, platform);
    const tk = pathKey(target, platform);
    if (tk === rk || tk.startsWith(`${rk}/`)) {
      return '缓存目录不能位于素材库内';
    }
  }
  return null;
}

/** 整体搬迁：逐子目录复制到新父目录，全部成功后删除旧内容（先复制后删除，跨盘安全）。
 *  复制中途失败抛错（半成品由 cleanupPartial 清理）；from 不存在时只建目标（从未产生缓存） */
export function migrateDir(from: string, to: string, progress: (processed: number, total: number) => void): void {
  fs.mkdirSync(to, { recursive: true });
  if (!fs.existsSync(from)) {
    return;
  }
  const entries = fs.readdirSync(from, { withFileTypes: true }).filter((e) => e.isDirectory());
  progress(0, entries.length);
  entries.forEach((entry, i) => {
    fs.cpSync(path.join(from, entry.name), path.join(to, entry.name), { recursive: true });
    progress(i + 1, entries.length);
  });
  for (const entry of entries) {
    fs.rmSync(path.join(from, entry.name), { recursive: true, force: true });
  }
}

/** 清理搬迁失败在新目录留下的半成品（调用前目标已验证为空，目录内容均来自本次搬迁） */
export function cleanupPartial(dir: string): void {
  if (!fs.existsSync(dir)) {
    return;
  }
  for (const entry of fs.readdirSync(dir)) {
    fs.rmSync(path.join(dir, entry), { recursive: true, force: true });
  }
}
