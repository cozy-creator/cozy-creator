// CI setup only. Build once before browser/performance measurement, then reuse the
// native test executable; no runtime endpoint, old-format loader or CRDT codec.
const { execFileSync } = require('child_process');
const { randomUUID } = require('crypto');
const { readFileSync, rmSync, writeFileSync } = require('fs');
const { dirname, join, resolve } = require('path');

const root = resolve(__dirname, '../../..');
let producer;
let source;

function prepareCurrentSeed() {
  if (process.env.GITHUB_ACTIONS !== 'true') throw new Error('current native web fixture setup runs only in ephemeral CI');
  if (producer) return;
  console.error('Preparing current native web fixture producer (--release)');
  const output = execFileSync('cargo', ['test', '--release', '--locked', '-p', 'calcifer', '--lib', '--no-run', '--message-format=json'], {
    cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'inherit'], maxBuffer: 64 << 20,
  });
  for (const line of output.split('\n')) {
    if (!line.trim()) continue;
    const item = JSON.parse(line);
    if (item.reason === 'compiler-artifact' && item.target?.name === 'calcifer' && item.profile?.test && item.executable) producer = item.executable;
  }
  if (!producer) throw new Error('Cargo did not identify the current native fixture executable');
  source = execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).trim();
}

function seedCurrentProject(folder, seed) {
  prepareCurrentSeed();
  const input = join(dirname(folder), `.current-web-seed-${randomUUID()}.json`);
  const receipt = `${input}.receipt`;
  writeFileSync(input, JSON.stringify({ ...seed, directory: resolve(folder), receipt, files: {} }), { flag: 'wx', mode: 0o600 });
  try {
    execFileSync(producer, ['--ignored', '--exact', 'project::web_seed_tests::seed_current_web_project', '--nocapture'], {
      cwd: root, env: { ...process.env, COZY_WEB_SEED: input }, stdio: ['ignore', 'pipe', 'pipe'], maxBuffer: 4 << 20,
    });
    const result = JSON.parse(readFileSync(receipt, 'utf8'));
    if (result.schema !== 2 || result.crdt_format !== 1 || result.assets !== seed.expected_assets || result.versions !== seed.expected_versions || result.canvas !== seed.expected_assets || result.measured !== Object.keys(seed.measured).length || result.local_file_rows !== 0) throw new Error('current native fixture population differs from its requested workload');
    return { ...result, producer_source: source };
  } finally {
    rmSync(input, { force: true });
    rmSync(receipt, { force: true });
  }
}

module.exports = { prepareCurrentSeed, seedCurrentProject };
