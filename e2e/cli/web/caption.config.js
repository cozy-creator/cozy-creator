// caption alone (se-151), as the cli suite's runs shard runs it (llm.sh `llm_caption`), beside its Qwen3.5 0.8B: the web
// suite's fixtures and Chrome, without its setup, which warms the local fixture tools its other cells render and none of which this one does.
const { defineConfig } = require('@playwright/test');
const { tmpdir } = require('os');
const { join } = require('path');
const { chromeArgs } = require('./chrome');

const out = join(process.env.OUT || tmpdir(), 'web');
module.exports = defineConfig({
  testDir: __dirname,
  testMatch: ['caption.spec.js'],
  grepInvert: /@gpu/,
  outputDir: join(out, 'results'),
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 900_000,
  expect: { timeout: 15_000 },
  reporter: [['list'], ['json', { outputFile: join(out, 'report.json') }]],
  use: {
    browserName: 'chromium',
    launchOptions: { executablePath: process.env.CHROME, args: chromeArgs() },
    viewport: { width: 1600, height: 1000 },
    actionTimeout: 15_000,
    navigationTimeout: 30_000,
    screenshot: 'only-on-failure',
    trace: 'retain-on-failure',
  },
});
