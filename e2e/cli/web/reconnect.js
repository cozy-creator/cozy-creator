// One stream across tabs, and across a restart of Calcifer (web-ui.md §3): tabs share one `GET /v1/events`, closing the tab that holds
// it hands it on, and `kill -9` on Calcifer says it's offline and recovers with no reload, the session intact. A reload after the
// restart must not hang, which it did in the shelved build.
//   node reconnect.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $RESTART: a shell command that kills this Calcifer with -9 and
// brings it back. Prints what it saw as JSON and exits 1 if anything failed.
const { chromium } = require(process.env.PLAYWRIGHT);
const { exec } = require('child_process');
const { calcifer, signIn, STEP_MS, drop, png, noStall, until } = require('./page');

const failures = [];
const seen = {};
noStall(seen, died);
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  if (!ok) failures.push(name);
}

const place = (page, name) => page.locator('.react-flow__node').filter({ has: page.locator(`.nm:text-is("${name}")`) }).evaluate((node) => getComputedStyle(node).transform);

async function drag(page, name, dx, dy) {
  const box = await page.locator('.react-flow__node').filter({ has: page.locator(`.nm:text-is("${name}")`) }).locator('.nm').boundingBox();
  const [x, y] = [box.x + box.width / 2, box.y + box.height / 2];
  await page.bringToFront();
  await page.mouse.move(x, y);
  await page.mouse.down();
  await page.mouse.move(x + 2, y);
  await page.mouse.move(x + dx, y + dy, { steps: 4 });
  await page.mouse.up();
}

(async () => {
  const base = calcifer();
  const browser = await chromium.launch({ executablePath: process.env.CHROME, headless: true });
  const context = await browser.newContext({ viewport: { width: 1400, height: 800 } });
  context.setDefaultTimeout(STEP_MS);
  const errors = [];
  let streams = 0;
  context.on('request', (request) => { if (new URL(request.url()).pathname === '/v1/events') streams += 1; });
  const open = async (url) => {
    const page = await context.newPage();
    page.on('pageerror', (error) => errors.push(error.message));
    await (url ? page.goto(url) : signIn(page));
    return page;
  };

  // A project of its own with one element on it.
  const first = await open();
  await first.waitForSelector('.top');
  await first.goto(`${base}/#/projects`);
  await first.fill('input[aria-label="New project\'s name"]', `web-reconnect-${Date.now()}`);
  await first.click('form button[type=submit]');
  await first.waitForURL(/#\/projects\/[^/]+$/);
  const url = first.url();
  const id = url.match(/#\/projects\/([^/]+)/)[1];
  const folder = (await first.evaluate(async (id) => {
    const { token } = JSON.parse(localStorage.getItem('cozy.session'));
    return (await fetch(`/v1/projects/${id}`, { headers: { Authorization: `Bearer ${token}` } })).json();
  }, id)).path;
  await drop(first, [{ name: 'Here.png', type: 'image/png', bytes: png(90) }], 600, 300);
  await first.waitForSelector('.nm-field');
  await first.keyboard.press('Enter');
  seen.project = { id, folder };
  // Its row as Calcifer answers it (cl-803: the canvas is project.db's), asked from here.
  const { token } = JSON.parse(await first.evaluate(() => localStorage.getItem('cozy.session')));
  const row = async (name) => {
    const card = (await (await fetch(`${base}/v1/projects/${id}/canvas`, { headers: { Authorization: `Bearer ${token}` } })).json()).cards?.[name];
    return card ? `${card.x},${card.y}` : null;
  };
  await until('the element landed', () => first.locator('.nm:text-is("here")').count().then(async (n) => n === 1 && (await first.locator('.card .status.mak').count()) === 0), Boolean);

  // Two tabs, one stream: a move in one reaches the other by it.
  const streamsAlone = streams;
  const second = await open(url);
  await second.waitForSelector('.nm:text-is("here")');
  const before = await place(second, 'here');
  await drag(first, 'here', 120, 80);
  await until('the other tab follows', () => place(second, 'here'), (now) => now !== before);
  check('two_tabs_one_stream', streams === streamsAlone, { streamsAlone, streams });

  // The tab holding it goes: the other takes it, and a third tab's move still arrives.
  await first.close();
  await until('the stream handed on', () => Promise.resolve(streams), (n) => n === streamsAlone + 1);
  const third = await open(url);
  await third.waitForSelector('.nm:text-is("here")');
  const atHandover = await place(second, 'here');
  await drag(third, 'here', -60, 50);
  await until('followed after the handover', () => place(second, 'here'), (now) => now !== atHandover);
  check('handover', true);

  if (process.env.RESTART) {
    // kill -9: offline, in the top bar; then back with no reload and the session intact.
    const held = await third.evaluate(() => localStorage.getItem('cozy.session'));
    const restarted = new Promise((done, fail) => exec(process.env.RESTART, { shell: 'bash' }, (error) => (error ? fail(error) : done())));
    const said = await until('it said offline', () => third.locator('.pill.warn').allTextContents(), (lines) => lines.some((line) => line.startsWith('Calcifer offline')));
    await restarted;
    await until('back', () => third.locator('.pill.warn').allTextContents(), (lines) => !lines.some((line) => line.includes('offline')));
    seen.said_during_restart = said;
    check('no_sign_in_needed', (await third.locator('.gate').count()) === 0 && (await third.evaluate(() => localStorage.getItem('cozy.session'))) === held);
    check('still_on_canvas', (await third.locator('.nm:text-is("here")').count()) === 1);
    const wasRow = await row('here');
    await drag(third, 'here', 90, 40);
    await until('a move lands after the restart', () => row('here'), (now) => now !== wasRow);
    await until('the other tab still follows', () => second.locator('.pill.warn').allTextContents(), (lines) => !lines.some((line) => line.includes('offline')));
    await third.reload();
    await third.waitForSelector('.nm:text-is("here")');
    check('reload_after_restart', true);
  }

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
