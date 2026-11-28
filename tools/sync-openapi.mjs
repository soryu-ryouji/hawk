// 把契约单一来源 hawk-daemon/openapi.json 同步到 Go 的 embed 副本。
// 用法：node tools/sync-openapi.mjs
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const source = path.join(root, 'hawk-daemon', 'openapi.json');
const target = path.join(root, 'hawk-daemon', 'internal', 'api', 'openapi.json');
fs.copyFileSync(source, target);
console.log(`已同步 ${target}`);
