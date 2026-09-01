// extraResources 平台隔离回归验证：npm run test:resources
// 直接调用 electron-builder 的配置解析（getFileMatchers，即打包时决定复制哪些文件的真实代码路径），
// 断言 hawk-update 只进 Windows 产物、mac/linux 只带 hawk-daemon。
// 背景：平台级 extraResources 与顶层是追加合并非覆盖，靠「hawk-update 只写在 win.extraResources」隔离，
// 若将来有人把它误挪到顶层，此脚本立即报警。
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { load } from 'js-yaml';
import { getFileMatchers } from 'app-builder-lib/out/fileMatcher.js';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const config = load(readFileSync(path.join(root, 'electron-builder.yml'), 'utf8'));

let failures = 0;
const assert = (cond, msg) => {
  console.log(`${cond ? 'ok' : 'FAIL'}: ${msg}`);
  if (!cond) failures++;
};

for (const platform of ['win', 'mac', 'linux']) {
  const matchers =
    getFileMatchers(config, 'extraResources', path.join(root, 'dist', 'unused'), {
      macroExpander: (it) => it,
      customBuildOptions: config[platform] ?? {},
      globalOutDir: path.join(root, 'dist'),
      defaultSrc: root,
    }) ?? [];
  // {from, to} 形式的条目各生成一个 matcher（patterns 为空、from/to 有效），matcher.from 即资源源目录
  const srcs = matchers.map((m) => path.basename(m.from)).sort();
  if (platform === 'win') {
    assert(srcs.includes('hawk-daemon'), `win: 携带 hawk-daemon`);
    assert(srcs.includes('hawk-update'), `win: 携带 hawk-update`);
  } else {
    assert(
      srcs.length === 1 && srcs[0] === 'hawk-daemon',
      `${platform}: 只携带 hawk-daemon，不带 hawk-update（实际：${srcs.join(', ') || '无'}）`,
    );
  }
}

if (failures > 0) {
  console.error(`\n${failures} 项断言失败`);
  process.exit(1);
}
console.log('\n全部通过');
