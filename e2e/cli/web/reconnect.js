// One stream across tabs, and across a restart of Calcifer (web-ui.md §3): tabs share one `GET /v1/events`, closing the tab that holds
// it hands it on, and `kill -9` on Calcifer says it's offline and recovers with no reload, the session intact. A reload after the
// restart must not hang, which it did in the shelved build.
//   node reconnect.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $RESTART: a shell command that kills this Calcifer with -9 and
// brings it back. Prints what it saw as JSON and exits 1 if anything failed.
const { chromium } = require(process.env.PLAYWRIGHT);
const { exec } = require('child_process');
const { projectFolder, projectFromPage, calcifer, signIn, STEP_MS, drop, png, noStall, until } = require('./page');

const { canvasBox, canvasView, hoverBox } = require('./page');

const failures = [];
const seen = {};
noStall(seen, died);
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  if (!ok) failures.push(name);
}

const place = async (page, name) => {
  const box = (await canvasBox(page, name)).bounds, view = await canvasView(page);
  const origin = await page.locator('.canvas-root').boundingBox();
  return `${Math.round((box.x - origin.x - view.x) / view.zoom)},${Math.round((box.y - origin.y - view.y) / view.zoom)}`;
};

async function drag(page, name, dx, dy) {
  await page.bringToFront();
  const box = (await until(`${name} on the canvas`, () => canvasBox(page, name), Boolean)).title;
  const [x, y] = [box.x + box.width / 2, box.y + box.height / 2];
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
    await signIn(page, url);
    return page;
  };

  // A project of its own with one element on it.
  const first = await open();
  await first.waitForSelector('.top');
  await first.goto(`${base}/projects`);
  await first.fill('input[aria-label="New project\'s folder"]', projectFolder(`web-reconnect-${Date.now()}`));
  await first.getByLabel("Create without encryption", { exact: true }).check();
  await first.click('form button[type=submit]');
  await first.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const url = first.url();
  const id = (await projectFromPage(first)).id;
  const folder = (await first.evaluate(async (id) => {
    const { token } = JSON.parse(sessionStorage.getItem('cozy.session'));
    return (await fetch(`/v1/projects/${id}`, { headers: { Authorization: `Bearer ${token}`, 'cozy-project-storage': '3' } })).json();
  }, id)).path;
  await drop(first, [{ name: 'Here.png', type: 'image/png', bytes: png(90) }], 600, 300);
  await first.waitForSelector('.nm-field');
  await first.keyboard.press('Enter');
  seen.project = { id, folder };
  // Its row as Calcifer answers it (cl-803: the canvas is project.db's), asked from here.
  const { token } = JSON.parse(await first.evaluate(() => sessionStorage.getItem('cozy.session')));
  const row = async (name) => {
    const card = (await (await fetch(`${base}/v1/projects/${id}/canvas`, { headers: { Authorization: `Bearer ${token}`, 'cozy-project-storage': '3' } })).json()).cards?.[name];
    return card ? `${card.x},${card.y}` : null;
  };
  await until('the element landed', async () => Boolean(await canvasBox(first, 'here')) && Boolean(await row('here')), Boolean);
  const native = await hoverBox(first, 'here');
  await until('the import no longer pending', () => native.locator('.status.mak').count(), (n) => n === 0);

  // Two tabs, one stream: a move in one reaches the other by it.
  const streamsAlone = streams;
  const second = await open(url);
  await until('here on the canvas', () => canvasBox(second, 'here'), Boolean);
  const before = await place(second, 'here');
  await drag(first, 'here', 120, 80);
  await until('the other tab follows', () => place(second, 'here'), (now) => now !== before);
  check('two_tabs_one_stream', streams === streamsAlone, { streamsAlone, streams });

  // The tab holding it goes: the other takes it, and a third tab's move still arrives.
  await first.close();
  await until('the stream handed on', () => Promise.resolve(streams), (n) => n === streamsAlone + 1);
  const third = await open(url);
  await until('here on the canvas', () => canvasBox(third, 'here'), Boolean);
  const atHandover = await place(second, 'here');
  await drag(third, 'here', -60, 50);
  await until('followed after the handover', () => place(second, 'here'), (now) => now !== atHandover);
  check('handover', true);

  if (process.env.RESTART) {
    // kill -9: offline, in the top bar; then back with no reload and the session intact.
    const held = await third.evaluate(() => sessionStorage.getItem('cozy.session'));
    const restarted = new Promise((done, fail) => exec(process.env.RESTART, { shell: 'bash' }, (error) => (error ? fail(error) : done())));
    const said = await until('it said offline', () => third.locator('.pill.warn').allTextContents(), (lines) => lines.some((line) => line.startsWith('Calcifer offline')));
    await restarted;
    await until('back', () => third.locator('.pill.warn').allTextContents(), (lines) => !lines.some((line) => line.includes('offline')));
    seen.said_during_restart = said;
    check('no_sign_in_needed', (await third.locator('.gate').count()) === 0 && (await third.evaluate(() => sessionStorage.getItem('cozy.session'))) === held);
    check('still_on_canvas', Boolean(await canvasBox(third, 'here')));
    const wasRow = await row('here');
    const otherWas = await place(second, 'here');
    await drag(third, 'here', 90, 40);
    await until('a move lands after the restart', () => row('here'), (now) => now !== wasRow);
    // The other tab is back too: it shows the move, not merely no "offline".
    await until('the other tab still follows', () => place(second, 'here'), (now) => now !== otherWas);
    check('the_other_tab_follows_after_the_restart', (await second.locator('.pill.warn').allTextContents()).every((line) => !line.includes('offline')));
    await third.reload();
    await until('here on the canvas', () => canvasBox(third, 'here'), Boolean);
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
