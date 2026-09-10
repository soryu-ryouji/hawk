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
    // 域边界强制（error）。两层守护，按引用书写方式分工：
    // 1) boundaries/dependencies 管「相对引用」的方向纪律：app 可引用一切；域向 shared/域；
    //    shared 只准自引用（不得上引）；域内相对引用为 internal 关系自动放行
    //    （含域 index.ts 的 barrel 再导出）。相对层的「只准经 index」不单独设卡——
    //    跨边界引用按约定一律走 @/ 别名（见下条），相对写法跨域在目录树上也不自然
    // 2) no-restricted-syntax 管「@/ 别名引用」的出口纪律（boundaries 的解析器不认识
    //    vite 别名）：@/domains/<x>/… 深入域内文件一律禁止（只准 @/domains/<x> 出口）；
    //    @/app/ 为装配层私有（app 内部互引用相对路径）
    files: ['web/src/**'],
    plugins: { boundaries },
    settings: {
      'boundaries/elements': [
        { type: 'app', pattern: 'web/src/app/**' },
        { type: 'domain', pattern: 'web/src/domains/*/**' },
        { type: 'shared', pattern: 'web/src/shared/**' },
      ],
    },
    rules: {
      'boundaries/dependencies': [
        'error',
        {
          default: 'disallow',
          policies: [
            {
              from: { element: { type: 'app' } },
              allow: [{ to: { element: { types: { anyOf: ['app', 'domain', 'shared'] } } } }],
            },
            {
              from: { element: { type: 'domain' } },
              allow: [{ to: { element: { types: { anyOf: ['domain', 'shared'] } } } }],
            },
            { from: { element: { type: 'shared' } }, allow: [{ to: { element: { type: 'shared' } } }] },
          ],
        },
      ],
      'no-restricted-syntax': [
        'error',
        {
          selector: 'ImportDeclaration[source.value=/^@\\/domains\\/[^/]+\\/.+/], ImportExpression[source.value=/^@\\/domains\\/[^/]+\\/.+/], ExportAllDeclaration[source.value=/^@\\/domains\\/[^/]+\\/.+/]',
          message: '域外引用必须经域出口：@/domains/<domain>（不得深入域内文件）',
        },
        {
          selector: 'ImportDeclaration[source.value=/^@\\/app\\//]',
          message: '@/app/ 为装配层私有（app 内部互相引用用相对路径）',
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
