import { Command } from 'commander';
import { describe, expect, it } from 'vitest';

import { countPayload, registerSpecsCommand } from '../commands/specs.js';

describe('specs command', () => {
  it('registers the complete management surface', () => {
    const program = new Command();
    registerSpecsCommand(program);

    const specs = program.commands.find(
      (command) => command.name() === 'specs',
    );
    expect(specs).toBeDefined();
    expect(specs?.commands.map((command) => command.name())).toEqual([
      'list',
      'get',
      'create',
      'update',
      'archive',
      'publish',
      'versions',
      'diff',
      'validation-results',
    ]);

    const update = specs?.commands.find(
      (command) => command.name() === 'update',
    );
    const publish = specs?.commands.find(
      (command) => command.name() === 'publish',
    );
    expect(
      update?.options.find((option) => option.long === '--revision')?.mandatory,
    ).toBe(true);
    expect(
      publish?.options.find((option) => option.long === '--revision')
        ?.mandatory,
    ).toBe(true);
  });

  it('counts all subject kinds in a payload', () => {
    expect(
      countPayload({
        events: [
          {
            name: 'Checkout',
            properties: [
              { name: 'cart_id', type: 'string' },
              { name: 'total', type: 'number' },
            ],
          },
        ],
        userProperties: [
          { name: 'plan', dataType: 'string' },
          { name: 'last_seen', dataType: 'datetime' },
        ],
        groupProperties: [
          { groupType: 'company', name: 'size', dataType: 'number' },
        ],
      }),
    ).toEqual({
      events: 1,
      eventProperties: 2,
      userProperties: 2,
      groupProperties: 1,
    });
  });
});
