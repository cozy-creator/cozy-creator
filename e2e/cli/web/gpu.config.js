// The web cells' GPU branches alone, on a rental (release-gate/sessions/llm-tools.sh): llm's real models (xs-204) and machine-details'
// GPU use (cl-1100). The web suite's fixtures and Chrome, without its setup (it warms local fixture tools and reads an isolated Hub's
// seed); one at a time, nothing retried. `-g` picks one.
const { defineConfig } = require('@playwright/test');
const { tmpdir } = require('os');
const { join } = require('path');
const { chromeArgs } = require('./chrome');

const out = join(process.env.OUT || tmpdir(), 'web');
module.exports = defineConfig({
  testDir: __dirname,
  testMatch: ['llm.spec.js', 'machine-details.spec.js'],
  grep: /@gpu|machine-details: GPU use/,
  outputDir: join(out, 'results'),
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 3_600_000,
  expect: { timeout: 15_000 },
  reporter: [['list'], ['json', { outputFile: join(out, 'report.json') }]],
  use: {
    browserName: 'chromium',
    launchOptions: { executablePath: process.env.CHROME, args: chromeArgs() },
    viewport: { width: 1600, height: 1000 },
    actionTimeout: 30_000,
    navigationTimeout: 60_000,
    screenshot: 'only-on-failure',
    trace: 'off',
  },
});
