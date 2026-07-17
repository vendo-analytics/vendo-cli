import { Command } from 'commander';

import {
  type TrackingSpec,
  type TrackingSpecDiffChange,
  type TrackingSpecPayload,
  type TrackingSpecPayloadInput,
  type TrackingSpecSummary,
  trackingSpecsApi,
} from '../api/tracking-specs.js';
import { readJsonFile } from './pipeline-resource.js';
import {
  normalizeCreateTrackingSpecInput,
  normalizeUpdateTrackingSpecInput,
  parseNonNegativeInteger,
  parsePositiveInteger,
} from '../tracking-spec-input.js';
import {
  addExamples,
  c,
  colorStatus,
  confirm,
  createTable,
  printCount,
  printField,
  printJson,
  printSuccess,
  resolveOutputMode,
  runAction,
  shortId,
  showArgError,
  timeAgo,
} from '../output.js';

interface JsonOptions {
  json?: boolean;
}

interface ListOptions extends JsonOptions {
  status?: string;
  limit: string;
  offset: string;
  output?: string;
}

export function registerSpecsCommand(program: Command): void {
  const command = program
    .command('specs')
    .description(
      'Design, version, publish, and validate tracking specifications',
    );

  registerListCommand(command);
  registerGetCommand(command);
  registerCreateCommand(command);
  registerUpdateCommand(command);
  registerArchiveCommand(command);
  registerPublishCommand(command);
  registerVersionsCommand(command);
  registerDiffCommand(command);
  registerValidationResultsCommand(command);
}

function registerListCommand(parent: Command): void {
  const command = parent
    .command('list')
    .description('List tracking specifications')
    .option('--status <status>', 'Filter by status (draft, active, archived)')
    .option('--limit <n>', 'Number of results', '20')
    .option('--offset <n>', 'Pagination offset', '0')
    .option('--json', 'Output the canonical JSON response')
    .option('--output <field>', 'Print one field per spec (e.g. id, name)')
    .action(async (opts: ListOptions) => {
      const limit = parseFlag(opts.limit, '--limit', [
        'vendo specs list --limit 50',
      ]);
      const offset = parseNonNegativeFlag(opts.offset, '--offset', [
        'vendo specs list --offset 20',
      ]);
      const response = await runAction('Fetching tracking specs...', () =>
        trackingSpecsApi.list({
          status: opts.status,
          limit,
          offset,
        }),
      );
      const outputMode = resolveOutputMode(opts);

      if (outputMode === 'json') {
        printJson(response);
        return;
      }
      if (outputMode === 'field') {
        printField(
          response.data as unknown as Array<Record<string, unknown>>,
          opts.output!,
        );
        return;
      }

      const table = createTable([
        'ID',
        'Name',
        'Status',
        'Version',
        'Revision',
        'Items',
        'Updated',
      ]);
      for (const spec of response.data) {
        table.push([
          c.dim(shortId(spec.id)),
          spec.name,
          colorSpecStatus(spec.status),
          spec.currentVersion?.versionNumber
            ? `v${spec.currentVersion.versionNumber}`
            : c.dim('draft'),
          String(spec.revision),
          formatCounts(spec.counts),
          timeAgo(spec.updatedAt),
        ]);
      }
      console.log(table.toString());
      printCount(
        response.meta?.pagination?.total ?? response.data.length,
        'tracking spec',
      );
    });

  addExamples(command, [
    'vendo specs list',
    'vendo specs list --status active',
    'vendo specs list --json',
    'vendo specs list --output id',
  ]);
}

function registerGetCommand(parent: Command): void {
  const command = parent
    .command('get <specId>')
    .description('Show a tracking spec, its bindings, and reconciliation state')
    .option('--json', 'Output the canonical JSON response')
    .action(async (specId: string, opts: JsonOptions) => {
      const response = await runAction('Fetching tracking spec...', () =>
        trackingSpecsApi.get(specId),
      );

      if (opts.json) {
        printJson(response);
        return;
      }

      printSpecDetail(response.data);
    });

  addExamples(command, [
    'vendo specs get <specId>',
    'vendo specs get <specId> --json',
  ]);
}

function registerCreateCommand(parent: Command): void {
  const examples = [
    'vendo specs create --name "Shopify lifecycle" --file tracking-spec.json',
    'vendo specs create --file tracking-spec-with-metadata.json',
    'vendo specs create --name "Empty draft"',
  ];
  const command = parent
    .command('create')
    .description('Create a draft tracking spec')
    .option('--name <name>', 'Spec name (or include it in --file)')
    .option('--description <description>', 'Spec description')
    .option('--strategy-id <strategyId>', 'Link the spec to a strategy')
    .option(
      '--file <path>',
      'JSON payload or { name, description, payload, bindings } wrapper',
    )
    .option('--json', 'Output the canonical JSON response')
    .action(async (opts) => {
      const document = readOptionalFile(opts.file, examples);
      const body = normalizeCreateOrExit(
        document,
        {
          name: opts.name,
          description: opts.description,
          strategyId: opts.strategyId,
        },
        examples,
      );
      const response = await runAction('Creating tracking spec...', () =>
        trackingSpecsApi.create(body),
      );

      if (opts.json) {
        printJson(response);
        return;
      }
      printSuccess(`Tracking spec "${response.data.name}" created.`);
      console.log(`  ID:       ${response.data.id}`);
      console.log(`  Revision: ${response.data.revision}`);
      console.log();
      console.log(
        c.dim(
          `Publish with: vendo specs publish ${response.data.id} --revision ${response.data.revision}`,
        ),
      );
    });

  addExamples(command, examples);
}

function registerUpdateCommand(parent: Command): void {
  const examples = [
    'vendo specs update <specId> --revision 3 --file tracking-spec.json',
    'vendo specs update <specId> --revision 3 --name "New name"',
  ];
  const command = parent
    .command('update <specId>')
    .description('Update a draft with optimistic revision checking')
    .requiredOption(
      '--revision <n>',
      'Expected current revision (shown by specs get)',
    )
    .option('--name <name>', 'New spec name')
    .option('--description <description>', 'New description')
    .option('--file <path>', 'JSON payload or { payload, bindings } wrapper')
    .option('--json', 'Output the canonical JSON response')
    .action(async (specId: string, opts) => {
      const revision = parseRevisionFlag(opts.revision, '--revision', examples);
      const document = readOptionalFile(opts.file, examples);
      const body = normalizeUpdateOrExit(
        document,
        {
          name: opts.name,
          description: opts.description,
          expectedRevision: revision,
        },
        examples,
      );
      const response = await runAction('Updating tracking spec...', () =>
        trackingSpecsApi.update(specId, body),
      );

      if (opts.json) {
        printJson(response);
        return;
      }
      printSuccess(`Tracking spec "${response.data.name}" updated.`);
      console.log(`  Revision: ${response.data.revision}`);
    });

  addExamples(command, examples);
}

function registerArchiveCommand(parent: Command): void {
  const command = parent
    .command('archive <specId>')
    .description('Archive a tracking spec')
    .option('-y, --yes', 'Skip confirmation')
    .option('--json', 'Output the canonical JSON response (implies --yes)')
    .action(async (specId: string, opts: { yes?: boolean; json?: boolean }) => {
      if (!opts.yes && !opts.json) {
        const accepted = await confirm(
          `Archive tracking spec ${shortId(specId)}?`,
        );
        if (!accepted) {
          console.log('Cancelled');
          return;
        }
      }

      const current = await runAction('Loading tracking spec...', () =>
        trackingSpecsApi.get(specId),
      );
      const response = await runAction('Archiving tracking spec...', () =>
        trackingSpecsApi.archive(specId, current.data.revision),
      );
      if (opts.json) {
        printJson(response);
        return;
      }
      printSuccess(`Tracking spec ${shortId(specId)} archived.`);
    });

  addExamples(command, [
    'vendo specs archive <specId>',
    'vendo specs archive <specId> -y',
  ]);
}

function registerPublishCommand(parent: Command): void {
  const examples = [
    'vendo specs publish <specId> --revision 3',
    'vendo specs publish <specId> --revision 3 --changelog "Add checkout events"',
  ];
  const command = parent
    .command('publish <specId>')
    .description('Publish and freeze the current draft version')
    .requiredOption(
      '--revision <n>',
      'Expected current revision (shown by specs get)',
    )
    .option('--changelog <note>', 'Describe what changed in this version')
    .option('--json', 'Output the canonical JSON response')
    .action(async (specId: string, opts) => {
      const expectedRevision = parseRevisionFlag(
        opts.revision,
        '--revision',
        examples,
      );
      const response = await runAction('Publishing tracking spec...', () =>
        trackingSpecsApi.publish(specId, {
          expectedRevision,
          changelog: opts.changelog,
        }),
      );

      if (opts.json) {
        printJson(response);
        return;
      }
      const version = response.data.currentVersion?.versionNumber;
      printSuccess(
        version
          ? `Tracking spec "${response.data.name}" published as v${version}.`
          : `Tracking spec "${response.data.name}" published.`,
      );
      console.log(`  Revision: ${response.data.revision}`);
    });

  addExamples(command, examples);
}

function registerVersionsCommand(parent: Command): void {
  const command = parent
    .command('versions <specId>')
    .description('List immutable published versions')
    .option(
      '--include-payload',
      'Include each version payload (most useful with --json)',
    )
    .option('--json', 'Output the canonical JSON response')
    .action(
      async (
        specId: string,
        opts: JsonOptions & { includePayload?: boolean },
      ) => {
        const response = await runAction('Fetching spec versions...', () =>
          trackingSpecsApi.versions(specId, opts.includePayload),
        );
        if (opts.json) {
          printJson(response);
          return;
        }

        const table = createTable([
          'Version',
          'Checksum',
          'Published',
          'Publisher',
          'Changelog',
        ]);
        for (const version of response.data) {
          table.push([
            `v${version.versionNumber}`,
            shortId(version.checksum),
            timeAgo(version.publishedAt),
            version.publishedBy ? shortId(version.publishedBy) : c.dim('—'),
            version.changelog ?? c.dim('—'),
          ]);
        }
        console.log(table.toString());
        printCount(response.data.length, 'version');
      },
    );

  addExamples(command, [
    'vendo specs versions <specId>',
    'vendo specs versions <specId> --include-payload --json',
    'vendo specs versions <specId> --json',
  ]);
}

function registerDiffCommand(parent: Command): void {
  const examples = [
    'vendo specs diff <specId> --from-version 1 --to-version 2',
  ];
  const command = parent
    .command('diff <specId>')
    .description('Compare two published spec versions')
    .requiredOption('--from-version <n>', 'Base version')
    .requiredOption('--to-version <n>', 'Target version')
    .option('--json', 'Output the canonical JSON response')
    .action(async (specId: string, opts) => {
      const fromVersion = parseFlag(
        opts.fromVersion,
        '--from-version',
        examples,
      );
      const toVersion = parseFlag(opts.toVersion, '--to-version', examples);
      if (fromVersion === toVersion) {
        showArgError(
          '--from-version and --to-version must be different.',
          examples,
        );
      }

      const response = await runAction(
        'Comparing spec versions...',
        async () => {
          const versions = await trackingSpecsApi.versions(specId);
          const from = versions.data.find(
            (version) => version.versionNumber === fromVersion,
          );
          const to = versions.data.find(
            (version) => version.versionNumber === toVersion,
          );
          if (!from || !to) {
            const available = versions.data
              .map((version) => `v${version.versionNumber}`)
              .join(', ');
            throw new Error(
              `Published version not found. Available versions: ${
                available || 'none'
              }`,
            );
          }
          return trackingSpecsApi.diff(specId, from.id, to.id);
        },
      );
      if (opts.json) {
        printJson(response);
        return;
      }

      console.log();
      console.log(
        c.bold(
          `Tracking spec diff: v${response.data.fromVersion.versionNumber} → v${response.data.toVersion.versionNumber}`,
        ),
      );
      console.log();
      if (response.data.changes.length === 0) {
        console.log(c.dim('No changes.'));
        return;
      }

      const table = createTable(['Change', 'Kind', 'Subject', 'Details']);
      for (const change of response.data.changes) {
        table.push([
          colorDiffOperation(change.change),
          change.kind.replaceAll('_', ' '),
          shortId(change.stableKey),
          summarizeDiffValue(change),
        ]);
      }
      console.log(table.toString());
      printCount(response.data.changes.length, 'change');
    });

  addExamples(command, [
    ...examples,
    'vendo specs diff <specId> --from-version 1 --to-version 2 --json',
  ]);
}

function registerValidationResultsCommand(parent: Command): void {
  const command = parent
    .command('validation-results <specId>')
    .description('Show report-only conformance and undeclared-data results')
    .option('--status <status>', 'Filter by result status')
    .option('--limit <n>', 'Number of results', '50')
    .option('--offset <n>', 'Pagination offset', '0')
    .option('--json', 'Output the canonical JSON response')
    .action(async (specId: string, opts) => {
      const limit = parseFlag(opts.limit, '--limit', [
        'vendo specs validation-results <specId> --limit 100',
      ]);
      const offset = parseNonNegativeFlag(opts.offset, '--offset', [
        'vendo specs validation-results <specId> --offset 50',
      ]);
      const response = await runAction(
        'Fetching spec validation results...',
        () =>
          trackingSpecsApi.validationResults(specId, {
            status: opts.status,
            limit,
            offset,
          }),
      );
      if (opts.json) {
        printJson(response);
        return;
      }

      const table = createTable([
        'Status',
        'Subject',
        'Version ID',
        'Binding ID',
        'Finished',
        'Message',
      ]);
      for (const result of response.data) {
        table.push([
          colorStatus(result.status),
          result.subjectId,
          shortId(result.trackingSpecVersionId),
          result.trackingSpecBindingId
            ? shortId(result.trackingSpecBindingId)
            : c.dim('—'),
          timeAgo(result.finishedAt),
          validationMessage(result.result),
        ]);
      }
      console.log(table.toString());
      printCount(
        response.meta?.pagination?.total ?? response.data.length,
        'validation result',
      );
    });

  addExamples(command, [
    'vendo specs validation-results <specId>',
    'vendo specs validation-results <specId> --status failed',
    'vendo specs validation-results <specId> --json',
  ]);
}

function printSpecDetail(spec: TrackingSpec): void {
  const counts = spec.counts;
  console.log();
  console.log(c.bold(spec.name), c.dim(`(${spec.status})`));
  console.log();
  console.log(`  ID:               ${spec.id}`);
  console.log(`  Status:           ${colorSpecStatus(spec.status)}`);
  console.log(`  Revision:         ${spec.revision}`);
  console.log(
    `  Current version:  ${
      spec.currentVersion
        ? `v${spec.currentVersion.versionNumber}`
        : c.dim('none')
    }`,
  );
  console.log(`  Events:           ${counts.events}`);
  console.log(`  Event properties: ${counts.eventProperties}`);
  console.log(`  User properties:  ${counts.userProperties}`);
  console.log(`  Group properties: ${counts.groupProperties}`);
  console.log(`  Bindings:         ${counts.bindings}`);
  console.log(`  Updated:          ${timeAgo(spec.updatedAt)}`);
  if (spec.description) {
    console.log();
    console.log(`  ${spec.description}`);
  }

  if (spec.bindings.length > 0) {
    console.log();
    console.log(c.bold('  Bindings'));
    const bindingTable = createTable([
      'Kind',
      'Target',
      'Environment',
      'Name',
      'Active',
    ]);
    for (const binding of spec.bindings) {
      bindingTable.push([
        binding.kind.replaceAll('_', ' '),
        shortId(
          binding.kind === 'tracking_write_key'
            ? binding.trackingWriteKeyId
            : binding.sourceSemanticMappingId,
        ),
        binding.environment,
        binding.displayName ?? c.dim('—'),
        binding.isActive ? c.green('yes') : c.dim('no'),
      ]);
    }
    console.log(bindingTable.toString());
  }

  if (spec.reconciliation?.summary) {
    const reconciliation = spec.reconciliation.summary;
    console.log();
    console.log(c.bold('  Reconciliation'));
    console.log(`    Spec only:       ${reconciliation.specOnly ?? 0}`);
    console.log(`    Spec + live:     ${reconciliation.specAndLive ?? 0}`);
    console.log(`    Undeclared live: ${reconciliation.liveOnly ?? 0}`);
    console.log(`    Nonconforming:   ${reconciliation.nonconforming ?? 0}`);
  }
}

export function countPayload(
  payload: TrackingSpecPayload | TrackingSpecPayloadInput | null | undefined,
): {
  events: number;
  eventProperties: number;
  userProperties: number;
  groupProperties: number;
} {
  const events = payload?.events ?? [];
  return {
    events: events.length,
    eventProperties: events.reduce(
      (total, event) => total + (event.properties?.length ?? 0),
      0,
    ),
    userProperties: payload?.userProperties?.length ?? 0,
    groupProperties: payload?.groupProperties?.length ?? 0,
  };
}

function formatCounts(
  counts: Pick<
    ReturnType<typeof countPayload>,
    'events' | 'eventProperties' | 'userProperties' | 'groupProperties'
  >,
): string {
  return `${counts.events}e · ${counts.eventProperties}ep · ${counts.userProperties}u · ${counts.groupProperties}g`;
}

function colorSpecStatus(status: string): string {
  switch (status) {
    case 'active':
    case 'published':
      return c.green(status);
    case 'draft':
      return c.yellow(status);
    case 'archived':
      return c.dim(status);
    default:
      return status;
  }
}

function colorDiffOperation(
  operation: TrackingSpecDiffChange['change'],
): string {
  switch (operation) {
    case 'added':
      return c.green(operation);
    case 'removed':
      return c.red(operation);
    case 'changed':
      return c.yellow(operation);
  }
}

function summarizeDiffValue(change: TrackingSpecDiffChange): string {
  if (change.change === 'added') return c.dim('new declaration');
  if (change.change === 'removed') return c.dim('removed declaration');
  return c.dim('definition changed');
}

function validationMessage(result: unknown): string {
  if (!result || typeof result !== 'object' || Array.isArray(result)) {
    return c.dim('—');
  }
  const record = result as Record<string, unknown>;
  const message = record.message;
  if (typeof message === 'string' && message.length > 0) return message;
  const error = record.error;
  if (typeof error === 'string' && error.length > 0) return error;
  return c.dim('See --json for details');
}

function readOptionalFile(
  path: string | undefined,
  examples: string[],
): unknown | undefined {
  if (!path) return undefined;
  try {
    return readJsonFile(path);
  } catch (error) {
    showArgError(
      error instanceof Error ? error.message : String(error),
      examples,
    );
  }
}

function normalizeCreateOrExit(
  document: unknown | undefined,
  overrides: {
    name?: string;
    description?: string;
    strategyId?: string;
  },
  examples: string[],
) {
  try {
    return normalizeCreateTrackingSpecInput(document, overrides);
  } catch (error) {
    showArgError(
      error instanceof Error ? error.message : String(error),
      examples,
    );
  }
}

function parseRevisionFlag(
  value: string | number,
  flagName: string,
  examples: string[],
): number {
  try {
    return parseNonNegativeInteger(value, flagName);
  } catch (error) {
    showArgError(
      error instanceof Error ? error.message : String(error),
      examples,
    );
  }
}

function normalizeUpdateOrExit(
  document: unknown | undefined,
  overrides: {
    name?: string;
    description?: string;
    expectedRevision: number;
  },
  examples: string[],
) {
  try {
    return normalizeUpdateTrackingSpecInput(document, overrides);
  } catch (error) {
    showArgError(
      error instanceof Error ? error.message : String(error),
      examples,
    );
  }
}

function parseFlag(
  value: string | number,
  flagName: string,
  examples: string[],
): number {
  try {
    return parsePositiveInteger(value, flagName);
  } catch (error) {
    showArgError(
      error instanceof Error ? error.message : String(error),
      examples,
    );
  }
}

function parseNonNegativeFlag(
  value: string | number,
  flagName: string,
  examples: string[],
): number {
  const parsed = typeof value === 'number' ? value : Number(value);
  if (!Number.isInteger(parsed) || parsed < 0) {
    showArgError(`${flagName} must be a non-negative integer.`, examples);
  }
  return parsed;
}

/**
 * Kept exported for command-level tests and third-party wrappers that want the
 * same human summary without recreating the CLI's payload semantics.
 */
export function summarizeSpec(spec: TrackingSpecSummary): string {
  return `${spec.name} (${spec.status}, ${formatCounts(spec.counts)})`;
}
