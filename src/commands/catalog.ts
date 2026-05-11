import { Command } from 'commander';

import { getClient } from '../client.js';
import {
  addExamples,
  c,
  createTable,
  printCount,
  printField,
  printJson,
  resolveOutputMode,
  runAction,
} from '../output.js';

interface CatalogItem {
  appType: string;
  displayName: string;
  category: string;
  description: string;
  supportedRoles: string[];
  selfServe?: boolean;
  lifecycle?: string;
}

interface CatalogDetail extends CatalogItem {
  provider?: string;
  logoUrl?: string | null;
  credentialFields?: Array<{
    name: string;
    label: string;
    type: string;
    description?: string;
  }>;
  documentationUrl?: string | null;
}

export function registerCatalogCommand(program: Command): void {
  const cmd = program
    .command('catalog')
    .description('Browse available integration types');

  // catalog list
  const listCmd = cmd
    .command('list')
    .description('List all available integration types')
    .option('--category <category>', 'Filter by category')
    .option('--role <role>', 'Filter by role (source, destination)')
    .option('--json', 'Output raw JSON')
    .option('--output <field>', 'Print a single field per row (e.g. appType)')
    .action(async (opts) => {
      const outputMode = resolveOutputMode(opts);

      const res = await runAction('Fetching catalog...', () =>
        getClient().get<CatalogItem[]>('/catalog', {
          category: opts.category,
          role: opts.role,
        }),
      );

      if (outputMode === 'json') {
        printJson(res);
        return;
      }

      if (outputMode === 'field') {
        printField(
          res.data as unknown as Record<string, unknown>[],
          opts.output,
        );
        return;
      }

      const table = createTable([
        'App Type',
        'Name',
        'Category',
        'Roles',
        'Self-Serve',
      ]);

      for (const item of res.data) {
        table.push([
          c.cyan(item.appType),
          item.displayName,
          item.category,
          item.supportedRoles.join(', '),
          item.selfServe ? c.green('yes') : c.dim('no'),
        ]);
      }

      console.log(table.toString());
      printCount(res.data.length, 'integration type');
    });

  addExamples(listCmd, [
    'vendo catalog list',
    'vendo catalog list --role source',
    'vendo catalog list --output appType',
  ]);

  // catalog get
  const getCmd = cmd
    .command('get <appType>')
    .description('Get details for a specific integration type')
    .option('--json', 'Output raw JSON')
    .action(async (appType: string, opts: { json?: boolean }) => {
      const res = await runAction('Fetching catalog entry...', () =>
        getClient().get<CatalogDetail>(`/catalog/${appType}`),
      );

      if (opts.json) {
        printJson(res);
        return;
      }

      const item = res.data;
      console.log();
      console.log(c.bold(item.displayName), c.dim(`(${item.appType})`));
      console.log();
      console.log(`  Category:    ${item.category}`);
      console.log(`  Roles:       ${item.supportedRoles.join(', ')}`);
      console.log(
        `  Self-Serve:  ${item.selfServe ? c.green('yes') : c.dim('no')}`,
      );
      if (item.lifecycle) {
        console.log(`  Lifecycle:   ${item.lifecycle}`);
      }
      if (item.provider) {
        console.log(`  Provider:    ${item.provider}`);
      }
      if (item.description) {
        console.log();
        console.log(`  ${item.description}`);
      }
      if (item.documentationUrl) {
        console.log();
        console.log(`  Docs: ${c.cyan(item.documentationUrl)}`);
      }
      if (item.credentialFields && item.credentialFields.length > 0) {
        console.log();
        console.log(c.bold('  Credential Fields:'));
        for (const field of item.credentialFields) {
          console.log(
            `    ${field.label} (${c.dim(field.name)}) — ${field.type}`,
          );
          if (field.description) {
            console.log(`      ${c.dim(field.description)}`);
          }
        }
      }
    });

  addExamples(getCmd, [
    'vendo catalog get shopify',
    'vendo catalog get bigquery --json',
  ]);

  // catalog credential-schema <appType>
  const credSchemaCmd = cmd
    .command('credential-schema <appType>')
    .description(
      'Print the credential fields required to create an app of this type',
    )
    .option('--json', 'Output raw JSON')
    .action(async (appType: string, opts) => {
      const res = await runAction('Fetching credential schema...', () =>
        getClient().get<CatalogDetail>(`/catalog/${appType}`),
      );

      if (opts.json) {
        printJson({
          appType,
          credentialFields: res.data?.credentialFields ?? [],
        });
        return;
      }

      const fields = res.data?.credentialFields ?? [];
      if (fields.length === 0) {
        console.log(c.dim(`No credential fields declared for ${appType}.`));
        return;
      }

      console.log(c.bold(`Credential fields for ${appType}:`));
      for (const field of fields) {
        console.log(`  ${field.label} (${c.dim(field.name)}) — ${field.type}`);
        if (field.description) console.log(`    ${c.dim(field.description)}`);
      }
      console.log();
      console.log(
        c.dim(
          'Use these as keys in the JSON file you pass to --credentials-file for `vendo apps create`.',
        ),
      );
    });

  addExamples(credSchemaCmd, [
    'vendo catalog credential-schema onesignal',
    'vendo catalog credential-schema bigquery --json',
  ]);
}
