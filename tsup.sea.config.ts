import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

import { defineConfig } from 'tsup';

const pkg = JSON.parse(
  readFileSync(resolve(import.meta.dirname, 'package.json'), 'utf-8'),
);

export default defineConfig({
  entry: ['src/cli.ts'],
  format: ['cjs'],
  target: 'node22',
  outDir: 'dist',
  outExtension: () => ({ js: '.sea.js' }),
  clean: false,
  splitting: false,
  noExternal: [/.*/],
  sourcemap: false,
  dts: false,
  banner: {
    js: '#!/usr/bin/env node',
  },
  define: {
    __CLI_VERSION__: JSON.stringify(pkg.version),
  },
});
