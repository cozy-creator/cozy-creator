// A write whose answer is lost after it landed, sent again by the page's outbox (cl-812): every try under one Idempotency-Key, and it is done
// once. A take discarded whose answer is lost stays discarded, with no "Couldn't discard" (before, the retry answered 404 version_unknown); an
// undo whose answer is lost undoes one entry.
//   node resend.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $COZY: the CLI. Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { mkdtempSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { projectFromPage, calcifer, signIn, STEP_MS, boxOf, closeBox, launch, lastLook, lookAt, noStall, openBox, png, settled, until, upload } = require('./page');

const failures = [];
const seen = {};
noStall(seen, died);
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  if (!ok) failures.push(name);
}

/** The next `times` calls to `path` go to Calcifer and their answers are lost: what a dropped connection does after the write landed. */
async function loseAnswers(page, method, path, times) {
  const keys = [];
  let left = times;
  const route = async (lost) => {
    if (lost.request().method() !== method) return lost.fallback();
    keys.push(lost.request().headers()['idempotency-key'] ?? null);
    if (left <= 0) return lost.fallback();
    left -= 1;
    await lost.fetch();
    await lost.abort('connectionreset');
  };
  await page.route(path, route);
  return { keys, done: () => page.unroute(path, route) };
}

(async () => {
  const base = calcifer();
  const browser = await launch(playwright);
  const page = await (await browser.newContext({ viewport: { width: 1600, height: 1000 } })).newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await signIn(page);
  await page.waitForSelector('.top');
  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-resend-${Date.now()}`);
  await page.getByLabel("Create without encryption", { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const token = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session') ?? 'null')?.token);
  const api = (path, init = {}) => page.evaluate(async ({ path, init, token }) => {
    const answer = await fetch(path, { ...init, headers: { ...(token ? { Authorization: `Bearer ${token}` } : {}), 'Content-Type': 'application/json', ...init.headers } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  }, { path, init, token });
  const at = `/v1/projects/${id}`;
  seen.project = { id, folder: (await api(at)).body.path };
  const asset = async (name) => (await api(at)).body.assets.find((row) => row.name === name);
  const undone = async () => (await api(`${at}/history?limit=50`)).body.entries.filter((entry) => entry.undone).length;
  const notes = () => page.locator('.notes .note .line').allInnerTexts();

  // A picture of two versions.
  const pictures = mkdtempSync(join(tmpdir(), 'web-resend-'));
  const picture = (shade) => {
    const path = join(pictures, `${shade}.png`);
    writeFileSync(path, png(shade));
    return path;
  };
  await api(`${at}/imports`, { method: 'POST', body: JSON.stringify({ source: await upload(token, picture(10)), name: 'shot', exact: true, at: { x: 80, y: 120 } }) });
  await api(`${at}/assets/shot/versions`, { method: 'POST', body: JSON.stringify({ source: await upload(token, picture(20)) }) });
  await boxOf(page, 'shot').waitFor();
  await page.keyboard.press('f');
  await settled(page);
  const shotRow = await asset('shot');
  const other = shotRow.approved === 2 ? 1 : 2;
  const takes = async () => (await asset('shot')).prefixes?.length;
  const had = await takes();

  // The discard's answer lost: the outbox sends it again under its key, and it's discarded once, with nothing said against it.
  const discard = await loseAnswers(page, 'DELETE', `**${at}/assets/shot/versions/*`, 1);
  await openBox(page, 'shot');
  await page.locator(`.card.here .take-at:has(.take[data-take="${other}"])`).hover();
  await page.locator(`.card.here .take-at:has(.take[data-take="${other}"]) .take-off`).click();
  await until('the discard sent again', () => Promise.resolve(discard.keys.length), (n) => n >= 2).catch(() => null);
  await discard.done();
  await settled(page);
  seen.discard = { keys: discard.keys, takes: await takes(), notes: await notes() };
  check('discard_tries_share_one_key', discard.keys.length >= 2 && discard.keys[0] && new Set(discard.keys).size === 1, discard.keys);
  check('discarded_once', (await takes()) === had - 1, { had, now: await takes() });
  check('no_couldnt_discard', !(await notes()).some((line) => /Couldn't discard/.test(line)), await notes());
  await closeBox(page);

  // The undo's answer lost: sent again under its key, one entry undone.
  const before = await undone();
  const back = await loseAnswers(page, 'POST', `**${at}/history/undo`, 1);
  await page.mouse.move(2, 500);
  await page.keyboard.press('ControlOrMeta+z');
  await until('the undo sent again', () => Promise.resolve(back.keys.length), (n) => n >= 2).catch(() => null);
  await back.done();
  await until('the take back', takes, (n) => n === had).catch(() => null);
  seen.undo = { keys: back.keys, undone_before: before, undone_now: await undone(), takes: await takes() };
  check('undo_tries_share_one_key', back.keys.length >= 2 && back.keys[0] && new Set(back.keys).size === 1, back.keys);
  check('undone_once', (await undone()) === before + 1, seen.undo);
  check('no_page_errors', errors.length === 0, errors);
  console.log(JSON.stringify({ failures, seen }, null, 2));
  await browser.close();
  process.exit(failures.length ? 1 : 0);
})().catch(died);

async function died(error) {
  console.error(error);
  await lastLook();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(2);
}
