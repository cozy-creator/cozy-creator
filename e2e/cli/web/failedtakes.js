// A take that fails stays in its box's takes column (cl-954; Paul, 11-12: a failed render vanishing is what he notices), in a real
// browser on a real Calcifer with a weightless local tool that fails when asked. It stays listed under its id, red, saying Failed; picked
// while another take renders, its frame shows its own error with Copy, not the other's progress; a reload keeps it; its trash deletes it
// in that frame, for good. Looked at with one failed and one rendering, at 1440×900 and 1920×1080, paper and dark.
//   node failedtakes.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots; $ALONE: on a shared computer, so
// no run. Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { writeFileSync } = require('fs');
const { join } = require('path');
const { holderToken, projectFolder, addBoxes, calcifer, lastLook, launch, lookAt, noStall, openBox, projectFromPage, renderOf, signIn, until, STEP_MS } = require('./page');

const FIRST_RUN_MS = 300_000;
const TOOL = `# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43.1", "msgspec>=0.19"]
# ///
"""The failedtakes cell's picture tool: a weightless still of one random colour, made in steps; asked to, it fails."""

import os
import time

import msgspec
from cozy_runtime.author import Adjustments, App, Context, ImageFrame, Outputs, Seed, Telemetry
from cozy_runtime.fields import Image, Prompt
from cozy_runtime.image import ImageRequest, ImageResult, Sizes

app = App()
SIZES = Sizes((0.25, 4.0), (64, 8192), 16, "the still's 0.25-4 MP")


class Drawing(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image] = Prompt()
    seed: Seed | None = None
    pace: float = 0.3
    fail: bool = False


@app.tool(label="Failed takes", named="failedtakes", accelerator=False)
def draw(ctx: Context, payload: Drawing, adj: Adjustments, out: Outputs, tel: Telemetry) -> ImageResult:
    width, height = ImageRequest(payload.instructions.text, None, None, payload.seed).size(adj, None, SIZES, megapixels=0.25)
    with tel.stage("denoise"):
        for step in range(8):
            time.sleep(payload.pace)
            ctx.progress(step, 8)
    if payload.fail:
        raise RuntimeError("asked to fail, as the cell wants")
    image = out.save_image(ImageFrame(width, height, os.urandom(3) * (width * height)), format="png")
    return ImageResult(image, width, height, payload.seed or 0, payload.instructions.text)
`;
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
  await page.fill('input[aria-label="New project\'s folder"]', projectFolder(`web-failedtakes-${Date.now()}`));
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const { id, path: folder } = await projectFromPage(page);
  seen.project = { id, folder };
  const api = async (path, method = 'GET', body) => {
    const answer = await fetch(base + path, { method, headers: { Authorization: `Bearer ${holderToken()}`, 'cozy-project-storage': '3', 'Content-Type': 'application/json' }, body: body === undefined ? undefined : JSON.stringify(body) });
    const value = await answer.json().catch(() => null);
    if (!answer.ok) throw new Error(`${method} ${path}: ${answer.status} ${JSON.stringify(value)}`);
    return value;
  };
  const asset = async () => (await api(`/v1/projects/${id}`)).assets.find((row) => row.name === 'card');
  if (!(await api('/v1/status')).local_usable || process.env.ALONE) {
    check('renders_here', false, 'no local machine to render the weightless tool on');
    throw new Error('nothing to render on');
  }

  const tool = join(folder, `failedtakes_${Date.now()}.py`);
  writeFileSync(tool, TOOL);
  addBoxes(folder, [{ name: 'card', tool: `${tool}/draw`, kind: 'image', fields: { instructions: 'A still of one colour.' } }]);
  const set = (fields) => api(`/v1/projects/${id}/assets/card`, 'PATCH', { fields });
  const column = '[data-focused-asset="card"] .here-side';
  const tile = (take) => page.locator(`${column} .take.making[data-take-id="${take}"]`);
  const pendingIds = () => page.evaluate((column) => [...document.querySelectorAll(`${column} .take.making[data-take-id]`)].map((element) => element.getAttribute('data-take-id')), column);
  const stateOf = (take) => page.evaluate(({ column, take }) => {
    const element = document.querySelector(`${column} .take.making[data-take-id="${take}"]`);
    return element ? { state: element.getAttribute('data-state'), dot: element.querySelector('.state-dot')?.className ?? null, said: element.innerText.trim(), on: element.classList.contains('on') } : null;
  }, { column, take });
  // Render pressed in Focus: the take it made, by its id, the one tile that wasn't there before.
  const press = async () => {
    const before = await pendingIds();
    await (await renderOf(page, 'card')).click();
    return until('the new take listed', pendingIds, (ids) => ids.some((one) => !before.includes(one)), 10_000).then((ids) => ids.find((one) => !before.includes(one)));
  };
  // A press as a person's: what's at its middle must be the tile itself.
  const pressable = (locator) => locator.evaluate((element) => {
    const r = element.getBoundingClientRect();
    const top = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
    return Boolean(top && (top === element || element.contains(top)));
  });

  await openBox(page, 'card');
  await (await renderOf(page, 'card')).click();
  await until('take 1', asset, (row) => row?.versions === 1 && !row.takes?.some((take) => !take.failure), FIRST_RUN_MS);

  // It fails: it stays, under its id, red, saying so; Calcifer lists it with why.
  await set({ fail: true });
  const failed = await press();
  const row = await until('the take failed', asset, (now) => now.takes?.some((take) => take.id === failed && take.failure), 120_000);
  const why = row.takes.find((take) => take.id === failed).failure;
  const look = await until('its tile says it failed', () => stateOf(failed), (now) => now?.state === 'failed', 15_000).catch(() => null);
  seen.failed = { take: failed, why: why.message, look };
  check('f_stays_listed_red', look !== null && /\bfailed\b/.test(look.dot) && look.said === 'Failed', look ?? (await stateOf(failed)));
  check('f_calcifer_says_why', /asked to fail/.test(why.message), why);

  // Another renders; the failed one, picked, shows its own error with Copy, not the other's progress.
  await set({ fail: false, pace: 1.5 });
  const going = await press();
  await until('the other running', asset, (now) => now.takes?.some((take) => take.id === going && take.run), 60_000);
  check('f_tile_pressable', await pressable(tile(failed)), await stateOf(failed));
  await tile(failed).click();
  const frame = page.locator('[data-focused-asset="card"] .focus-viewer .run-face.failed');
  const shown = await until('its error in the frame', () => frame.innerText().catch(() => ''), (text) => /asked to fail/.test(text), 10_000).catch(() => null);
  const copy = await page.locator('[data-focused-asset="card"] .focus-viewer .run-face.failed .copy-error').count();
  const face = await page.evaluate(() => document.querySelector('[data-focused-asset="card"] .run-face.above')?.getAttribute('data-run') ?? null);
  seen.selected = { shown, copy, face, on: (await stateOf(failed))?.on };
  check('f_selected_shows_its_own_error', shown !== null && copy === 1 && face === null && (await stateOf(failed))?.on === true, seen.selected);
  for (const [width, height] of [[1440, 900], [1920, 1080]]) {
    await page.setViewportSize({ width, height });
    for (const dark of [false, true]) {
      await page.evaluate((dark) => { if (dark) document.documentElement.dataset.theme = 'dark'; else delete document.documentElement.dataset.theme; }, dark);
      await page.waitForTimeout(500);
      await shot(`failed-${width}x${height}-${dark ? 'dark' : 'paper'}`);
    }
  }
  await page.evaluate(() => { delete document.documentElement.dataset.theme; });
  await page.setViewportSize({ width: 1600, height: 1000 });

  // The other lands beside it; it's still there, after a reload too.
  await until('the other lands', asset, (now) => now.prefixes.some((version) => version.id === going), FIRST_RUN_MS);
  check('f_stays_after_another_lands', (await asset()).takes?.some((take) => take.id === failed && take.failure) && (await stateOf(failed))?.state === 'failed', await asset());
  await page.reload();
  await page.locator(column).waitFor();
  const kept = await until('it after a reload', () => stateOf(failed), (now) => now?.state === 'failed', 15_000).catch(() => null);
  check('f_kept_through_a_reload', kept !== null, kept);

  // Its trash deletes it, in that frame, for good.
  await page.evaluate(() => {
    window.failedSeen = [];
    const look = () => window.failedSeen.push(document.querySelectorAll('[data-take-id][data-state="failed"]').length);
    new MutationObserver(look).observe(document.body, { subtree: true, childList: true, attributes: true });
  });
  const trash = page.locator(`${column} .take-at:has(.take[data-take-id="${failed}"]) .take-off`);
  check('f_trash_is_labelled', (await trash.getAttribute('aria-label')) === 'Delete this failed take');
  await trash.click();
  const gone = await until('it gone from Calcifer', asset, (now) => !now.takes?.some((take) => take.id === failed), 15_000).catch(() => null);
  await page.waitForTimeout(1000);
  const after = await page.evaluate(() => window.failedSeen);
  check('f_deleted_at_once_never_back', gone !== null && after.length > 0 && after.every((n, i) => i === 0 || n <= after[i - 1]) && after.at(-1) === 0, { gone: gone?.takes, after: after.slice(-6) });
  await page.reload();
  await page.locator(column).waitFor();
  // Its takes listed again, by the same read that would list the failed one: only then does its absence mean something.
  const made = (await asset()).versions;
  await until('its takes listed again', () => page.locator(`${column} .take[data-take]`).count(), (n) => n === made, 15_000).catch(() => {});
  check('f_deleted_through_a_reload', (await stateOf(failed)) === null && !(await asset()).takes?.some((take) => take.id === failed));

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch(died);
