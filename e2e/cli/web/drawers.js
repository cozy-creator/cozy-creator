// Runs and Machines, in a real browser against a real Calcifer (web-ui.md §1): a real run from the CLI shows live with no reload, to its end;
// its log and events; a cancel from the page, which the terminal sees; one log stream across tabs; the filters. Machines: the live list,
// what's installed, a log. Everything from a package stays text, and the page's CSP holds. It rents nothing, ends nothing and updates nothing.
//   node drawers.js <sign-in link from `cozy web --no-open`> <a project's folder>
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $COZY: the CLI; $CLI_E2E: e2e/cli, where `./weightless` is. Runs on `local`,
// so on Linux. Prints what it saw as JSON and exits 1 if anything failed.
const { chromium } = require(process.env.PLAYWRIGHT);
const { execFileSync, spawn } = require('child_process');
const { STEP_MS, until } = require('./page');

const [link, folder] = process.argv.slice(2);
const failures = [];
const seen = {};
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  if (!ok) failures.push(name);
}

const cozy = (...args) => execFileSync(process.env.COZY, args, { encoding: 'utf8' });
const shown = (number) => JSON.parse(cozy('--json', 'run', 'show', String(number), '--once'));
/** A real run the terminal asks for: the CLI follows it, and exits when it ends. */
function run(seconds) {
  const child = spawn(process.env.COZY, ['run', '-p', folder, './weightless/wait', `seconds:=${seconds}`, '--on', 'local'], { cwd: process.env.CLI_E2E, stdio: 'ignore' });
  return new Promise((done) => child.on('exit', (code) => done(code)));
}
const body = (page) => page.textContent('.drawer');
const rowOf = (page, words) => page.locator('.runrow').filter({ hasText: words });
const numberIn = async (row) => Number((await row.first().locator('b').textContent()).slice(1));

(async () => {
  const browser = await chromium.launch({ executablePath: process.env.CHROME, headless: true });
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', (error) => errors.push(String(error)));
  page.on('console', (message) => {
    if (message.type() === 'error' && /Content Security Policy/.test(message.text())) errors.push(message.text());
  });

  await page.goto(link);
  await page.waitForSelector('.top', { timeout: STEP_MS });
  const id = JSON.parse(cozy('--json', 'project', '-p', folder, 'status')).id;

  // A run the CLI starts shows live, its progress moves, and it ends, with no reload.
  await page.goto(page.url().split('#')[0] + `#/projects/${id}/runs`);
  await page.waitForSelector('.drawer');
  let reloads = 0;
  page.on('load', () => { reloads += 1; });
  const first = run(20);
  const row = rowOf(page, 'wait');
  await row.first().waitFor({ timeout: STEP_MS });
  const number = await numberIn(row);
  check('run_shows_live', number > 0);
  await until('the count in the top bar', () => page.textContent('.top a[href*="runs"]'), (text) => /\d/.test(text));
  await row.first().locator('a').click();
  await page.waitForSelector('.run-head');
  await until('running', () => page.textContent('.run-head'), (text) => /running|queue/.test(text));
  check('run_names_its_machine', (await body(page)).includes('local'));
  await until('its log', () => page.textContent('.log'), (text) => text.trim().length > 0);
  await until('its end, live', () => page.textContent('.run-head'), (text) => text.includes('completed'), 120_000);
  check('run_ends_live_with_no_reload', reloads === 0);
  check('cli_agrees_it_ended', shown(number).status === 'completed', shown(number).status);
  await first;
  await until('the count goes', () => page.textContent('.top a[href*="runs"]'), (text) => !/\d/.test(text));
  check('no_hash_on_a_run', !/sha256:|[0-9a-f]{64}/.test(await body(page)));

  // Cancel, from the page, under its confirmation. The terminal's follower ends with it.
  await page.goto(page.url().split('#')[0] + `#/projects/${id}/runs`);
  const second = run(300);
  const secondNumber = await until('the second run', () => numberIn(rowOf(page, 'wait')), (now) => now > number);
  await rowOf(page, 'wait').first().locator('a').click();
  await until('it runs', () => page.textContent('.run-head'), (text) => text.includes('running'));

  // One log stream across tabs: a second tab waits, and takes over when the first lets go.
  const other = await context.newPage();
  await other.goto(page.url());
  await until('the second tab waits for the log', () => other.textContent('.drawer'), (text) => text.includes('Another tab is following'));
  check('one_log_stream_across_tabs', true);
  await other.close();

  await page.click('.drawer-head button:has-text("Cancel")');
  await page.click('dialog[open] button:has-text("Not now")');
  check('no_cancel_without_its_yes', !shown(secondNumber).cancel_requested);
  await page.click('.drawer-head button:has-text("Cancel")');
  await page.click('dialog[open] button:has-text("Cancel it")');
  await until('canceled, live', () => page.textContent('.run-head'), (text) => text.includes('canceled'), 120_000);
  check('cli_agrees_it_canceled', shown(secondNumber).status === 'canceled', shown(secondNumber).status);
  await second;

  // The filters.
  await page.goto(page.url().split('#')[0] + `#/projects/${id}/runs?status=canceled`);
  await until('only canceled', () => page.locator('.runrow').allTextContents(), (rows) => rows.length > 0 && rows.every((text) => text.includes('canceled')));
  await page.selectOption('.filters select[aria-label=Status]', '');
  await page.check('.filters input[type=checkbox]');
  await until('every project in the route', () => Promise.resolve(page.url()), (url) => url.includes('all'));
  check('filters', true);

  // Local code is the CLI's.
  await page.goto(page.url().split('#')[0] + `#/projects/${id}/new?target=${encodeURIComponent('./weightless/wait')}`);
  await page.waitForSelector('.drawer .copyable');
  check('local_code_stays_cli_only', (await page.locator('.drawer .primary').count()) === 0);

  // Machines: the live list, as `cozy machine ls` has it.
  await page.goto(page.url().split('#')[0] + `#/machines`);
  await page.waitForSelector('.runrow');
  const listed = JSON.parse(cozy('--json', 'machine', 'ls')).machines.filter((machine) => machine.state !== 'ended').map((machine) => machine.name);
  const rows = await page.locator('.runrow b').allTextContents();
  check('machines_as_the_cli_has_them', listed.every((name) => rows.includes(name)), { listed, rows });
  check('prices_are_gpu_only', !/storage|bandwidth|\/GB/i.test(await body(page)));
  await page.click('.runrow a:has-text("local")');
  await page.waitForSelector('.kv');
  check('local_has_no_end', (await page.locator('.ending button').count()) === 0);
  await page.click('summary:text-is("What\'s installed")');
  await until('what it holds', () => body(page), (text) => /wait|weightless|No packages/.test(text));
  await page.click('summary:text-is("Logs")');
  await until('a log', () => page.textContent('.log'), (text) => text.trim().length > 0);
  check('a_log_shows', true);

  check('no_page_errors', errors.length === 0, errors);
  console.log(JSON.stringify({ failures, seen }, null, 2));
  await browser.close();
  process.exit(failures.length ? 1 : 0);
})().catch((error) => {
  console.error(error);
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(2);
});
