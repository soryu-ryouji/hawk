// ESLint 扁平配置：正确性规则为主（样式交给 Prettier，末尾 eslint-config-prettier 关闭冲突项）。
// 覆盖 web 前端与 Electron 主进程/preload 源码；生成物（schema.d.ts）与构建产物不检查。
import js from '@eslint/js';
import tseslint from 'typescript-eslint';
import pluginVue from 'eslint-plugin-vue';
import boundaries from 'eslint-plugin-boundaries';
import globals from 'globals';
import prettier from 'eslint-config-prettier';

export default tseslint.config(
  {
    ignores: ['node_modules/**', 'dist/**', 'out/**', 'build/**', 'web/dist/**', 'web/src/api/schema.d.ts', 'resources/**'],
  },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  ...pluginVue.configs['flat/essential'],
  {
    files: ['**/*.{ts,vue}'],
    languageOptions: {
      globals: { ...globals.browser, ...globals.node },
    },
  },
  {
    files: ['**/*.vue'],
    languageOptions: {
      parserOptions: { parser: tseslint.parser, extraFileExtensions: ['.vue'] },
    },
  },
  {
    // 域边界强制（重构过渡期 warn，域迁移完成后收敛 error）：
    // app（装配层）可引用一切；域只能引用 shared 与其他域的 index.ts；shared 不依赖上层。
    // legacy 类型覆盖尚未迁入域结构的存量文件（components/ stores/ composables/ 根模块）——
    // 随阶段 4/5 迁移完成后收紧为 disallow
    files: ['web/src/**'],
    plugins: { boundaries },
    settings: {
      'boundaries/elements': [
        { type: 'app', pattern: 'web/src/app/**' },
        { type: 'domain', pattern: 'web/src/domains/*/**' },
        { type: 'shared', pattern: 'web/src/shared/**' },
        { type: 'legacy', pattern: 'web/src/**' },
      ],
    },
    rules: {
      // 目标终态规则的先遣（warn）：域之间禁止直接引用（须经各自 index.ts，阶段 5 升 error
      // 并补 entry-point 强制）；shared 只准自引用；legacy 存量区与 app 装配层暂不受限
      'boundaries/dependencies': [
        'warn',
        {
          default: 'disallow',
          policies: [
            {
              from: { element: { type: 'app' } },
              allow: [{ to: { element: { types: { anyOf: ['app', 'domain', 'shared', 'legacy'] } } } }],
            },
            {
              from: { element: { type: 'domain' } },
              allow: [
                { to: { element: { types: { anyOf: ['shared', 'legacy'] } } } },
                // 同元素内部引用（域内 index/相对路径）放行；跨域仍需经对方 index（阶段 5 收紧）
                { to: { element: { type: 'domain' } }, sameElement: true },
              ],
            },
            { from: { element: { type: 'shared' } }, allow: [{ to: { element: { type: 'shared' } } }] },
            {
              from: { element: { type: 'legacy' } },
              allow: [{ to: { element: { types: { anyOf: ['legacy', 'app', 'domain', 'shared'] } } } }],
            },
          ],
        },
      ],
    },
  },
  {
    rules: {
      // 组件名沿用 PascalCase（Icon/TitleBar 等），不强制多词
      'vue/multi-word-component-names': 'off',
      // 闭包先读后赋的初始化模式（如 fail() 引用尚未创建的定时器句柄）是刻意的
      'prefer-const': ['error', { ignoreReadBeforeAssign: true }],
      // `_` 前缀参数/变量为刻意占位（测试 mock 签名仅为类型推断而声明）
      '@typescript-eslint/no-unused-vars': ['error', { argsIgnorePattern: '^_', varsIgnorePattern: '^_' }],
    },
  },
  prettier,
);
