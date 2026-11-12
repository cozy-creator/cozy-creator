// Once a job, before any cell (playwright.config.js's globalSetup): Calcifer answers, the local fixture tools the cells render are warm on
// local, and the Hub lists what isolated.sh seeded. `$OUT/web/fixtures.json` says what was warmed, in how long, and the seed's tool ids.
const { execFile } = require('child_process');
const { appendFileSync, mkdirSync, readFileSync, writeFileSync } = require('fs');
const { join } = require('path');

/** Each local fixture tool a cell renders, with inputs it runs on: warmed on local two at a time. A cell's tool goes here. Only Linux runs
 *  models on its own machine (lib.sh's LOCAL): macOS and Windows warm nothing. */
const WARM = process.platform === 'linux' ? [
  ['draw.py/draw', 'instructions=warm'],
] : [];
/** What isolated.sh publishes at the root for every web job. */
const SEED = ['qwen-image-2', 'minimax-h3', 'sdxl', 'birefnet', 'moss-image', 'e2e-boxed'];

const need = (name) => process.env[name] || (() => { throw new Error(`$${name} is not set: web.sh's web_suite sets it`); })();

module.exports = async () => {
  const out = join(need('OUT'), 'web');
  mkdirSync(out, { recursive: true });
  const holder = JSON.parse(readFileSync(need('HOLDER'), 'utf8'));
  const origin = `http://${holder.address}`;
  const ask = async (method, path, body) => {
    const answer = await fetch(origin + path, {
      method,
      headers: { Authorization: `Bearer ${holder.token}`, 'cozy-project-storage': '3', 'Content-Type': 'application/json' },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(60_000),
    });
    const text = await answer.text();
    if (!answer.ok) throw new Error(`${method} ${path}: ${answer.status} ${text.slice(0, 500)}`);
    return text ? JSON.parse(text) : null;
  };
  /** `read` until `wanted` holds of what it says, or `ms` is up: what it last said either way. */
  const until = async (read, wanted, ms) => {
    const deadline = Date.now() + ms;
    for (;;) {
      const said = await read().catch((error) => ({ error: error.message }));
      if (wanted(said) || Date.now() > deadline) return said;
      await new Promise((done) => setTimeout(done, 250));
    }
  };
  const began = Date.now();

  const status = await until(() => ask('GET', '/v1/status'), (said) => !said.error, 120_000);
  if (status.error) throw new Error(`Calcifer at ${origin} doesn't answer: ${status.error}`);

  // Warmed in a project of the setup's own, as `cozy run` in a person's project folder.
  const warmAt = join(out, 'web-warm');
  await ask('POST', '/v1/projects', { path: warmAt, unencrypted: true });
  const cozy = (words) => new Promise((done) => {
    if (process.env.ARGV_LOG) appendFileSync(process.env.ARGV_LOG, `${words.map((word) => `${word}\x1f`).join('')}\x1e`);
    const started = Date.now();
    execFile(need('COZY'), words, { cwd: warmAt, maxBuffer: 1 << 26, timeout: 600_000 }, (error, stdout, stderr) => done({ ms: Date.now() - started, code: error ? error.code ?? 1 : 0, said: `${stdout}${stderr}`.slice(-600) }));
  });
  const warmed = {};
  const left = [...WARM];
  await Promise.all([0, 1].map(async () => {
    for (let next; (next = left.shift()); ) {
      const [tool, ...inputs] = next;
      warmed[tool] = await cozy(['run', join(__dirname, '..', 'project', tool), ...inputs, '--on', 'local']);
    }
  }));
  const cold = Object.entries(warmed).filter(([, run]) => run.code !== 0);
  if (cold.length) throw new Error(`fixture tools didn't run on local: ${cold.map(([tool, run]) => `${tool}: exit ${run.code}: ${run.said}`).join('; ')}`);

  // The Hub's packages, as the palette lists them: a Hub that's late says so in `note`.
  const listed = await until(() => ask('GET', '/v1/tools'), (said) => !said.error && !said.note && SEED.every((name) => said.tools.some((tool) => tool.id.startsWith(`${name}/`))), 120_000);
  if (listed.error) throw new Error(`GET /v1/tools: ${listed.error}`);
  const hub = Object.fromEntries(SEED.map((name) => [name, listed.tools.filter((tool) => tool.id.startsWith(`${name}/`)).map((tool) => tool.id)]));
  const missing = SEED.filter((name) => !hub[name].length);
  if (missing.length) throw new Error(`the Hub lists none of ${missing.join(', ')}${listed.note ? ` (${listed.note})` : ''}: ${listed.tools.map((tool) => tool.id).join(' ')}`);

  writeFileSync(join(out, 'fixtures.json'), JSON.stringify({ setup_ms: Date.now() - began, warmed, hub }, null, 2));
  console.log(`web setup: ${((Date.now() - began) / 1000).toFixed(1)} s; warmed ${Object.entries(warmed).map(([tool, run]) => `${tool} ${(run.ms / 1000).toFixed(1)} s`).join(', ')}`);
};
