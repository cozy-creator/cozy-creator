// Runs and Machines, as a person uses them (web-ui.md §1). Where this computer runs models: a real run from the CLI shows live with no reload,
// to its end; its log and events; a cancel from the page, which the terminal sees; one log stream across tabs. Everywhere: the filters, local
// code staying the CLI's, machines as the CLI lists them, what's installed, a log, and the page's CSP holding. It rents, ends and updates nothing.
//   node drawers.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $COZY: the CLI; $SHOT: where to keep its screenshots; $ALONE: on a shared
// computer, so it runs nothing. Prints what it saw as JSON and exits 1 if anything failed.
const { chromium } = require(process.env.PLAYWRIGHT);
const { execFileSync, spawn } = require('child_process');
const { join } = require('path');
const { cozyIn, signIn, STEP_MS, noStall, until } = require('./page');

const failures = [];
const seen = {};
noStall(seen, died);
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  if (!ok) failures.push(name);
}

const here = join(__dirname, '..');
/** The CLI's answer, whatever its exit: a canceled run's `show` exits 1 and still says so. */
function cozy(...args) {
  try {
    return execFileSync(process.env.COZY, args, { encoding: 'utf8', cwd: here, stdio: ['ignore', 'pipe', 'pipe'], timeout: 600_000 });
  } catch (error) {
    if (error.stdout) return error.stdout;
    throw error;
  }
}
const shown = (number) => JSON.parse(cozy('--json', 'run', 'show', String(number), '--once'));
const body = (page) => page.textContent('.drawer');
const rowOf = (page, words) => page.locator('.runrow').filter({ hasText: words });
const numberIn = async (row) => Number((await row.first().locator('b').textContent()).slice(1));

async function runs(page, base, id, folder) {
  /** A real run the terminal asks for: the CLI follows it, and exits when it ends. */
  const asked = (seconds) => {
    const child = spawn(process.env.COZY, ['run', join(here, 'weightless/wait'), `seconds:=${seconds}`, '--on', 'local'], { cwd: folder, stdio: 'ignore' });
    return new Promise((done) => child.on('exit', done));
  };
  // Local code installs on first use, as its own run: past that, a run of it is one row.
  cozyIn(folder, ['run', join(here, 'weightless/reverse'), 'text=warm', '--on', 'local'], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], timeout: 600_000 });

  await page.goto(`${base}/#/projects/${id}/runs`);
  await page.waitForSelector('.drawer');
  let reloads = 0;
  page.on('load', () => { reloads += 1; });
  const first = asked(20);
  const row = rowOf(page, 'wait');
  await row.first().waitFor({ timeout: STEP_MS });
  const number = await numberIn(row);
  check('run_shows_live', number > 0, number);
  await until('the count in the top bar', () => page.textContent('.top a[href*="runs"]'), (text) => /\d/.test(text));
  await row.first().locator('a').click();
  await page.waitForSelector('.run-head');
  await until('running', () => page.textContent('.run-head'), (text) => /running|queue/.test(text));
  check('run_names_its_machine', (await body(page)).includes('local'));
  await until('its log', () => page.textContent('.log'), (text) => text.trim().length > 0 && text !== 'Nothing yet.', 120_000);
  await until('its end, live', () => page.textContent('.run-head'), (text) => text.includes('completed'), 120_000);
  check('run_ends_live_with_no_reload', reloads === 0, reloads);
  check('cli_agrees_it_ended', shown(number).status === 'completed', shown(number).status);
  await first;
  await until('the count goes', () => page.textContent('.top a[href*="runs"]'), (text) => !/\d/.test(text)).catch(async (error) => {
    // Whose it is: the page's, or a run Calcifer says is going.
    seen.count_held = await page.evaluate(async () => {
      const { token } = JSON.parse(localStorage.getItem('cozy.session'));
      const going = async (status) => (await (await fetch(`/v1/runs?status=${status}&limit=5`, { headers: { Authorization: `Bearer ${token}` } })).json()).runs.map((run) => [run.number, run.status]);
      return { in_progress: await going('in_progress'), queued: await going('queued') };
    });
    throw error;
  });
  check('no_hash_on_a_run', !/sha256:|[0-9a-f]{64}/.test(await body(page)));
  if (process.env.SHOT) await page.screenshot({ path: `${process.env.SHOT}-run.png` });

  // A cancel, from the page, under its confirmation: the terminal's follower ends with it.
  await page.goto(`${base}/#/projects/${id}/runs`);
  const second = asked(300);
  const secondNumber = await until('the second run', () => numberIn(rowOf(page, 'wait')), (now) => now > number);
  await rowOf(page, 'wait').first().locator('a').click();
  await until('it runs', () => page.textContent('.run-head'), (text) => text.includes('running'), 120_000);

  // One log stream across tabs: a second tab says it waits, while this one holds it.
  await until('this tab holds the log', () => page.textContent('.log'), (text) => text !== 'Nothing yet.' && !text.includes('Another tab'), 30_000).catch(() => {});
  const other = await page.context().newPage();
  await other.goto(page.url());
  await until('the second tab waits for the log', () => other.textContent('.drawer'), (text) => text.includes('Another tab is following'));
  check('one_log_stream_across_tabs', true);
  await other.close();

  await page.click('.drawer-head button:has-text("Cancel")');
  await page.click('.confirm button:has-text("Not now")');
  check('no_cancel_without_its_yes', !shown(secondNumber).cancel_requested);
  await page.click('.drawer-head button:has-text("Cancel")');
  await page.click('.confirm button:has-text("Cancel it")');
  await until('canceled, live', () => page.textContent('.run-head'), (text) => text.includes('canceled'), 120_000);
  check('cli_agrees_it_canceled', shown(secondNumber).status === 'canceled', shown(secondNumber).status);
  await second;

  await page.goto(`${base}/#/projects/${id}/runs?status=canceled`);
  await until('only canceled', () => page.locator('.runrow').allTextContents(), (rows) => rows.length > 0 && rows.every((text) => text.includes('canceled')));
  check('filter_by_status', true);
}

(async () => {
  const browser = await chromium.launch({ executablePath: process.env.CHROME, headless: true });
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', (error) => errors.push(String(error)));
  await page.addInitScript(() => {
    window.violations = [];
    document.addEventListener('securitypolicyviolation', (e) => window.violations.push(`${e.violatedDirective}: ${e.blockedURI}`));
  });

  await signIn(page);
  await page.waitForSelector('.top', { timeout: STEP_MS });
  const base = new URL(page.url()).origin;

  // A project of its own, by name, from the Projects drawer.
  await page.goto(`${base}/#/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-drawers-${Date.now()}`);
  await page.click('form button[type=submit]');
  await page.waitForURL(/#\/projects\/[^/]+$/);
  const id = page.url().match(/#\/projects\/([^/]+)/)[1];
  const read = (path) =>
    page.evaluate(async (path) => {
      const { token } = JSON.parse(localStorage.getItem('cozy.session'));
      return (await fetch(path, { headers: { Authorization: `Bearer ${token}` } })).json();
    }, path);
  const folder = (await read(`/v1/projects/${id}`)).path;
  seen.project = { id, folder };

  const runsHere = (await read('/v1/status')).local_usable && !process.env.ALONE;
  seen.ran = runsHere;
  if (runsHere) await runs(page, base, id, folder);

  // The filters: every project goes in the route.
  await page.goto(`${base}/#/projects/${id}/runs`);
  await page.click('.filters input[type=checkbox]');
  await until('every project in the route', () => Promise.resolve(page.url()), (url) => url.includes('all'));
  check('filter_every_project', true);

  // Local code is the CLI's: it says so, and offers no button to run it.
  await page.goto(`${base}/#/projects/${id}/new?target=${encodeURIComponent('./weightless/wait')}`);
  await page.waitForSelector('.drawer .copyable');
  check('local_code_stays_cli_only', (await page.locator('.drawer .primary').count()) === 0);

  // Machines: the live list, as `cozy machine ls` has it.
  await page.goto(`${base}/#/machines`);
  await page.waitForSelector('.sect');
  const listed = JSON.parse(cozy('--json', 'machine', 'ls')).machines.filter((machine) => machine.state !== 'ended').map((machine) => machine.name);
  await until('the machines', () => page.locator('.runrow b').allTextContents(), (rows) => listed.every((name) => rows.includes(name)));
  check('machines_as_the_cli_has_them', true);
  check('prices_are_gpu_only', !/storage|bandwidth|\/GB/i.test(await body(page)));
  if (process.env.SHOT) await page.screenshot({ path: `${process.env.SHOT}-machines.png` });
  if (listed.includes('local')) {
    await page.click('.runrow a:has-text("local")');
    await page.click('summary:text-is("Details")');
    await page.waitForSelector('.kv');
    check('local_has_no_end', (await page.locator('.ending button').count()) === 0);
    if (runsHere) {
      await page.click('summary:text-is("What\'s installed")');
      await until('what it holds', async () => (await page.locator('.heldrow').count()) > 0 || (await body(page)).includes('No packages'), Boolean);
      await page.click('summary:text-is("Logs")');
      await until('a log', () => page.textContent('.log'), (text) => text.trim().length > 0 && text !== 'Nothing yet.');
      check('a_log_shows', true);
      if (process.env.SHOT) await page.screenshot({ path: `${process.env.SHOT}-local.png` });
    }
  }

  check('csp_holds', (await page.evaluate(() => window.violations)).length === 0, await page.evaluate(() => window.violations));
  check('no_page_errors', errors.length === 0, errors);
  console.log(JSON.stringify({ failures, seen }, null, 2));
  await browser.close();
  process.exit(failures.length ? 1 : 0);
})().catch(died);

async function died(error) {
  console.error(error);
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(2);
}
