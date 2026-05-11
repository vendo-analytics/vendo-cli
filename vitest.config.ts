import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

import { defineConfig } from 'vitest/config';

const pkg = JSON.parse(
  readFileSync(resolve(import.meta.dirname, 'package.json'), 'utf-8'),
);

export default defineConfig({
  test: {
    environment: 'node',
    include: ['src/__tests__/**/*.test.ts'],
    exclude: ['node_modules', 'dist'],
  },
  define: {
    // Provide the compile-time constant used by update-check.ts (matches tsup behavior)
    __CLI_VERSION__: JSON.stringify(pkg.version),
  },
});
