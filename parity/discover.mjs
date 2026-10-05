// Print the TypeScript CLI's command tree as JSON without running a command:
// patch commander's parse() before the built CLI registers and parses.
// Usage: node parity/discover.mjs [repoRoot] [--all]
// Prints the leaf commands; with --all, every command path instead (the root
// as "", then groups and leaves in help order), for the help check.
import { createRequire } from 'node:module';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const all = process.argv.includes('--all');
const root = resolve(process.argv.slice(2).find((arg) => arg !== '--all') ?? '.');
const require = createRequire(`${root}/package.json`);
const { Command } = await import(pathToFileURL(require.resolve('commander')).href);

Command.prototype.parse = function dumpTree() {
  const leaves = [];
  const paths = [];
  const walk = (cmd, path) => {
    const subs = cmd.commands.filter((sub) => sub.name() !== 'help');
    paths.push(path.join(' '));
    if (subs.length === 0) {
      leaves.push({
        path: path.join(' '),
        args: cmd.registeredArguments.map((arg) => ({
          name: arg.name(),
          required: arg.required,
        })),
        options: cmd.options.map((opt) => opt.long).filter(Boolean),
      });
      return;
    }
    for (const sub of subs) walk(sub, [...path, sub.name()]);
  };
  walk(this, []);
  process.stdout.write(JSON.stringify(all ? paths : leaves));
  return this;
};

await import(pathToFileURL(`${root}/dist/cli.js`).href);
