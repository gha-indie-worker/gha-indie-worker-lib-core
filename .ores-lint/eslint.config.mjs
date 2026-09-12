// ESLint flat config for the TypeScript/ESM sibling in typescript/.
//
// No framework plugins and no TypeScript parser: the sibling is plain ESM with
// JSDoc types, which is what keeps it loadable from a browser, from Node and
// from the Flutter web bridge with no build step.
export default [
  {
    files: ['typescript/src/**/*.mjs', 'typescript/test/**/*.mjs'],
    languageOptions: {
      ecmaVersion: 2023,
      sourceType: 'module',
      globals: {
        console: 'readonly',
        process: 'readonly',
        TextDecoder: 'readonly',
        TextEncoder: 'readonly',
        DataView: 'readonly',
        Uint8Array: 'readonly',
      },
    },
    linterOptions: { reportUnusedDisableDirectives: true },
    rules: {
      'no-unused-vars': ['error', { argsIgnorePattern: '^_' }],
      'no-var': 'error',
      'prefer-const': 'error',
      eqeqeq: ['error', 'always', { null: 'ignore' }],
      'no-implicit-coercion': 'error',
      'no-throw-literal': 'error',
      curly: ['error', 'multi-line'],
    },
  },
  { ignores: ['node_modules/**', 'target/**', 'dart/**', 'gleam/**'] },
];
