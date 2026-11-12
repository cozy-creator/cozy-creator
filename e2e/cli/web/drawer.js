// The right-side panel closes on a click off it (cl-929, Paul 11-12: "when you open the right-side panel, clicking off of it should close
// that right-side panel"), in a real browser on a real Calcifer, under cl-923's rule. Machines open from the main menu: words in it selected and
// let go on the canvas, it's still open, the words still selected; pressed and let go on the canvas, it closes. A click inside keeps it; Esc closes it.
//   node drawer.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER.
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots. Prints what it saw as JSON and exits 1
// if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { projectFromPage, calcifer, signIn, STEP_MS, lastLook, launch, lookAt, noStall, openCanvas, until } = require('./page');

const failures = [];
const seen = {};
noStall(seen, died);
let during = 'the start';
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  during = name;
  if (!ok) failures.push(name);
}
async function died(why) {
  failures.push(`${during}: ${why}`);
  await lastLook();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(1);
}

(async () => {
  const base = calcifer();
  const browser = await launch(playwright);
  const context = await browser.newContext({ viewport: { width: 1600, height: 1100 } });
  const page = await context.newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
  page.on('console', (m) => { if (m.type() === 'error' && !/status of 4\d\d|net::ERR_/.test(m.text())) errors.push(`console: ${m.text()}`); });
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());

  await signIn(page);
  await page.waitForSelector('.top');
  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-drawer-${Date.now()}`);
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  seen.project = await projectFromPage(page);

  const drawer = page.locator('aside.drawer');
  const isOpen = async () => (await drawer.count()) > 0 && (await drawer.isVisible());
  /** A drawer opened from the main menu by its link's words. */
  const openFromMenu = async (words) => {
    await page.locator('details.main-menu > summary').click();
    await page.locator('details.main-menu nav').getByText(words, { exact: true }).first().click();
    await until(`${words} open`, isOpen, Boolean);
  };

  // Its title's words selected, let go on the canvas.
  await openFromMenu('Machines');
  const title = drawer.locator('.drawer-head b').first();
  const from = await title.boundingBox();
  const away = await openCanvas(page);
  seen.press = { from, away };
  await page.mouse.move(from.x + 2, from.y + from.height / 2);
  await page.mouse.down();
  await page.mouse.move(away.x, away.y, { steps: 16 });
  await page.mouse.up();
  await page.waitForTimeout(400);
  const selected = await page.evaluate(() => document.getSelection()?.toString() ?? '');
  seen.selected = selected;
  check('a_selection_let_go_outside_keeps_it_open', (await isOpen()) && selected.length > 0, { open: await isOpen(), selected });
  await shot('kept');

  // Pressed and let go on the canvas: it closes, and the address is the canvas's.
  await page.mouse.click(away.x, away.y);
  const closed = await until('closed by a click off it', isOpen, (open) => !open, 5_000).then(() => true, () => false);
  seen.after_click = page.url();
  check('a_click_off_it_closes_it', closed && !/\/machines$/.test(new URL(page.url()).pathname), { closed, url: page.url() });

  // A press inside and let go inside: still open. Esc: closed.
  await openFromMenu('Machines');
  const head = await drawer.locator('.drawer-head b').first().boundingBox();
  await page.mouse.click(head.x + head.width / 2, head.y + head.height / 2);
  await page.waitForTimeout(300);
  check('a_click_inside_keeps_it_open', await isOpen());
  await page.keyboard.press('Escape');
  const escaped = await until('closed by Esc', isOpen, (open) => !open, 5_000).then(() => true, () => false);
  check('esc_closes_it', escaped);

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch((why) => died(why.stack ?? why));
