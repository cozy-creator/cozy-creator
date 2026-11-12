// Playwright's JSON report, as web.sh's web_suite says it: `FAILED web-<cell>: <step>: <error>` for each failure, a cell of the lane that never
// ran included, then each cell's time and the 1-minute load as it ended, and the lane's wall time.
//   node report.js <report.json> <cell...>
const { readFileSync } = require('fs');
const { basename } = require('path');

const [file, ...cells] = process.argv.slice(2);
const report = JSON.parse(readFileSync(file, 'utf8'));
const plain = (text = '') => text.replace(/\u001b\[[0-9;]*m/g, '');
/** An error's words, up to its call log, code frame or stack, on one line. */
const said = (text) => {
  const lines = plain(text).split('\n');
  const end = lines.findIndex((line) => /^\s*(Call log:|>?\s*\d+ \||at )/.test(line));
  return lines.slice(0, end < 0 ? undefined : end).map((line) => line.trim()).filter(Boolean).join(' · ').slice(0, 600);
};
/** The innermost test.step whose error is the one reported (`message`, as the report formats it), as a path; with none, the cell. */
const stepOf = (steps = [], message, path = []) => {
  for (const step of steps) {
    const first = plain(step.error?.message).split('\n')[0].trim();
    if (first && plain(message).includes(first)) return stepOf(step.steps, message, [...path, step.title]);
  }
  return path.join(' › ') || 'the cell';
};

const ran = new Set();
const lines = [];
for (const error of report.errors ?? []) console.log(`FAILED web-setup: ${said(error.message)}`);
const walk = (suite) => {
  for (const spec of suite.specs ?? []) {
    const cell = basename(spec.file).replace(/\.spec\.js$/, '');
    ran.add(cell);
    for (const test of spec.tests) {
      const name = `web-${cell}${test.projectName === 'chrome' ? '' : ` (${test.projectName})`}`;
      const result = test.results.at(-1);
      const load = result?.annotations?.find((note) => note.type === 'load')?.description ?? '?';
      lines.push(`TIME ${name}: ${result ? (result.duration / 1000).toFixed(1) : '?'} s, load ${load}, ${test.status}`);
      if (test.status === 'skipped') console.log(`FAILED ${name}: skipped, which no cell is`);
      if (test.status !== 'unexpected' && test.status !== 'flaky') continue;
      for (const error of result?.errors?.length ? result.errors : [{ message: `${result?.status ?? 'no result'}` }]) console.log(`FAILED ${name}: ${stepOf(result?.steps, error.message)}: ${said(error.message)}`);
    }
  }
  for (const inner of suite.suites ?? []) walk(inner);
};
for (const suite of report.suites ?? []) walk(suite);
for (const cell of cells) if (!ran.has(cell)) console.log(`FAILED web-${cell}: in the lane, never ran: no test of its project's tags`);
for (const line of lines) console.log(line);
const { stats } = report;
if (stats) console.log(`WALL ${(stats.duration / 1000).toFixed(1)} s: ${stats.expected} passed, ${stats.unexpected} failed, ${stats.flaky} flaky, ${stats.skipped} skipped`);
