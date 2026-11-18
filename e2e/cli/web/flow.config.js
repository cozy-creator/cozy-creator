// user-flow.spec.js alone (xs-203; release-gate/sessions/user-flow.sh), or another session's spec ($FLOW_SPEC: shared-download.sh's): the
// web suite's fixtures and Chrome, without its setup, which warms local fixture tools and reads an isolated Hub's seed. One test, hours
// long, on real rentals: nothing is retried.
const { defineConfig } = require('@playwright/test');
const { tmpdir } = require('os');
const { join } = require('path');
const { chromeArgs } = require('./chrome');

const out = join(process.env.OUT || tmpdir(), 'web');
module.exports = defineConfig({
  testDir: __dirname,
  testMatch: process.env.FLOW_SPEC || 'user-flow.spec.js',
  outputDir: join(out, 'results'),
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 4 * 3600_000,
  expect: { timeout: 15_000 },
  reporter: [['list'], ['json', { outputFile: join(out, 'report.json') }]],
  use: {
    browserName: 'chromium',
    launchOptions: { executablePath: process.env.CHROME, args: chromeArgs() },
    viewport: { width: 1600, height: 1000 },
    actionTimeout: 30_000,
    navigationTimeout: 60_000,
    screenshot: 'only-on-failure',
    // Off: a trace keeps what was typed, the fixture's passphrase too, and the artifact is public.
    trace: 'off',
  },
});
