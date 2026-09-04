// 一次性迁移：把含 :hover 的规则包进 @media (hover: hover)（触屏无 hover 能力，粘性 hover 残留高亮）。
// 用法：node tools/wrap-hover.mjs。幂等（已在 @media (hover:*) 内的规则跳过）。
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import postcss from 'postcss';

const src = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../web/src');

/** 规则是否已在 hover 能力媒体查询内 */
function inHoverAtRule(rule) {
  for (let p = rule.parent; p; p = p.parent) {
    if (p.type === 'atrule' && p.name === 'media' && /hover\s*:\s*hover/.test(p.params)) {
      return true;
    }
  }
  return false;
}

function wrapHoverRules(css) {
  const root = postcss.parse(css);
  let changed = false;
  root.walkRules((rule) => {
    if (!rule.selector?.includes(':hover') || inHoverAtRule(rule)) {
      return;
    }
    const media = postcss.atRule({ name: 'media', params: '(hover: hover)' });
    rule.replaceWith(media);
    media.append(rule);
    changed = true;
  });
  return changed ? root.toString() : css;
}

/** 处理 .vue 文件的 <style> 块（可能多个）与 .css 文件 */
function processFile(file) {
  let text = fs.readFileSync(file, 'utf8');
  if (file.endsWith('.css')) {
    const out = wrapHoverRules(text);
    if (out !== text) {
      fs.writeFileSync(file, out);
      console.log(`wrapped: ${path.relative(src, file)}`);
    }
    return;
  }
  let changed = false;
  text = text.replace(/(<style[^>]*>)([\s\S]*?)(<\/style>)/g, (_, open, body, close) => {
    const out = wrapHoverRules(body);
    if (out !== body) {
      changed = true;
    }
    return open + out + close;
  });
  if (changed) {
    fs.writeFileSync(file, text);
    console.log(`wrapped: ${path.relative(src, file)}`);
  }
}

function walk(dir) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      walk(full);
    } else if (/\.(vue|css)$/.test(entry.name)) {
      processFile(full);
    }
  }
}

walk(src);
console.log('done');
