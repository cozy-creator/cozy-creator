// The web suite (cl-973): Playwright Test, each cell one test in web/<cell>.spec.js, several at once on this job's Calcifer, each on a project
// and a browser context of its own (fixtures.js). web.sh's web_suite runs a lane of them; nothing here is retried.
// Projects: chrome (Google Chrome at $CHROME, H.264), firefox and webkit (`@engines` cells), vault (`@vault`, the job with the
// filesystem driver) and solo (`@solo`: cells that stop, restart or re-point Calcifer, or need the job to itself, one at a time in a job of
// their own, which has the filesystem driver too).
const { defineConfig } = require('@playwright/test');
const { tmpdir } = require('os');
const { join } = require('path');

const out = join(process.env.OUT || tmpdir(), 'web');
const chrome = { browserName: 'chromium', launchOptions: { executablePath: process.env.CHROME } };

module.exports = defineConfig({
  testDir: __dirname,
  testMatch: '*.spec.js',
  globalSetup: require.resolve('./setup.js'),
  outputDir: join(out, 'results'),
  // 3 browsers on a 4-vCPU runner leave a core for Calcifer, TensorD and the Hub.
  workers: 3,
  fullyParallel: true,
  retries: 0,
  forbidOnly: true,
  timeout: 180_000,
  expect: { timeout: 15_000 },
  reporter: [['list'], ['json', { outputFile: join(out, 'report.json') }]],
  use: {
    viewport: { width: 1600, height: 1000 },
    actionTimeout: 15_000,
    navigationTimeout: 30_000,
    screenshot: 'only-on-failure',
    trace: 'retain-on-failure',
  },
  projects: [
    { name: 'chrome', grepInvert: /@solo|@vault/, use: chrome },
    { name: 'firefox', grep: /@engines/, use: { browserName: 'firefox' } },
    { name: 'webkit', grep: /@engines/, use: { browserName: 'webkit' } },
    { name: 'vault', grep: /@vault/, use: chrome },
    { name: 'solo', grep: /@solo/, timeout: 300_000, use: chrome },
  ],
});
