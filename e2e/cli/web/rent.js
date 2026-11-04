// Rent and End, in a real browser against a real Calcifer (web-ui.md §1, decision 2). Alone it spends nothing: it quotes $SKU and prints the
// stated price. With --rent it spends real money: one click on that price, one order, followed until it's ready; then End, followed until the
// Hub affirms the pod gone, which `cozy machine ls` then agrees with. Whatever fails after the order, it ends the machine from the CLI and
// says so. Each --rent needs the lead's OK for that pod (agent-tools/AGENT-RULES.md).
//   node rent.js <sign-in link from `cozy web --no-open`> [--rent]
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $COZY: the CLI; $SKU: the product, `cpu` unless named. Prints JSON; exits 1 if anything failed.
const { chromium } = require(process.env.PLAYWRIGHT);
const { execFileSync } = require('child_process');
const { STEP_MS, until } = require('./page');

const link = process.argv[2];
const spend = process.argv[3] === '--rent';
const sku = process.env.SKU || 'cpu';
/** A wait judged by progress: it fails when `read` stops changing for this long, not when the whole of it takes long. */
const QUIET_MS = 15 * 60_000;
const failures = [];
const seen = {};
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  if (!ok) failures.push(name);
}
const cozy = (...args) => execFileSync(process.env.COZY, args, { encoding: 'utf8' });
const live = () => JSON.parse(cozy('--json', 'machine', 'ls')).machines.filter((machine) => machine.state !== 'ended');

async function whileMoving(what, read, wanted) {
  let last, moved = Date.now();
  for (;;) {
    const now = await read();
    if (wanted(now)) return now;
    if (now !== last) {
      last = now;
      moved = Date.now();
    }
    if (Date.now() - moved > QUIET_MS) throw new Error(`${what}: no change for ${QUIET_MS} ms, still ${now}`);
    await new Promise((done) => setTimeout(done, 1000));
  }
}

(async () => {
  const browser = await chromium.launch({ executablePath: process.env.CHROME, headless: true });
  const page = await (await browser.newContext({ viewport: { width: 1440, height: 900 } })).newPage();
  await page.goto(link);
  await page.waitForSelector('.top', { timeout: STEP_MS });
  await page.goto(page.url().split('#')[0] + '#/machines');
  await page.waitForSelector('.sku', { timeout: STEP_MS });

  const before = live().map((machine) => machine.name);
  await page.locator('.sku').filter({ has: page.locator(`b:text-is("${sku}")`) }).locator('button:text-is("Quote")').click();
  await page.waitForSelector('.order .primary', { timeout: STEP_MS });
  const stated = await page.textContent('.order .primary');
  seen.stated = stated;
  check('states_a_price', /^Rent \d+× .+ at \$\d+\.\d\d\/h$/.test(stated), stated);
  check('states_no_disk_or_traffic', !/disk|GB|storage|bandwidth/i.test(await page.textContent('.order')));
  check('a_quote_orders_nothing', JSON.stringify(live().map((machine) => machine.name)) === JSON.stringify(before));
  if (!spend) {
    console.log(JSON.stringify({ failures, seen }, null, 2));
    await browser.close();
    process.exit(failures.length ? 1 : 0);
  }

  // Yours is what this order returned: the route it lands on names it by its rental id, and nothing else is ever ended.
  let mine = null;
  try {
    await page.dblclick('.order .primary');
    await until('its drawer', () => Promise.resolve(page.url()), (url) => url.includes('/machines/') && !url.endsWith('/machines/'), 120_000);
    mine = decodeURIComponent(page.url().split('/machines/')[1]);
    seen.machine = mine;
    await page.waitForTimeout(2000);
    const fresh = live().filter((machine) => !before.includes(machine.name));
    check('one_order', fresh.length === 1, fresh.map((machine) => machine.name));
    await whileMoving('booting', () => page.textContent('.run-head'), (text) => /\bready\b/.test(text));
    check('ready', true);
    // End: what it waits for, then the Hub's word.
    await page.click('.ending button:text-is("End")');
    await page.waitForSelector('dialog[open]');
    seen.end_says = await page.textContent('dialog[open] .says');
    check('end_says_what_bills', /bills \$\d+\.\d\d\/h until the Hub affirms it gone/.test(seen.end_says), seen.end_says);
    await page.click('dialog[open] button:has-text("End it")');
    await whileMoving('ending', () => page.textContent('.drawer-body'), (text) => /nothing of it bills now/.test(text));
    check('the_page_says_ended_only_on_the_hubs_word', true);
    const gone = live().filter((machine) => machine.rental_id === mine || machine.name === mine);
    check('cli_agrees_it_is_gone', gone.length === 0, gone);
    mine = null;
  } finally {
    if (mine) {
      seen.ended_from_the_cli = cozy('machine', 'end', mine).trim();
      failures.push('ended_from_the_cli');
    }
  }
  console.log(JSON.stringify({ failures, seen }, null, 2));
  await browser.close();
  process.exit(failures.length ? 1 : 0);
})().catch((error) => {
  console.error(error);
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(2);
});
