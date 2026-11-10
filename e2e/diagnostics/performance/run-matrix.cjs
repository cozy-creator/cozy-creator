const { spawnSync } = require('node:child_process');
const { readFileSync, writeFileSync, existsSync, mkdirSync } = require('node:fs');
const { join } = require('node:path');
const order = ['current-plain', 'baseline-plain', 'baseline-profiled', 'current-profiled'];
const root = process.env.PROFILE_OUTPUT_ROOT;
mkdirSync(root, { recursive: true });
const rows = [];
let failed = false;
for (const name of order) {
  const sourceRoot = name.startsWith('current') ? process.env.PROFILE_CURRENT_ROOT : process.env.PROFILE_BASELINE_ROOT;
  const variant = name.endsWith('-profiled') ? 'profiled' : 'plain';
  const result = spawnSync(process.execPath, [join(__dirname, 'run-case.cjs')], {
    env: { ...process.env, PROFILE_CASE: name, PROFILE_FRONTEND_MANIFEST: join(sourceRoot, 'manifest.json'), PROFILE_DIST_ROOT: join(sourceRoot, variant) },
    stdio: 'inherit',
  });
  const receiptPath = join(root, name, 'case.json');
  const receipt = existsSync(receiptPath) ? JSON.parse(readFileSync(receiptPath, 'utf8')) : null;
  rows.push({ case: name, exit: result.status, signal: result.signal, receipt: Boolean(receipt), browser_closed: receipt?.cleanup?.awaited_browser_close ?? null, db_closed: receipt?.native_closed?.project_db_open === false });
  if (result.status !== 0) failed = true;
  // Never run a next workload through an unknown browser/Reader state.
  if (!receipt || receipt.cleanup?.awaited_browser_close !== true || receipt.native_closed?.project_db_open !== false) {
    const path = join(root, 'matrix.json');
    const matrix = existsSync(path) ? JSON.parse(readFileSync(path, 'utf8')) : {};
    matrix.lifecycle_blocked = true;
    matrix.attribution = 'NO regression/fix attribution: incomplete matched workload or unknown preceding lifecycle';
    writeFileSync(path, JSON.stringify(matrix, null, 2));
    failed = true;
    break;
  }
}
writeFileSync(join(root, 'execution.json'), JSON.stringify({ planned_order: order, actual: rows, automatic_retries: 0, partial_matrix: rows.length !== order.length }, null, 2));
process.exitCode = failed ? 1 : 0;
