// A new take never changes what's approved (cl-908, Paul 11-12), in a real browser on a real Calcifer with a weightless local tool:
// a box's first take, rendered from the page, isn't approved on the wire or the page; a double tap approves take 1; the page's Render
// makes take 2, and take 1 stays approved, in project.db and on the page, and is what a mention of the box sends to a real run.
//   node approvals.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots; $ALONE: on a shared computer, so
// no run. Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { join } = require('path');
const { addBoxes, calcifer, lastLook, launch, lookAt, noStall, openBox, projectFromPage, renderOf, signIn, storedOf, until, STEP_MS } = require('./page');

const FIRST_RUN_MS = 300_000;
const failures = [];
const seen = {};
let during = 'the start';
noStall(seen, died);
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  during = name;
  if (!ok) failures.push(name);
}
async function died(why) {
  failures.push(`${during}: ${why instanceof Error ? why.message : why}`);
  await lastLook();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(1);
}

(async () => {
  const base = calcifer();
  const browser = await launch(playwright);
  const page = await (await browser.newContext({ viewport: { width: 1600, height: 1000 } })).newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());

  await signIn(page);
  await page.waitForSelector('.top');
  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-approvals-${Date.now()}`);
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const { id, path: folder } = await projectFromPage(page);
  seen.project = { id, folder };
  const session = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session') ?? 'null'));
  const api = async (path, method = 'GET', body) => {
    const answer = await fetch(base + path, {
      method,
      headers: { Authorization: `Bearer ${session.token}`, 'Content-Type': 'application/json', ...(method === 'POST' ? { 'Idempotency-Key': `approvals-${Date.now()}-${Math.random()}` } : {}) },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const value = await answer.json().catch(() => null);
    if (!answer.ok) throw new Error(`${method} ${path}: ${answer.status} ${JSON.stringify(value)}`);
    return value;
  };
  const at = `/v1/projects/${id}`;
  const asset = async (name) => (await api(at)).assets.find((row) => row.name === name);
  if (!(await api('/v1/status')).local_usable || process.env.ALONE) {
    check('renders_here', false, 'no local machine to render the weightless tool on');
    throw new Error('nothing to render on');
  }

  const fixtures = join(__dirname, '..', 'project');
  addBoxes(folder, [
    { name: 'card', tool: `${join(fixtures, 'draw.py')}/draw`, fields: { instructions: 'A still of one colour.' } },
    { name: 'copied', tool: `${join(fixtures, 'copied.py')}/copy`, fields: { image: '<card>' } },
  ]);
  const card = page.locator('.card.here[data-asset="card"]');
  const take = (number) => card.locator(`.take[data-take="${number}"]`);
  const render = async (count) => {
    await openBox(page, 'card');
    await (await renderOf(page, 'card')).click();
    return until(`card's take ${count}`, () => asset('card'), (row) => row?.versions === count && !row.making?.length, FIRST_RUN_MS);
  };
  const mainOf = (row, number) => row.prefixes.find((version) => version.number === number)?.main?.sha256;

  // A fresh box: its first take is its state, and nothing is approved.
  const first = await render(1);
  check('first_take_not_approved', !first.pinned && first.approved == null && first.version === 1 && first.status === 'current', first);
  await openBox(page, 'card');
  await take(1).waitFor();
  check('first_take_not_shown_approved', (await card.locator('.take.approved').count()) === 0 && (await page.locator('.card.showing-approved').count()) === 0);
  check('first_take_no_approved_line', storedOf(folder).card?.approved === undefined, storedOf(folder).card);
  await shot('first');

  // A double tap approves take 1.
  await take(1).dblclick();
  const approved = await until('take 1 approved', () => asset('card'), (row) => row.pinned && row.approved === 1);
  await until('take 1 shown approved', () => card.locator('.take.approved[data-take="1"]').count(), (n) => n === 1);

  // Take 2 from the page's Render: take 1 stays approved, in project.db and on the page.
  const second = await render(2);
  const take1 = mainOf(approved, 1);
  const take2 = mainOf(second, 2);
  check('second_take_is_new', Boolean(take2) && take2 !== take1, second.prefixes);
  check('approval_stays_on_take_1', second.pinned && second.approved === 1 && second.version === 1 && second.status?.startsWith('approved: version 1'), second);
  await openBox(page, 'card');
  await take(2).waitFor();
  check('page_shows_take_1_approved', (await card.locator('.take.approved').count()) === 1 && (await card.locator('.take.approved[data-take="1"]').count()) === 1);
  check('approved_line_is_take_1', String(storedOf(folder).card?.approved ?? '').startsWith(take1.replace('sha256:', '').slice(0, 12)), storedOf(folder).card);
  await shot('second');

  // What mentions it reads take 1: a real run of `copied` (image: <card>) is sent take 1's file, and isn't approved itself.
  await api(`${at}/runs`, 'POST', { names: ['copied'], on: ['local'] });
  const copied = await until('copied made', () => asset('copied'), (row) => row?.versions === 1 && !row.making?.length, FIRST_RUN_MS);
  const run = copied.prefixes[0].run;
  const sent = JSON.stringify((await api(`/v1/runs/${run}?detail=full`)).request?.input ?? null);
  seen.copied_input = sent;
  check('mention_reads_the_approved_take', sent.includes(take1.replace('sha256:', '')) && !sent.includes(take2.replace('sha256:', '')), sent);
  check('reader_take_not_approved', !copied.pinned && copied.approved == null, copied);
  check('card_still_take_1', (await asset('card')).approved === 1);

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch(died);
