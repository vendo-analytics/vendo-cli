import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

import { defineConfig } from 'tsup';

const pkg = JSON.parse(
  readFileSync(resolve(import.meta.dirname, 'package.json'), 'utf-8'),
);

export default defineConfig({
  entry: ['src/cli.ts'],
  format: ['esm'],
  target: 'node22',
  outDir: 'dist',
  clean: true,
  splitting: false,
  sourcemap: true,
  dts: false,
  banner: {
    js: '#!/usr/bin/env node',
  },
  define: {
    __CLI_VERSION__: JSON.stringify(pkg.version),
  },
});
