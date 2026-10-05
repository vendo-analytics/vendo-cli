import { Command } from 'commander';
import { describe, expect, it } from 'vitest';

import { registerDictionaryCommand } from '../commands/dictionary.js';

function helpText(command: Command): string {
  let out = '';
  command.configureOutput({
    writeOut: (str) => {
      out += str;
    },
    writeErr: (str) => {
      out += str;
    },
  });
  command.outputHelp();
  return out;
}

describe('dictionary command', () => {
  function registered(): Command {
    const program = new Command();
    registerDictionaryCommand(program);
    const dictionary = program.commands.find(
      (command) => command.name() === 'dictionary',
    );
    expect(dictionary).toBeDefined();
    return dictionary!;
  }

  it('registers search, list, and get', () => {
    const dictionary = registered();
    expect(dictionary.commands.map((command) => command.name())).toEqual([
      'list',
      'search',
      'get',
    ]);
    expect(dictionary.description()).toMatch(/data dictionary/i);
    expect(helpText(dictionary)).toMatch(/search|list|get/i);
  });

  it('lists events by default and exposes catalog filters', () => {
    const dictionary = registered();
    const list = dictionary.commands.find(
      (command) => command.name() === 'list',
    );
    expect(list).toBeDefined();

    const type = list?.options.find((option) => option.long === '--type');
    const query = list?.options.find((option) => option.long === '--query');
    expect(type?.defaultValue).toBe('event');
    expect(query?.short).toBe('-q');

    const help = helpText(list!);
    expect(help).toContain('--type');
    expect(help).toContain('--query');
    expect(help).toContain('--limit');
    expect(help).toContain('--offset');
    expect(help).toContain('--json');
    expect(help).toContain('--output');
    expect(help).toContain('vendo dictionary list');
    expect(help).toContain('--type event --query checkout');
  });

  it('looks up a single subject id', () => {
    const dictionary = registered();
    const get = dictionary.commands.find((command) => command.name() === 'get');
    expect(get).toBeDefined();
    expect(get?.registeredArguments.map((argument) => argument.name())).toEqual(
      ['subjectId'],
    );

    const help = helpText(get!);
    expect(help).toContain('--json');
    expect(help).toContain('vendo dictionary get event:checkout_completed');
  });

  it('searches catalog names and descriptions', () => {
    const dictionary = registered();
    const search = dictionary.commands.find(
      (command) => command.name() === 'search',
    );
    expect(search).toBeDefined();
    expect(
      search?.registeredArguments.map((argument) => argument.name()),
    ).toEqual(['query']);

    const help = helpText(search!);
    expect(help).toContain('--type');
    expect(help).toContain('--json');
    expect(help).toContain('vendo dictionary search checkout');
  });
});
