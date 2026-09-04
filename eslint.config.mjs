import tseslint from '@typescript-eslint/eslint-plugin';
import tsparser from '@typescript-eslint/parser';
import reactHooks from 'eslint-plugin-react-hooks';
export default [
  { ignores: ['dist/**', 'src-tauri/target/**', 'node_modules/**'] },
  {
    files: ['src/**/*.{ts,tsx}', 'e2e/**/*.ts', 'scripts/**/*.ts'],
    languageOptions: { parser: tsparser, parserOptions: { ecmaVersion: 2022, sourceType: 'module', ecmaFeatures: { jsx: true } } },
    plugins: { '@typescript-eslint': tseslint, 'react-hooks': reactHooks },
    rules: {
      ...tseslint.configs['recommended'].rules,
      '@typescript-eslint/no-explicit-any': 'error',
      'react-hooks/rules-of-hooks': 'error',
      'react-hooks/exhaustive-deps': 'warn',
      'no-restricted-syntax': ['error', { selector: "CallExpression[callee.name='unwrap']", message: 'no unwrap outside tests' }],
    },
  },
];
