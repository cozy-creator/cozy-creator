// walkthrough.spec.js (release-gate/sessions/walkthrough.sh): the page as a new user meets it, in Chrome and WebKit. Nothing retried.
const { defineConfig } = require('@playwright/test');
const { tmpdir } = require('os');
const { join } = require('path');
const { chromeArgs } = require('./chrome');

const out = join(process.env.OUT || tmpdir(), 'web');
module.exports = defineConfig({
  testDir: __dirname,
  testMatch: 'walkthrough.spec.js',
  outputDir: join(out, 'results'),
  workers: 1,
  retries: 0,
  timeout: 3 * 3600_000,
  expect: { timeout: 15_000 },
  reporter: [['list'], ['json', { outputFile: join(out, 'report.json') }]],
  use: { viewport: { width: 1600, height: 1000 }, actionTimeout: 20_000, navigationTimeout: 60_000, trace: 'off' },
  projects: [
    { name: 'chrome', use: { browserName: 'chromium', launchOptions: { executablePath: process.env.CHROME, args: chromeArgs() } } },
    { name: 'webkit', use: { browserName: 'webkit' } },
  ],
});
