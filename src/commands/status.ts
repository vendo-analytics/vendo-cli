import { Command } from 'commander';

import { getClient } from '../client.js';
import {
  addExamples,
  c,
  createTable,
  printJson,
  runAction,
  timeAgo,
} from '../output.js';
import { checkForUpdates } from '../update-check.js';

interface AppItem {
  state: string;
  errorMessage?: string | null;
  consecutiveFailureCount?: number;
}

interface SourceItem {
  state: string;
  integrationStatus: string;
}

interface IntegrationItem {
  state: string;
  status: string;
}

interface JobItem {
  id: string;
  jobType: string;
  connectorType?: string | null;
  status: string;
  errorMessage?: string | null;
  finishedAt?: string | null;
}

export function registerStatusCommand(program: Command): void {
  const cmd = program
    .command('status')
    .description('Account health overview')
    .option('--json', 'Output raw JSON')
    .action(async (opts: { json?: boolean }) => {
      await checkForUpdates();

      const [appsRes, sourcesRes, integrationsRes, jobsRes] = await runAction(
        'Fetching account status...',
        () => {
          const client = getClient();
          return Promise.all([
            client.get<AppItem[]>('/apps', { limit: 100 }),
            client.get<SourceItem[]>('/sources', { limit: 100 }),
            client.get<IntegrationItem[]>('/integrations', { limit: 100 }),
            client.get<JobItem[]>('/jobs', {
              status: 'failed',
              limit: 5,
              sort: 'created_at:desc',
            }),
          ]);
        },
      );

      if (opts.json) {
        printJson({
          apps: appsRes.data,
          sources: sourcesRes.data,
          integrations: integrationsRes.data,
          recentFailures: jobsRes.data,
        });
        return;
      }

      const apps = appsRes.data;
      const sources = sourcesRes.data;
      const integrations = integrationsRes.data;
      const failedJobs = jobsRes.data;

      // Summary counts
      console.log();
      const summary = createTable(['', 'Total', 'Active', 'Paused', 'Errored']);
      summary.push([
        c.bold('Apps'),
        String(apps.length),
        c.green(String(apps.filter((a) => a.state === 'active').length)),
        c.gray(String(apps.filter((a) => a.state === 'inactive').length)),
        c.red(
          String(
            apps.filter(
              (a) =>
                typeof a.consecutiveFailureCount === 'number' &&
                a.consecutiveFailureCount > 0,
            ).length,
          ),
        ),
      ]);
      summary.push([
        c.bold('Sources'),
        String(sources.length),
        c.green(String(sources.filter((s) => s.state === 'active').length)),
        c.gray(
          String(
            sources.filter((s) => s.integrationStatus === 'paused').length,
          ),
        ),
        c.red(
          String(
            sources.filter((s) => s.integrationStatus === 'errored').length,
          ),
        ),
      ]);
      summary.push([
        c.bold('Integrations'),
        String(integrations.length),
        c.green(
          String(integrations.filter((i) => i.state === 'active').length),
        ),
        c.gray(
          String(integrations.filter((i) => i.status === 'paused').length),
        ),
        c.red(
          String(integrations.filter((i) => i.status === 'errored').length),
        ),
      ]);
      console.log(summary.toString());

      // Recent failures
      if (failedJobs.length > 0) {
        console.log();
        console.log(c.bold('Recent Failures'));
        const table = createTable([
          'Job ID',
          'Type',
          'Connector',
          'Error',
          'Failed',
        ]);
        for (const job of failedJobs) {
          table.push([
            c.dim(job.id.slice(0, 8)),
            job.jobType,
            job.connectorType ?? c.dim('—'),
            truncate(job.errorMessage ?? '—', 40),
            timeAgo(job.finishedAt),
          ]);
        }
        console.log(table.toString());
      } else {
        console.log();
        console.log(c.green('No recent failures.'));
      }

      console.log();
      console.log(c.bold('Next steps'));
      if (failedJobs[0]) {
        console.log(`  vendo jobs get ${failedJobs[0].id}`);
        console.log('  vendo jobs list --status failed');
        console.log('  vendo doctor');
      } else {
        console.log('  vendo jobs watch');
        console.log('  vendo whoami');
        console.log('  vendo doctor');
      }
    });

  addExamples(cmd, ['vendo status', 'vendo status --json']);
}

function truncate(s: string, max: number): string {
  return s.length > max ? s.slice(0, max - 1) + '...' : s;
}
