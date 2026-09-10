// ESLint 扁平配置：正确性规则为主（样式交给 Prettier，末尾 eslint-config-prettier 关闭冲突项）。
// 覆盖 web 前端与 Electron 主进程/preload 源码；生成物（schema.d.ts）与构建产物不检查。
import js from '@eslint/js';
import tseslint from 'typescript-eslint';
import pluginVue from 'eslint-plugin-vue';
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
