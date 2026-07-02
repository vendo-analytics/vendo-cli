import { Command } from 'commander';

import { getClient } from '../client.js';
import {
  type EnsureSourceDataResult,
  resolveRefreshWindow,
  summarizeEnsureSourceData,
} from '../source-refresh.js';
import {
  formatJobProgress,
  getActiveJobForResource,
  getActiveJobs,
} from '../job-progress.js';
import {
  addExamples,
  c,
  colorStatus,
  createTable,
  exitWithError,
  printCount,
  printField,
  printJson,
  printLabel,
  printSuccess,
  resolveOutputMode,
  runAction,
  shortId,
  timeAgo,
} from '../output.js';
import {
  type PipelineResourceConfig,
  readJsonFile,
  registerDeleteCommand,
  registerStateActionCommand,
  registerSyncCommand,
} from './pipeline-resource.js';

interface IntegrationItem {
  id: string;
  sourceAppId?: string | null;
  sourceAppName?: string | null;
  sourceAppType?: string | null;
  destinationAppId: string;
  destinationAppName?: string | null;
  destinationAppType?: string | null;
  dataType: string;
  state: string;
  status: string;
  lastSyncAt?: string | null;
  lastError?: string | null;
  consecutiveFailures?: number;
  latestJobId?: string | null;
  createdAt: string;
}

interface IntegrationDetail extends IntegrationItem {
  config?: unknown;
  schedule?: unknown;
  isActive?: boolean;
  importDependencyStatus?: string | null;
  latestJobId?: string | null;
  historicalSyncStartDate?: string | null;
  updatedAt: string;
}

const RESOURCE: PipelineResourceConfig = {
  singular: 'integration',
  idParam: 'integrationId',
  apiPath: '/integrations',
};

export function registerIntegrationsCommand(program: Command): void {
  const cmd = program
    .command('integrations')
    .alias('int')
    .description('Manage data export integrations');

  // integrations list
  const listCmd = cmd
    .command('list')
    .description('List all integrations')
    .option('--state <state>', 'Filter by state (active, inactive)')
    .option('--status <status>', 'Filter by status')
    .option('--type <type>', 'Filter by data type')
    .option('--limit <n>', 'Number of results', '20')
    .option('--offset <n>', 'Pagination offset', '0')
    .option('--json', 'Output raw JSON')
    .option('--output <field>', 'Print a single field per row (e.g. id)')
    .action(async (opts) => {
      const outputMode = resolveOutputMode(opts);

      const integrationsRequest = () =>
        getClient().get<IntegrationItem[]>('/integrations', {
          state: opts.state,
          status: opts.status,
          data_type: opts.type,
          limit: opts.limit,
          offset: opts.offset,
        });

      if (outputMode === 'json') {
        const res = await runAction(
          'Fetching integrations...',
          integrationsRequest,
        );
        printJson(res);
        return;
      }

      if (outputMode === 'field') {
        const res = await runAction(
          'Fetching integrations...',
          integrationsRequest,
        );
        printField(
          res.data as unknown as Record<string, unknown>[],
          opts.output,
        );
        return;
      }

      const [res, activeJobs] = await runAction(
        'Fetching integrations...',
        async () => Promise.all([integrationsRequest(), getActiveJobs()]),
      );

      const activeJobsByIntegrationId = new Map(
        activeJobs
          .filter((job) => job.integrationId)
          .map((job) => [job.integrationId!, job]),
      );

      const table = createTable([
        'ID',
        'Source',
        'Destination',
        'Data Type',
        'Status',
        'Progress',
        'Last Sync',
      ]);

      for (const int of res.data) {
        table.push([
          c.dim(shortId(int.id)),
          int.sourceAppName ?? c.dim('—'),
          int.destinationAppName ?? c.dim('—'),
          int.dataType,
          colorStatus(int.status),
          formatJobProgress(activeJobsByIntegrationId.get(int.id)),
          timeAgo(int.lastSyncAt),
        ]);
      }

      console.log(table.toString());
      printCount(res.meta?.pagination?.total ?? res.data.length, 'integration');
    });

  addExamples(listCmd, [
    'vendo integrations list',
    'vendo int list --state active',
    'vendo integrations list --output id',
  ]);

  // integrations get
  const getCmd = cmd
    .command('get <integrationId>')
    .description('Get integration details')
    .option('--json', 'Output raw JSON')
    .action(async (integrationId: string, opts: { json?: boolean }) => {
      const integrationRequest = () =>
        getClient().get<IntegrationDetail>(`/integrations/${integrationId}`);

      if (opts.json) {
        const res = await runAction(
          'Fetching integration...',
          integrationRequest,
        );
        printJson(res);
        return;
      }

      const [res, activeJob] = await runAction(
        'Fetching integration...',
        async () =>
          Promise.all([
            integrationRequest(),
            getActiveJobForResource('integration', integrationId),
          ]),
      );

      const int = res.data;
      console.log();
      console.log(
        c.bold(
          `${int.sourceAppName ?? '—'} → ${int.destinationAppName ?? '—'}`,
        ),
        c.dim(`(${int.dataType})`),
      );
      console.log();
      console.log(`  ID:            ${int.id}`);
      console.log(
        `  Source App:    ${int.sourceAppName ?? c.dim('—')} ${c.dim(int.sourceAppId ?? '')}`,
      );
      console.log(
        `  Dest App:     ${int.destinationAppName ?? c.dim('—')} ${c.dim(int.destinationAppId)}`,
      );
      console.log(`  Data Type:    ${int.dataType}`);
      console.log(`  State:        ${colorStatus(int.state)}`);
      console.log(`  Status:       ${colorStatus(int.status)}`);
      console.log(`  Progress:     ${formatJobProgress(activeJob)}`);
      console.log(`  Last Sync:    ${timeAgo(int.lastSyncAt)}`);
      console.log(`  Created:      ${timeAgo(int.createdAt)}`);

      if (int.schedule) {
        console.log(`  Schedule:     ${JSON.stringify(int.schedule)}`);
      }
      if (int.lastError) {
        console.log(`  Error:        ${c.red(int.lastError)}`);
      }
      if (int.consecutiveFailures && int.consecutiveFailures > 0) {
        console.log(
          `  Failures:     ${c.red(String(int.consecutiveFailures))} consecutive`,
        );
      }
      if (int.latestJobId) {
        console.log(`  Latest Job:   ${c.dim(int.latestJobId)}`);
      }
    });

  addExamples(getCmd, [
    'vendo integrations get <integrationId>',
    'vendo int get <integrationId> --json',
  ]);

  // integrations sync (with idempotent check)
  registerSyncCommand<IntegrationDetail>(
    cmd,
    RESOURCE,
    (int) => ({
      Source: int.sourceAppName ?? '—',
      Destination: int.destinationAppName ?? '—',
      'Data Type': int.dataType,
    }),
    [
      'vendo integrations sync <integrationId>',
      'vendo int sync <integrationId> --watch',
      'vendo integrations sync <integrationId> --dry-run',
    ],
  );

  // integrations refresh-source (ensure-source-data, VE-1565)
  const refreshCmd = cmd
    .command('refresh-source <integrationId>')
    .description(
      'Check source-data availability for a window and trigger top-up imports for missing ranges',
    )
    .option(
      '--from <date>',
      'Window start — ISO datetime or YYYY-MM-DD (default: 7 days before --to)',
    )
    .option('--to <date>', 'Window end — ISO datetime or YYYY-MM-DD (default: now)')
    .option('--json', 'Output raw JSON')
    .action(
      async (
        integrationId: string,
        opts: { from?: string; to?: string; json?: boolean },
      ) => {
        let window;
        try {
          window = resolveRefreshWindow(opts.from, opts.to, new Date());
        } catch (err) {
          exitWithError(err);
        }

        // The web API resolves the integration's source app itself; the CLI
        // only supplies the window (full ISO — the route rejects bare dates).
        const res = await runAction(
          'Checking source data availability...',
          () =>
            getClient().post<EnsureSourceDataResult>(
              `/integrations/${integrationId}/refresh-source`,
              {
                requestedStart: window.requestedStart,
                requestedEnd: window.requestedEnd,
              },
            ),
        );

        const summary = summarizeEnsureSourceData(res.data);

        if (opts.json) {
          // Keep stdout pure JSON, but preserve the failure exit code — CI
          // chains like `refresh-source --json && sync` must stop on
          // `unavailable` exactly like the human-readable path does (VE-1603).
          printJson(res);
          if (summary.tone === 'error') {
            process.exit(1);
          }
          return;
        }

        if (summary.tone === 'error') {
          exitWithError(new Error(summary.headline));
        }

        printSuccess(summary.headline);
        console.log(
          c.dim(
            `Window: ${window.requestedStart} → ${window.requestedEnd}`,
          ),
        );
        for (const jobId of summary.jobIds) {
          console.log(`  Import job: ${jobId}`);
        }
        if (summary.jobIds.length > 0) {
          console.log();
          console.log(
            c.dim(`Follow progress: vendo jobs tail ${summary.jobIds[0]}`),
          );
        }
      },
    );

  addExamples(refreshCmd, [
    'vendo integrations refresh-source <integrationId>',
    'vendo int refresh-source <integrationId> --from 2026-06-29 --to 2026-07-02',
    'vendo integrations refresh-source <integrationId> --json',
  ]);

  // integrations pause
  registerStateActionCommand(cmd, RESOURCE, {
    name: 'pause',
    gerund: 'Pausing',
    pastTense: 'paused',
    description: 'Pause an integration',
    examples: [
      'vendo integrations pause <integrationId>',
      'vendo int pause <integrationId> --dry-run',
    ],
  });

  // integrations resume
  registerStateActionCommand(cmd, RESOURCE, {
    name: 'resume',
    gerund: 'Resuming',
    pastTense: 'resumed',
    description: 'Resume a paused integration',
    examples: [
      'vendo integrations resume <integrationId>',
      'vendo integrations resume <integrationId> --dry-run',
    ],
  });

  // integrations delete
  registerDeleteCommand(
    cmd,
    RESOURCE,
    'Delete an integration (soft delete)',
    [
      'vendo integrations delete <integrationId>',
      'vendo int delete <integrationId> --yes',
      'vendo integrations delete <integrationId> --dry-run',
    ],
  );

  // integrations create
  const createCmd = cmd
    .command('create')
    .description('Create a new integration (source → destination pipeline)')
    .requiredOption(
      '--dest-app <appId>',
      'Destination app ID (must have destination role)',
    )
    .option(
      '--source-app <appId>',
      'Source app ID (optional for some data types)',
    )
    .requiredOption(
      '--data-type <type>',
      'Data type (e.g. events, user_properties, conversions)',
    )
    .requiredOption(
      '--config-file <path>',
      'Path to JSON with { global?, tasks: [...] }',
    )
    .option('--schedule-file <path>', 'Path to JSON with schedule overrides')
    .option('--frequency <value>', 'Sync frequency value', '1')
    .option(
      '--unit <unit>',
      'Sync frequency unit (hours, days, weeks, months)',
      'days',
    )
    .option('--run-now', 'Trigger first sync immediately')
    .option('--json', 'Output raw JSON')
    .option('--output <field>', 'Print a single field (e.g. id)')
    .action(async (opts) => {
      const config = readJsonFile(opts.configFile);
      const schedule = opts.scheduleFile
        ? (readJsonFile(opts.scheduleFile) as Record<string, unknown>)
        : {
            frequencyValue: Number(opts.frequency),
            frequencyUnit: opts.unit,
            runNow: Boolean(opts.runNow),
          };

      const body = {
        destinationAppId: opts.destApp,
        sourceAppId: opts.sourceApp,
        dataType: opts.dataType,
        config,
        schedule,
      };

      const res = await runAction('Creating integration...', () =>
        getClient().post<IntegrationDetail>('/integrations', body),
      );

      const outputMode = resolveOutputMode(opts);

      if (outputMode === 'json') {
        printJson(res);
        return;
      }

      const int = res.data;
      if (!int) {
        printSuccess('Integration created.');
        return;
      }

      if (outputMode === 'field') {
        console.log(int.id);
        return;
      }

      printSuccess(`Integration ${shortId(int.id)} created.`);
      printLabel('Data type', int.dataType);
      printLabel('State', colorStatus(int.state));
    });

  addExamples(createCmd, [
    'vendo int create --dest-app <onesignal-app-id> --data-type events --config-file tasks.json',
    'vendo int create --source-app <bq-id> --dest-app <os-id> --data-type user_properties --config-file tasks.json --run-now',
  ]);

  // integrations update
  const updateCmd = cmd
    .command('update <integrationId>')
    .description('Update an integration')
    .option('--config-file <path>', 'Replace config from a JSON file')
    .option('--schedule-file <path>', 'Replace schedule from a JSON file')
    .option('--frequency <value>', 'Sync frequency value')
    .option('--unit <unit>', 'Sync frequency unit')
    .option('--json', 'Output raw JSON')
    .option('--output <field>', 'Print a single field (e.g. id)')
    .action(async (integrationId: string, opts) => {
      const body: Record<string, unknown> = {};
      if (opts.configFile) body.config = readJsonFile(opts.configFile);

      if (opts.scheduleFile) {
        body.schedule = readJsonFile(opts.scheduleFile);
      } else if (opts.frequency !== undefined || opts.unit !== undefined) {
        const schedule: Record<string, unknown> = {};
        if (opts.frequency !== undefined) {
          schedule.frequencyValue = Number(opts.frequency);
        }
        if (opts.unit !== undefined) schedule.frequencyUnit = opts.unit;
        body.schedule = schedule;
      }

      if (Object.keys(body).length === 0) {
        throw new Error('Nothing to update — pass at least one flag.');
      }

      const res = await runAction('Updating integration...', () =>
        getClient().patch<IntegrationDetail>(
          `/integrations/${integrationId}`,
          body,
        ),
      );

      const outputMode = resolveOutputMode(opts);

      if (outputMode === 'json') {
        printJson(res);
        return;
      }

      if (outputMode === 'field') {
        console.log(integrationId);
        return;
      }

      printSuccess(`Integration ${shortId(integrationId)} updated.`);
    });

  addExamples(updateCmd, [
    'vendo int update <integrationId> --frequency 6 --unit hours',
    'vendo int update <integrationId> --config-file new-tasks.json',
  ]);
}
