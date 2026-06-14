import { Command } from 'commander';
import { readFileSync } from 'node:fs';

import { getClient } from '../client.js';
import { getActiveJobForResource } from '../job-progress.js';
import {
  addExamples,
  c,
  colorStatus,
  confirm,
  printDryRun,
  printJson,
  printSingleField,
  printSuccess,
  resolveOutputMode,
  runAction,
  shortId,
} from '../output.js';
import { watchTriggeredResourceJob } from '../watch-job.js';

/**
 * Sources and integrations are distinct resources (different schemas, different
 * display fields) but share an identical CRUD-ish lifecycle: pause, resume,
 * delete, and sync (with the same idempotent-job + watch behavior). This module
 * holds that shared lifecycle so the two command files only describe what's
 * genuinely resource-specific (list/get/create/update rendering).
 */
export interface PipelineResourceConfig {
  /** Lowercase singular noun used in messages and as the job resource type. */
  singular: 'source' | 'integration';
  /** Positional-argument placeholder, e.g. `sourceId`. */
  idParam: string;
  /** API base path, e.g. `/sources`. */
  apiPath: string;
}

export interface SyncTriggerResponse {
  jobId?: string;
  status?: string;
  message?: string;
}

interface ActionOpts {
  json?: boolean;
  dryRun?: boolean;
  output?: string;
}

export function readJsonFile(path: string): unknown {
  try {
    return JSON.parse(readFileSync(path, 'utf-8'));
  } catch (err) {
    const msg = err instanceof Error ? err.message : 'read error';
    throw new Error(`Failed to read ${path}: ${msg}`);
  }
}

function capitalize(s: string): string {
  return s.charAt(0).toUpperCase() + s.slice(1);
}

/**
 * Register a `pause`/`resume` command. Both are a single POST with no payload
 * differing only in verb and tense, so they share one implementation.
 */
export function registerStateActionCommand(
  parent: Command,
  config: PipelineResourceConfig,
  action: {
    /** API verb + command name, e.g. `pause`. */
    name: 'pause' | 'resume';
    /** Spinner gerund, e.g. `Pausing`. */
    gerund: string;
    /** Success past-tense, e.g. `paused`. */
    pastTense: string;
    description: string;
    examples: string[];
  },
): void {
  const { singular, apiPath, idParam } = config;
  const cmd = parent
    .command(`${action.name} <${idParam}>`)
    .description(action.description)
    .option('--json', 'Output raw JSON')
    .option('--dry-run', 'Preview the action without executing')
    .option('--output <field>', 'Print a single field (e.g. id)')
    .action(async (id: string, opts: ActionOpts) => {
      if (opts.dryRun) {
        printDryRun(action.name, singular, id);
        return;
      }

      const res = await runAction(`${action.gerund} ${singular}...`, () =>
        getClient().post(`${apiPath}/${id}/${action.name}`),
      );

      const outputMode = resolveOutputMode(opts);

      if (outputMode === 'json') {
        printJson(res);
        return;
      }

      if (outputMode === 'field') {
        console.log(id);
        return;
      }

      printSuccess(`${capitalize(singular)} ${shortId(id)} ${action.pastTense}.`);
    });

  addExamples(cmd, action.examples);
}

/** Register the `delete` command (soft delete, with a confirmation prompt). */
export function registerDeleteCommand(
  parent: Command,
  config: PipelineResourceConfig,
  description: string,
  examples: string[],
): void {
  const { singular, apiPath, idParam } = config;
  const cmd = parent
    .command(`delete <${idParam}>`)
    .description(description)
    .option('--json', 'Output raw JSON')
    .option('-y, --yes', 'Skip confirmation prompt')
    .option('--dry-run', 'Preview the action without executing')
    .option('--output <field>', 'Print a single field (e.g. id)')
    .action(async (id: string, opts: ActionOpts & { yes?: boolean }) => {
      if (opts.dryRun) {
        printDryRun('delete', singular, id);
        return;
      }

      if (!opts.yes && !opts.json) {
        const ok = await confirm(`Delete ${singular} ${shortId(id)}?`);
        if (!ok) return;
      }

      const res = await runAction(`Deleting ${singular}...`, () =>
        getClient().delete(`${apiPath}/${id}`),
      );

      const outputMode = resolveOutputMode(opts);

      if (outputMode === 'json') {
        printJson(res);
        return;
      }

      if (outputMode === 'field') {
        console.log(id);
        return;
      }

      printSuccess(`${capitalize(singular)} ${shortId(id)} deleted.`);
    });

  addExamples(cmd, examples);
}

/**
 * Register the `sync` command: idempotent (no-ops onto an existing active job),
 * triggers a new sync otherwise, and optionally watches the resulting job.
 *
 * @param dryRunFields maps the fetched detail to the resource-specific rows
 *   shown in `--dry-run` output (the shared `Active Job` row is appended here).
 */
export function registerSyncCommand<TDetail>(
  parent: Command,
  config: PipelineResourceConfig,
  dryRunFields: (detail: TDetail) => Record<string, string>,
  examples: string[],
): void {
  const { singular, apiPath, idParam } = config;
  const cmd = parent
    .command(`sync <${idParam}>`)
    .description('Trigger a manual sync')
    .option('--json', 'Output raw JSON')
    .option('--watch', 'Watch the job until it completes')
    .option('--dry-run', 'Preview the action without executing')
    .option('--output <field>', 'Print a single field (e.g. id for job ID)')
    .action(async (id: string, opts: ActionOpts & { watch?: boolean }) => {
      // Dry-run: show what would happen
      if (opts.dryRun) {
        const [detailRes, activeJob] = await runAction(
          `Checking ${singular}...`,
          async () =>
            Promise.all([
              getClient().get<TDetail>(`${apiPath}/${id}`),
              getActiveJobForResource(singular, id),
            ]),
        );
        printDryRun('trigger sync for', singular, id, {
          ...dryRunFields(detailRes.data),
          'Active Job': activeJob
            ? `${shortId(activeJob.id)} (${activeJob.status})`
            : 'none',
        });
        return;
      }

      // Idempotent: check for existing active job
      const existingJob = await runAction('Checking for active jobs...', () =>
        getActiveJobForResource(singular, id),
      );

      if (existingJob) {
        const outputMode = resolveOutputMode(opts);

        if (outputMode === 'json') {
          printJson({
            data: {
              jobId: existingJob.id,
              status: existingJob.status,
              message: 'Sync already in progress',
            },
          });
          return;
        }

        if (outputMode === 'field') {
          printSingleField(
            existingJob as unknown as Record<string, unknown>,
            opts.output ?? 'id',
          );
          return;
        }

        console.log(
          c.yellow('Sync already in progress'),
          `for ${singular} ${shortId(id)}.`,
        );
        console.log(
          `  Job: ${existingJob.id} (${colorStatus(existingJob.status)})`,
        );

        if (opts.watch) {
          await watchTriggeredResourceJob(
            id,
            singular,
            existingJob.id,
            new Date().toISOString(),
          );
        }
        return;
      }

      // No active job — trigger a new sync
      const syncRequestedAt = new Date().toISOString();
      const res = await runAction('Triggering sync...', () =>
        getClient().post<SyncTriggerResponse>(`${apiPath}/${id}/sync`),
      );

      const outputMode = resolveOutputMode(opts);

      if (outputMode === 'json') {
        printJson(res);
        return;
      }

      if (outputMode === 'field') {
        printSingleField(
          { id: res.data.jobId ?? id, ...res.data } as Record<string, unknown>,
          opts.output ?? 'id',
        );
        return;
      }

      printSuccess(`Sync triggered for ${singular} ${shortId(id)}.`);

      if (opts.watch) {
        await watchTriggeredResourceJob(
          id,
          singular,
          res.data.jobId,
          syncRequestedAt,
        );
      } else {
        console.log(
          c.dim(`Use 'vendo jobs list --${singular} ${id}' to monitor.`),
        );
      }
    });

  addExamples(cmd, examples);
}
