// Generates tests/fixtures/node-locale.json (VE-3728): what Node prints for
// `toLocaleDateString()`, `toLocaleTimeString()`, `Number#toLocaleString()` and the measurement
// views' money format with each LANG under each TZ. The Rust table test in
// src/output/locale.rs must reproduce every value.
//
//   node scripts/gen-locale-fixture.mjs > tests/fixtures/node-locale.json
import { execFileSync } from 'node:child_process';

const LANGS = ['en_US.UTF-8', 'en_AU.UTF-8', 'en_GB.UTF-8', 'de_DE.UTF-8', 'ja_JP.UTF-8', 'C'];
const ZONES = ['Australia/Sydney', 'UTC', 'America/New_York'];
const INSTANTS = [
  '1970-01-01T00:00:00Z', '1999-12-31T23:30:00Z', '2000-02-29T12:00:00Z', '2026-01-05T09:07:03Z',
  '2026-07-01T00:30:00Z', '2026-07-01T13:59:59.999Z', '2026-07-01T15:04:05Z', '2026-10-04T15:59:00Z',
  '2026-12-31T23:59:59Z', '2026-03-08T07:30:00Z', '2026-04-04T16:30:00Z', '1883-11-18T16:00:00Z',
  '1900-06-15T12:00:00Z', '0001-01-01T12:00:00Z', '0099-07-04T12:00:00Z', '0999-12-31T12:00:00Z',
  '-000001-06-01T12:00:00Z', '2026-06-15T00:04:05Z', '2026-06-15T11:04:05Z', '2026-06-15T12:04:05Z',
  '2026-06-15T23:04:05Z',
];
const NUMBERS = [
  '0', '1', '12', '123', '1234', '12345', '123456', '1234567', '1000000000', '9007199254740992', '1e21',
  '1.7976931348623157e308', '0.5', '1.5', '2.5', '1.0005', '2.0005', '999.9995', '0.0005', '0.0004', '1.2345',
  '1234.5678', '0.30000000000000004', '1e-7', '1.5e-10', '-1234.5', '-0', '-0.0004', '-0.0005', '123456789.123456',
  '1000.1', '10.05', '4.35', '1.005', '8.345', '0.07', '100', '1000000.5',
];

// `fmtMoney` in src/commands/measurement.ts.
const MONEY = { style: 'currency', currency: 'USD', maximumFractionDigits: 2 };

if (process.argv[2] === 'child') {
  console.log(
    JSON.stringify({
      locale: Intl.DateTimeFormat().resolvedOptions().locale,
      dates: INSTANTS.map((s) => {
        const date = new Date(s);
        return [date.getTime(), date.toLocaleDateString(), date.toLocaleTimeString()];
      }),
      numbers: NUMBERS.map((n) => [n, Number(n).toLocaleString()]),
      money: NUMBERS.map((n) => [n, Number(n).toLocaleString(undefined, MONEY)]),
    }),
  );
} else {
  const runs = [];
  for (const lang of LANGS) {
    for (const tz of ZONES) {
      const env = { PATH: process.env.PATH, LANG: lang, TZ: tz };
      const out = execFileSync(process.execPath, [new URL(import.meta.url).pathname, 'child'], { env });
      runs.push({ lang, tz, ...JSON.parse(out.toString()) });
    }
  }
  console.log(JSON.stringify({ node: process.version, icu: process.versions.icu, cldr: process.versions.cldr, runs }, null, 1));
}
