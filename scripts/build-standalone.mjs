import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { execFileSync } from 'node:child_process';

const cliRoot = resolve(import.meta.dirname, '..');
const distDir = join(cliRoot, 'dist');
const seaBundlePath = join(distDir, 'cli.sea.js');
const tempDir = mkdtempSync(join(tmpdir(), 'vendo-sea-'));
const seaConfigPath = join(tempDir, 'sea-config.json');
const seaBlobPath = join(tempDir, 'sea-prep.blob');
const nodeBinary = process.env.SEA_NODE_BINARY || process.execPath;
const binaryName = getBinaryName();
const binaryPath = join(distDir, binaryName);
const sentinelFuse = 'NODE_SEA_FUSE_fce680ab2cc467b6e072b8b5df1996b2';
const sentinelMarker = `${sentinelFuse}:0`;

if (!existsSync(seaBundlePath)) {
  throw new Error(`SEA bundle not found at ${seaBundlePath}. Run build:sea-bundle first.`);
}

if (!readFileSync(nodeBinary).includes(Buffer.from(sentinelMarker))) {
  throw new Error(
    `Node binary at ${nodeBinary} is missing the SEA sentinel fuse. ` +
      'Use an official Node 22 binary via SEA_NODE_BINARY to build standalone executables.',
  );
}

rmSync(binaryPath, { force: true });

try {
  writeFileSync(
    seaConfigPath,
    JSON.stringify(
      {
        main: seaBundlePath,
        output: seaBlobPath,
        disableExperimentalSEAWarning: true,
        useSnapshot: false,
        useCodeCache: false,
      },
      null,
      2,
    ),
  );

  execFileSync(nodeBinary, ['--experimental-sea-config', seaConfigPath], {
    cwd: cliRoot,
    stdio: 'inherit',
  });

  mkdirSync(dirname(binaryPath), { recursive: true });
  copyFileSync(nodeBinary, binaryPath);
  chmodSync(binaryPath, 0o755);

  if (process.platform === 'darwin') {
    execFileSync('codesign', ['--remove-signature', binaryPath], {
      cwd: cliRoot,
      stdio: 'inherit',
    });
  }

  const postjectArgs = [
    'exec',
    'postject',
    binaryPath,
    'NODE_SEA_BLOB',
    seaBlobPath,
    '--sentinel-fuse',
    sentinelFuse,
  ];

  if (process.platform === 'darwin') {
    postjectArgs.push('--macho-segment-name', 'NODE_SEA');
  }

  execFileSync('pnpm', postjectArgs, {
    cwd: cliRoot,
    stdio: 'inherit',
  });

  if (process.platform === 'darwin') {
    execFileSync('codesign', ['--sign', '-', binaryPath], {
      cwd: cliRoot,
      stdio: 'inherit',
    });
  }
} finally {
  rmSync(tempDir, { recursive: true, force: true });
}

console.log(`Standalone binary written to ${binaryPath}`);

function getBinaryName() {
  const platform = process.platform;
  const arch = process.arch;

  if (!['darwin', 'linux'].includes(platform)) {
    throw new Error(`Unsupported platform for standalone build: ${platform}`);
  }

  if (!['arm64', 'x64'].includes(arch)) {
    throw new Error(`Unsupported architecture for standalone build: ${arch}`);
  }

  return `vendo-${platform}-${arch}`;
}
