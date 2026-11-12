// A box with a tool, right clicked: Download, Star, Rename and Delete, and no "Change the tool" (Paul, 11-12: a tool changes only by a drop
// from the palette), in a real browser on a real Calcifer.
//   node menu.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots. Prints what it saw as JSON and exits 1
// if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { boxOf, calcifer, fitCanvas, lastLook, launch, lookAt, noStall, projectFromPage, signIn, STEP_MS, until } = require('./page');

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
  const context = await browser.newContext({ viewport: { width: 1600, height: 1000 } });
  const page = await context.newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
  page.on('console', (m) => { if (m.type() === 'error' && !/status of 4\d\d|net::ERR_/.test(m.text())) errors.push(`console: ${m.text()}`); });

  await signIn(page);
  await page.waitForSelector('.top');
  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-menu-${Date.now()}`);
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const { id } = await projectFromPage(page);
  const session = await page.evaluate(() => JSON.parse(sessionStorage.getItem('cozy.session') ?? 'null'));
  const api = async (path, method = 'GET', body) => {
    const answer = await fetch(base + path, {
      method,
      headers: { Authorization: `Bearer ${session.token}`, 'cozy-project-storage': '3', 'Content-Type': 'application/json' },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const value = await answer.json().catch(() => null);
    if (!answer.ok) throw new Error(`${method} ${path}: ${answer.status} ${JSON.stringify(value)}`);
    return value;
  };
  const at = `/v1/projects/${id}`;
  seen.project = { id, folder: (await api(at)).path };

  // A box with an image tool, and another image tool it could have taken.
  const { tools } = await api(`/v1/tools?project=${id}&palette=true`);
  const images = tools.filter((tool) => tool.makes === 'image');
  if (!images.length) throw new Error('no image tool listed for the fixture');
  seen.tool = images[0].id;
  seen.others = images.slice(1).map((tool) => tool.id);
  await api(`${at}/assets`, 'POST', { name: 'tooled', exact: true, kind: 'image', tool: images[0].id, fields: {}, at: { x: 300, y: 200 } });
  await until('the box has its tool', async () => (await api(at)).assets.find((row) => row.name === 'tooled'), (row) => row?.tool === images[0].id);
  await boxOf(page, 'tooled').waitFor();
  await fitCanvas(page);

  await page.locator('.react-flow__node:has(.card[data-asset="tooled"]) .nm').first().click({ button: 'right' });
  await page.waitForSelector('.asset-menu');
  const items = await page.locator('.asset-menu [role=menuitem]').allTextContents();
  seen.items = items;
  check('menu_has_no_change_the_tool', !items.some((item) => /change the tool/i.test(item)), items);
  check('menu_keeps_the_rest', ['Star', 'Rename', 'Delete'].every((item) => items.includes(item)), items);
  if (process.env.SHOT) await page.screenshot({ path: `${process.env.SHOT}-menu.png` });
  await page.keyboard.press('Escape');

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch((why) => died(why.stack ?? why));
