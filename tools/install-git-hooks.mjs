#!/usr/bin/env node
// 一次性把仓库的 Git hooks 指向版本化目录 .githooks。
// 由 hawk-app 的 npm prepare 自动调用；首次 npm install / npm ci 后即生效。
import { execFileSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

try {
  execFileSync('git', ['-C', root, 'rev-parse', '--show-toplevel'], { stdio: 'ignore' });
  execFileSync('git', ['-C', root, 'config', 'core.hooksPath', '.githooks'], { stdio: 'ignore' });
  console.log('Git hooks 已启用：.githooks/pre-commit');
} catch {
  // 非 Git 检出（源码包、容器内构建等）没有可安装的 hooks，不阻断构建。
  console.log('跳过 Git hooks 安装：当前目录不是 Git 仓库');
}
