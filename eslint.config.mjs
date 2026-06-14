import js from '@eslint/js';
import tseslint from '@typescript-eslint/eslint-plugin';
import tsparser from '@typescript-eslint/parser';

export default [
  js.configs.recommended,
  {
    files: ['src/**/*.ts'],
    languageOptions: {
      parser: tsparser,
      parserOptions: {
        ecmaVersion: 2022,
        sourceType: 'module',
      },
    },
    plugins: {
      '@typescript-eslint': tseslint,
    },
    rules: {
      // Terminal UI directly writes to stdout — that's the product.
      'no-console': 'off',
      // TypeScript already reports undefined identifiers during type
      // checking, and `no-undef` misfires on type-only globals (e.g.
      // `RequestInit`) while forcing a hand-maintained globals allowlist.
      // Per typescript-eslint guidance, turn it off for TS sources.
      'no-undef': 'off',
      'no-unused-vars': 'off',
      '@typescript-eslint/no-unused-vars': [
        'error',
        { argsIgnorePattern: '^_', varsIgnorePattern: '^_' },
      ],
    },
  },
  {
    ignores: ['dist/**', 'node_modules/**', 'scripts/**'],
  },
];
