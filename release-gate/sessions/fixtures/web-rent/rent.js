// web-rent.sh's browser: Rent and End as a person does them (web-ui.md §1, decision 2), on this runner's Calcifer. Alone it spends nothing: the SKU
// list, a quote through the page, its stated price, and no order. With --rent: one click on that price, one order, followed until it's ready; it's
// used; then End, followed until the Hub affirms the pod gone. What it uses is checked and said, and End goes on whatever those found: `web-rent.sh`'s
// trap ends the pod if the page doesn't.
//   node rent.js <sign-in link from `cozy open --no-open`> [--rent]
// $PLAYWRIGHT: playwright-core's directory; $CHROME: Google Chrome; $COZY: the CLI; $SKU: the product; $SSH_KEY: an OpenSSH public key; $ORDER: where
// what the order answered goes, the moment it does; $PROJECT: a project's folder for its run; $SHOT: where its screenshots go. Prints what it saw as
// JSON and exits 1 if anything failed.
const { chromium } = require(process.env.PLAYWRIGHT);
const { execFileSync, spawn } = require('child_process');
const { writeFileSync } = require('fs');

const link = process.argv[2];
const spend = process.argv[3] === '--rent';
const sku = process.env.SKU;
const STEP_MS = 60_000;
/** A wait judged by progress: it fails when what it reads stops changing for this long, not when the whole of it takes long. */
const QUIET_MS = 15 * 60_000;
const failures = [];
const seen = {};
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  if (!ok) failures.push(name);
}
/** The CLI's answer, whatever its exit. */
function cozy(...args) {
  try {
    return execFileSync(process.env.COZY, args, { encoding: 'utf8', cwd: __dirname, stdio: ['ignore', 'pipe', 'pipe'], timeout: 600_000 });
  } catch (error) {
    if (error.stdout) return error.stdout;
    throw error;
  }
}
const mine = () => JSON.parse(cozy('--json', 'machine', 'ls')).machines.filter((machine) => machine.kind === 'rental' && machine.ordered && machine.state !== 'ended');
const pause = (ms) => new Promise((done) => setTimeout(done, ms));

async function until(what, read, wanted, ms = STEP_MS) {
  const deadline = Date.now() + ms;
  let last;
  while (Date.now() < deadline) {
    last = await read();
    if (wanted(last)) return last;
    await pause(250);
  }
  throw new Error(`${what}: still ${JSON.stringify(last)} after ${ms} ms`);
}

/** Each text it passes through is said, in `seen[what]`. */
async function whileMoving(what, read, wanted) {
  const said = (seen[what] = []);
  let moved = Date.now();
  for (;;) {
    const now = (await read()).trim();
    if (said[said.length - 1] !== now) {
      said.push(now);
      moved = Date.now();
    }
    if (wanted(now)) return now;
    if (Date.now() - moved > QUIET_MS) throw new Error(`${what}: no change for ${QUIET_MS} ms, still ${now}`);
    await pause(1000);
  }
}

/** One use of the pod. What it finds is said, and End comes whatever it found. */
async function use(name, work) {
  try {
    await work();
  } catch (error) {
    check(name, false, String(error).split('\n')[0]);
  }
}

(async () => {
  const browser = await chromium.launch({ executablePath: process.env.CHROME, headless: true });
  const page = await (await browser.newContext({ viewport: { width: 1440, height: 900 } })).newPage();
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(String(error)));
  await page.addInitScript(() => {
    window.violations = [];
    document.addEventListener('securitypolicyviolation', (e) => window.violations.push(`${e.violatedDirective}: ${e.blockedURI}`));
  });
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : undefined);
  await page.goto(link);
  await page.waitForSelector('.top');
  const base = new URL(page.url()).origin;
  const machines = `${base}/#/machines`;
  await page.goto(machines);
  await page.waitForSelector('.sku');
  await shot('skus');

  // The quote: a price stated by the page, and nothing ordered.
  const before = mine().map((machine) => machine.name);
  check('runner_starts_with_no_rental', before.length === 0, before);
  if (process.env.SSH_KEY) {
    await page.click('summary:text-is("Log in over SSH")');
    await page.fill('input[aria-label="An OpenSSH public key"]', process.env.SSH_KEY);
  }
  await page.locator('.sku').filter({ has: page.locator(`b:text-is("${sku}")`) }).locator('button:text-is("Rent")').click();
  await page.waitForSelector('.order .primary');
  await shot('quoted');
  const stated = (await page.textContent('.order .primary')).trim();
  seen.stated = stated;
  check('states_a_price', /^Rent \d+× .+ at \$\d+\.\d\d\/h$/.test(stated), stated);
  check('states_no_disk_or_traffic', !/disk|\bGB\b|storage|bandwidth/i.test(await page.textContent('.order')));
  check('the_button_is_live', await page.locator('.order .primary').isEnabled());
  check('a_quote_orders_nothing', mine().length === before.length);
  check('csp_holds_on_the_quote', (await page.evaluate(() => window.violations)).length === 0, await page.evaluate(() => window.violations));
  if (!spend || failures.length) {
    seen.spent = false;
    console.log(JSON.stringify({ failures, seen }, null, 2));
    await browser.close();
    process.exit(failures.length ? 1 : 0);
  }

  // One click on the stated price. What the order answers names the pod: nothing else is ever taken for it.
  const answered = page.waitForResponse((r) => r.request().method() === 'POST' && new URL(r.url()).pathname === '/v1/machines', { timeout: 300_000 });
  await page.dblclick('.order .primary');
  const answer = await answered;
  check('order_was_answered', answer.ok(), answer.status());
  const rented = await answer.json();
  if (!answer.ok()) throw new Error(`the order: ${JSON.stringify(rented).slice(0, 400)}`);
  const id = rented.machine.rental_id ?? rented.machine.name;
  writeFileSync(process.env.ORDER, JSON.stringify({ name: rented.machine.name, rental_id: rented.machine.rental_id ?? null }));
  seen.order = { name: rented.machine.name, rental_id: rented.machine.rental_id ?? null, replay: rented.replay };
  check('a_first_order', rented.replay === false, rented.replay);
  // An order its Hub hasn't answered yet has no rental to read.
  check('the_order_is_of_the_sku', !rented.machine.rental || rented.machine.rental.sku.toLowerCase() === sku, rented.machine.rental);
  await page.waitForURL(/#\/machines\/.+/);
  await pause(2000);
  check('one_order', mine().length === 1, mine().map((machine) => machine.name));
  const drawer = `${machines}/${encodeURIComponent(id)}`;

  // Still trying, while it is: the list's row says so with End, and the drawer does.
  await use('row_says_still_trying', async () => {
    await page.goto(machines);
    await page.waitForSelector('.runrow');
    if (await page.locator('.runrow.trying').count()) {
      await shot('trying');
      const row = await page.textContent('.runrow.trying');
      check('row_says_still_trying', /Still trying/.test(row), row);
      check('row_shows_end', (await page.locator('.runrow.trying button:text-is("End")').count()) === 1);
    }
    await page.goto(drawer);
  });

  // Its Hub may give up on a boot: an order that ended is over, and nothing is left for End to do.
  let ready = false;
  let over = false;
  await use('boots', async () => {
    const last = await whileMoving('boot', () => page.textContent('.run-head'), (text) => /^(ready|ended)/.test(text));
    ready = last.startsWith('ready');
    over = last.startsWith('ended');
    check('boots', ready, last);
  });

  if (ready) {
    await shot('ready');
    const name = await page.textContent('.drawer-head b');
    seen.name = name;
    const pod = () => JSON.parse(cozy('--json', 'machine', 'show', id));

    await use('facts', async () => {
      const text = await page.textContent('.drawer-body');
      check('facts_show_the_price', text.includes(stated.match(/\$\d+\.\d\d\/h/)[0]), text.slice(0, 300));
      check('ssh_line_to_copy', !process.env.SSH_KEY || (await page.locator('.drawer-body .copyable code').filter({ hasText: /^ssh / }).count()) === 1);
      check('no_link_to_ssh', (await page.locator('.drawer-body a[href^="ssh"]').count()) === 0);
    });

    await use('keep_up', async () => {
      await until('the idle clock', () => page.textContent('.drawer-body'), (text) => /ends in \d/.test(text), 180_000);
      const clock = pod().status.idle_deadline_ms;
      await pause(3000);
      await page.click('button:text-is("Keep up")');
      await until('the deadline moves', () => Promise.resolve(pod().status.idle_deadline_ms), (now) => now > clock);
      check('keep_up', true);
    });

    await use('installed_and_logs', async () => {
      await page.click('summary:text-is("What\'s installed")');
      await until('what it holds', () => page.textContent('.drawer-body'), (text) => !text.includes('Couldn\'t read'));
      await page.click('summary:text-is("Logs")');
      await until('its log', () => page.textContent('.log'), (text) => text.trim().length > 0 && text !== 'Nothing yet.');
      check('installed_and_logs', true);
      await shot('used');
    });

    // A real run on it, from the terminal, live in the page by machine, to its end.
    await use('run_on_it', async () => {
      let out = '';
      let exited = null;
      const child = spawn(process.env.COZY, ['run', '-p', process.env.PROJECT, './words.py/count', 'text=three small words', '--on', name], { cwd: __dirname });
      child.stdout.on('data', (data) => { out += data; });
      child.stderr.on('data', (data) => { out += data; });
      child.on('exit', (code) => { exited = code; });
      await page.goto(`${base}/#/runs?all&machine=${encodeURIComponent(name)}`);
      await until('its row, live', () => page.locator('.runrow').count(), (count) => count > 0, 300_000);
      check('run_shows_live_on_its_machine', true);
      await shot('run');
      await whileMoving('run', async () => `${exited}:${out.length}`, (text) => !text.startsWith('null:'));
      seen.run_said = out.slice(-400);
      check('run_ended_well', exited === 0, exited);
      // The last it names is its own, after local code's install.
      const number = [...out.matchAll(/^run (\d+)$/gm)].pop()?.[1] ?? (await page.locator('.runrow b').first().textContent()).slice(1);
      await page.goto(`${base}/#/runs/${number}`);
      await until('its end in the page', () => page.textContent('.run-head'), (text) => text.includes('completed'));
      await page.click('summary:text-is("Result")');
      check('its_result_shows', (await page.textContent('.drawer-body')).includes('words'));
      await shot('run-done');
    });
  }

  if (!over) {
    // End, through the page: what it says, what it waits for, and the Hub's word.
    await page.goto(drawer);
    await page.reload();
    await page.waitForSelector('.ending button:text-is("End")');
    await page.click('.ending button:text-is("End")');
    await page.waitForSelector('.confirm');
    seen.end_says = (await page.textContent('.confirm .says')).trim();
    check('end_says_what_bills', /bills \$\d+\.\d\d\/h until the Hub affirms it gone/.test(seen.end_says), seen.end_says);
    await shot('end');
    await page.click('.confirm button:has-text("End it")');
    // What it says of the end, and not the facts below it, whose clock would read as progress.
    const ended = await whileMoving('ending', () => page.evaluate(() => [...document.querySelectorAll('.run-head, .ending, .drawer-body > p')].map((el) => el.innerText).join(' | ')), (text) => /nothing of it bills now/i.test(text));
    // A pod that's gone can't answer, and that's no news (42018791462).
    check('ended_is_all_it_says', !/didn't answer/.test(ended), ended);
    await shot('ended');
    check('the_page_says_ended_on_the_hubs_word', true);
  }
  const record = JSON.parse(cozy('--json', 'machine', 'show', id));
  check('cli_agrees_it_ended', record.state === 'ended', record.state);
  check('nothing_left_of_it', mine().length === 0, mine().map((machine) => machine.name));
  seen.spent_said = record.rental?.spend_usd_micros;

  check('csp_holds', (await page.evaluate(() => window.violations)).length === 0, await page.evaluate(() => window.violations));
  check('no_page_errors', errors.length === 0, errors);
  console.log(JSON.stringify({ failures, seen }, null, 2));
  await browser.close();
  process.exit(failures.length ? 1 : 0);
})().catch((error) => {
  console.error(error);
  failures.push(String(error).split('\n')[0]);
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(2);
});
