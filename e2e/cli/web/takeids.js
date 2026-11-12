// A take has one id from the press of Render to its version (cl-945, Paul 11-12: "as soon as I push the button, the UI updates, and we are
// not fighting for inconsistent state"), in a real browser on a real Calcifer with a weightless local tool. Focus's takes column is watched
// on every change, by each tile's take id:
// A, Render: the new take is listed in the frame after the press, stays listed under that id through every update until it lands, and
//    lands as the version of that id, which the viewer draws.
// B, a slow Calcifer: the render's ask held 4 s on its way and every project read 1.5 s, so the reads the page gets meanwhile don't know
//    the take. It's never taken off.
// C, a reload while it's being made: the same take, by its id, is listed after the reload and lands.
// D, Cancel on a take being made: it goes in that frame and is never listed again; no version lands.
//   node takeids.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots; $ALONE: on a shared computer, so
// no run. Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { writeFileSync } = require('fs');
const { join } = require('path');
const { calcifer, importYaml, lastLook, launch, lookAt, noStall, openBox, projectFromPage, renderOf, signIn, until, STEP_MS } = require('./page');

const FIRST_RUN_MS = 300_000;
const TOOL = `# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43.1", "msgspec>=0.19"]
# ///
"""The takeids cell's picture tool: a weightless still of one random colour, made in steps."""

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
    pace: float = 1.5


@app.tool(label="Take ids", named="takeids", accelerator=False)
def draw(ctx: Context, payload: Drawing, adj: Adjustments, out: Outputs, tel: Telemetry) -> ImageResult:
    width, height = ImageRequest(payload.instructions.text, None, None, payload.seed).size(adj, None, SIZES, megapixels=0.25)
    with tel.stage("denoise"):
        for step in range(8):
            time.sleep(payload.pace)
            ctx.progress(step, 8)
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

// Focus's takes column on every change, by take id: those being made and those landed. A press of Render is noted, and what the
// column lists in the next frame.
const RECORD = () => {
  const log = (window.takesLog = []);
  const look = () => {
    const side = document.querySelector('[data-focused-asset="card"] .here-side');
    const ids = (selector) => [...(side?.querySelectorAll(selector) ?? [])].map((tile) => tile.getAttribute('data-take-id')).filter(Boolean);
    const host = document.querySelector('[data-focused-asset="card"] .focus-viewer .buffered-image');
    return { open: Boolean(side), making: ids('.take.making[data-take-id]'), landed: ids('.take[data-take][data-take-id]'), drawn: (host?.querySelector('img') && host.dataset.displayedSha?.replace(/^sha256:/, '')) || null };
  };
  window.takesNow = look;
  const note = () => {
    const now = look();
    const last = log.at(-1);
    if (!last || JSON.stringify({ ...last, t: 0 }) !== JSON.stringify({ ...now, t: 0 })) log.push({ ...now, t: Math.round(performance.now()) });
  };
  new MutationObserver(note).observe(document.body, { subtree: true, childList: true, attributes: true });
  // React handles a press at its root, so this hears it after the page has: the next frame is what the press drew.
  document.addEventListener('click', (event) => {
    if (!event.target.closest?.('button[aria-label=Render], .take-off')) return;
    const before = look();
    requestAnimationFrame(() => { window.pressed = { before, after: look(), at: log.length } });
  });
  note();
};

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
  await page.fill('input[aria-label="New project\'s name"]', `web-takeids-${Date.now()}`);
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const { id, path: folder } = await projectFromPage(page);
  seen.project = { id, folder };
  const session = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session') ?? 'null'));
  const api = async (path) => {
    const answer = await fetch(base + path, { headers: { Authorization: `Bearer ${session.token}` } });
    const value = await answer.json().catch(() => null);
    if (!answer.ok) throw new Error(`GET ${path}: ${answer.status} ${JSON.stringify(value)}`);
    return value;
  };
  const asset = async () => (await api(`/v1/projects/${id}`)).assets.find((row) => row.name === 'card');
  if (!(await api('/v1/status')).local_usable || process.env.ALONE) {
    check('renders_here', false, 'no local machine to render the weightless tool on');
    throw new Error('nothing to render on');
  }

  const tool = join(folder, `takeids_${Date.now()}.py`);
  writeFileSync(tool, TOOL);
  importYaml(folder, `version: 4\nassets:\n  card:\n    tool: ${tool}/draw\n    instructions: A still of one colour.\n`);
  const landed = (count) => until(`card's take ${count}`, asset, (row) => row?.versions === count && !row.takes?.length, FIRST_RUN_MS);
  const record = () => page.evaluate(RECORD);
  const now = () => page.evaluate(() => window.takesNow());
  const mark = () => page.evaluate(() => { window.pressed = undefined; return window.takesLog.length });
  const logFrom = (from) => page.evaluate((from) => window.takesLog.slice(from), from);
  // The press of Render: the take the next frame lists that wasn't before.
  const press = async () => {
    const from = await mark();
    await (await renderOf(page, 'card')).click();
    const pressed = await until('the frame after Render', () => page.evaluate(() => window.pressed), Boolean, 10_000);
    return { from, pressed, take: pressed.after.making.find((take) => !pressed.before.making.includes(take)) };
  };
  // From the press on, the take is listed in every look, being made and then landed, never off in between.
  const steady = (log, take) => {
    const off = log.filter((look) => look.open && !look.making.includes(take) && !look.landed.includes(take));
    return { off: off.slice(0, 4), landed: log.some((look) => look.landed.includes(take)) };
  };
  const versionOf = (row, take) => row.prefixes.find((version) => version.id === take);

  await record();
  await openBox(page, 'card');
  await (await renderOf(page, 'card')).click();
  await landed(1);

  // A: listed in the frame after the press, under one id to its version.
  const a = await press();
  check('a_listed_in_the_next_frame', Boolean(a.take), a.pressed);
  const two = await landed(2);
  const logA = await logFrom(a.from);
  const steadyA = steady(logA, a.take);
  const versionA = versionOf(two, a.take);
  const drawnA = await until('take 2 drawn', now, (look) => look.drawn === versionA?.main?.sha256?.replace(/^sha256:/, ''), 20_000).catch(() => null);
  seen.a = { take: a.take, version: versionA?.number, steady: steadyA };
  check('a_never_off_until_it_lands', steadyA.off.length === 0 && steadyA.landed, { ...steadyA, log: logA.slice(0, 12) });
  check('a_lands_as_the_version_of_its_id', versionA?.number === 2, two.prefixes.map((version) => [version.number, version.id]));
  check('a_viewer_draws_it', drawnA !== null, await now());

  // B: a slow Calcifer. Its ask held on its way and every project read late, so what the page reads meanwhile doesn't know the take.
  const slowAsk = (url) => /\/v1\/projects\/[^/]+\/runs$/.test(new URL(url).pathname);
  const at = `/v1/projects/${encodeURIComponent(id)}`;
  const slowRead = (url) => new URL(url).pathname === at || new URL(url).pathname.startsWith(`${at}/assets/`);
  const hold = (ms) => async (route) => {
    await new Promise((done) => setTimeout(done, ms));
    await route.continue().catch(() => {});
  };
  await page.route(slowAsk, async (route) => (route.request().method() === 'POST' ? hold(4000)(route) : route.continue()));
  await page.route(slowRead, async (route) => (route.request().method() === 'GET' ? hold(1500)(route) : route.continue()));
  const b = await press();
  check('b_listed_in_the_next_frame', Boolean(b.take), b.pressed);
  await page.waitForTimeout(4500);
  const heldB = await now();
  check('b_listed_while_calcifer_has_not_heard', heldB.making.includes(b.take) && !(await asset()).takes?.some((take) => take.id === b.take), heldB);
  await page.unroute(slowAsk);
  await page.unroute(slowRead);
  const three = await landed(3);
  const logB = await logFrom(b.from);
  const steadyB = steady(logB, b.take);
  seen.b = { take: b.take, steady: steadyB };
  check('b_never_off_until_it_lands', steadyB.off.length === 0 && steadyB.landed, { ...steadyB, log: logB.slice(0, 12) });
  check('b_lands_as_the_version_of_its_id', versionOf(three, b.take)?.number === 3, three.prefixes.map((version) => [version.number, version.id]));

  // C: reloaded while it's being made: the same take, by its id, listed again and landing.
  const c = await press();
  await until('take 4 being made', asset, (row) => row.takes?.some((take) => take.id === c.take && take.run), 30_000);
  await page.reload();
  await page.locator('[data-focused-asset="card"] .here-side').waitFor();
  await record();
  const again = await until('the take listed after the reload', now, (look) => look.making.includes(c.take) || look.landed.includes(c.take), 15_000).catch(() => null);
  check('c_same_take_after_reload', again !== null, await now());
  const four = await landed(4);
  check('c_lands_as_the_version_of_its_id', versionOf(four, c.take)?.number === 4, four.prefixes.map((version) => [version.number, version.id]));
  await shot('c-reloaded');

  // D: Cancel on a take being made: gone in the frame after, never listed again, and nothing of it lands.
  const d = await press();
  await until('take 5 being made', asset, (row) => row.takes?.some((take) => take.id === d.take && take.run), 30_000);
  const fromD = await mark();
  await page.locator(`[data-focused-asset="card"] .here-side .take-at:has(.take[data-take-id="${d.take}"]) .take-off`).click();
  const cancelled = await until('the frame after Cancel', () => page.evaluate(() => window.pressed), Boolean, 10_000);
  await until('its run ended', asset, (row) => !row.takes?.some((take) => take.id === d.take), 60_000);
  await page.waitForTimeout(1500);
  const logD = await logFrom(fromD);
  const backD = logD.filter((look) => look.making.includes(d.take) || look.landed.includes(d.take));
  seen.d = { take: d.take, back: backD.slice(0, 4) };
  check('d_gone_in_the_next_frame', !cancelled.after.making.includes(d.take), cancelled);
  check('d_never_listed_again', backD.length === 0, { back: backD.slice(0, 4), log: logD.slice(0, 12) });
  check('d_nothing_lands', (await asset()).versions === 4);

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch(died);
