// A run says one thing at most, and only what matters (cl-921, Paul 11-12: "a ton of errors like 'image CUDA graph cannot be used' but
// the image generated … they flashed up on my screen so fast"). A render that completes with warnings (its log's and its result's, as the
// Runtime's handled fallbacks are) puts none of them in a note: they're its box's to show. Its one note says it finished (cl-937: its box,
// its take, a picture of it and View); a take landing while it's looked at says nothing. A render that fails says so in one note, with
// its error to copy, that stays until it's dismissed. Every note the page puts up is kept as it comes, so none that flashes by is missed.
// In a real browser on a real Calcifer with a weightless local tool.
//   node toasts.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots; $ALONE: on a shared computer, so
// no run. Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { writeFileSync } = require('fs');
const { join } = require('path');
const { holderToken, projectFolder, addBoxes, calcifer, lastLook, launch, lookAt, noStall, openBox, projectFromPage, renderOf, signIn, until, STEP_MS } = require('./page');

const FIRST_RUN_MS = 300_000;
const WARNED = 'a CUDA graph was refused at capture';
const TOOL = `# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43.1", "msgspec>=0.19"]
# ///
"""The toasts cell's tool: a still of one random colour that warns, in its log and its result, as a handled fallback does; asked to,
it fails."""

import os
import time

import msgspec
from cozy_runtime.author import App, Context, ImageFrame, Outputs, Seed, Telemetry
from cozy_runtime.fields import Image, Prompt
from cozy_runtime.media import ImageFile

app = App()
SIDE = 16


class Drawing(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image] = Prompt()
    seed: Seed | None = None
    fail: bool = False


class Warned(msgspec.Struct):
    image: ImageFile
    prompt: str
    warnings: list[str]


@app.tool(label="Toasts", named="toasts", accelerator=False)
def draw(ctx: Context, payload: Drawing, out: Outputs, tel: Telemetry) -> Warned:
    with tel.stage("denoise"):
        for step in range(4):
            time.sleep(0.3)
            ctx.progress(step, 4)
    if payload.fail:
        raise RuntimeError("asked to fail, as the cell wants")
    tel.log("${WARNED} (2x), and it ran without one", level="warn")
    image = out.save_image(ImageFrame(SIDE, SIDE, os.urandom(3) * SIDE**2), format="png")
    return Warned(image, payload.instructions.text, ["${WARNED} (2x), and it ran without one"])
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
  await page.fill('input[aria-label="New project\'s folder"]', projectFolder(`web-toasts-${Date.now()}`));
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const { id, path: folder } = await projectFromPage(page);
  seen.project = { id, folder };
  const session = { token: holderToken() };
  const api = async (path) => {
    const answer = await fetch(base + path, { headers: { Authorization: `Bearer ${session.token}`, 'cozy-project-storage': '3' } });
    const value = await answer.json().catch(() => null);
    if (!answer.ok) throw new Error(`GET ${path}: ${answer.status} ${JSON.stringify(value)}`);
    return value;
  };
  const asset = async (name) => (await api(`/v1/projects/${id}`)).assets.find((row) => row.name === name);
  if (!(await api('/v1/status')).local_usable || process.env.ALONE) {
    check('renders_here', false, 'no local machine to render the weightless tool on');
    throw new Error('nothing to render on');
  }

  const tool = join(folder, `toasts_${Date.now()}.py`);
  writeFileSync(tool, TOOL);
  // Their kind given, Calcifer reads nothing of the tool before its first render installs it.
  addBoxes(folder, [
    { name: 'warned', tool: `${tool}/draw`, kind: 'image', fields: { instructions: 'A still of one colour.' } },
    { name: 'broken', tool: `${tool}/draw`, kind: 'image', fields: { instructions: 'Never made.', fail: true } },
  ]);
  const leave = async () => {
    for (let i = 0; i < 3 && (await page.locator('.card.here').count()); i++) {
      await page.keyboard.press('Escape');
      await page.waitForTimeout(200);
    }
    await page.locator('.card.here').waitFor({ state: 'detached' });
  };
  const render = async (name) => {
    await openBox(page, name);
    await (await renderOf(page, name)).click();
  };
  // Every note the page puts up, by its id, as the page draws it: an observer in the page keeps each, so one up for a frame is still seen.
  await page.evaluate(() => {
    window.notesSeen = {};
    const keep = () => { for (const one of document.querySelectorAll('[data-note]')) window.notesSeen[one.dataset.note] = one.innerText; };
    keep();
    new MutationObserver(keep).observe(document.body, { subtree: true, childList: true, characterData: true });
  });
  const notes = new Map();
  const look = async () => {
    for (const [note, text] of Object.entries(await page.evaluate(() => window.notesSeen))) notes.set(note, text);
  };

  // 1. Completed with warnings: none of them is a note; at most one note of it (that it finished).
  await render('warned');
  await leave();
  const made = await until('warned has a take', () => asset('warned'), (row) => row?.versions >= 1, FIRST_RUN_MS);
  // Its note that it finished: its box and take, a picture of it, View (cl-937).
  const finished = page.locator('[data-note]', { hasText: 'warned finished' }).first();
  await finished.waitFor({ timeout: 15_000 });
  const picture = await until('its picture', () => finished.locator('img.note-picture').evaluate((img) => img.complete && img.naturalWidth > 0).catch(() => false), Boolean, 10_000).catch(() => false);
  const finishedSays = await finished.innerText();
  check('a_finished_take_says_so_with_its_picture_and_view', /warned finished: version 1/.test(finishedSays) && picture && (await finished.getByRole('button', { name: 'View', exact: true }).count()) === 1, { says: finishedSays, picture });
  // A while after it landed, for a warning that would come late: the observer has every note meanwhile.
  await page.waitForTimeout(4000);
  await look();
  const ofWarned = [...notes.values()].filter((text) => /warned/.test(text) || text.includes(WARNED));
  seen.warned = { versions: made.versions, notes: [...notes.values()] };
  const kept = (await api(`/v1/projects/${id}`)).warnings ?? [];
  seen.warned.kept = kept.filter((one) => one.name === 'warned').map((one) => one.message);
  check('its_warnings_reached_the_box', seen.warned.kept.some((line) => line.includes(WARNED)), seen.warned);
  check('no_note_says_a_warning', ![...notes.values()].some((text) => text.includes(WARNED)), seen.warned.notes);
  check('one_note_at_most_for_a_completed_run', ofWarned.length <= 1, ofWarned);
  await shot('warned');

  // 2. Rendered and looked at where it lands: nothing is said of it (cl-937).
  await render('warned');
  await until('warned has its second take', () => asset('warned'), (row) => row?.versions >= 2, FIRST_RUN_MS);
  await until('Focus shows it', () => page.locator('.focus-heading .focus-version-label').innerText().catch(() => ''), (label) => label === 'v2', 15_000).catch(() => {});
  // As long as a finished note took to come for the first take, and some: the observer would have it.
  await page.waitForTimeout(3000);
  await look();
  check('a_take_landing_while_looked_at_says_nothing', ![...notes.values()].some((text) => /warned finished: version 2/.test(text)), [...notes.values()]);
  await leave();

  // 3. Failed: one note, its error to copy, still up after 20 s.
  const before = new Set(notes.keys());
  await render('broken');
  await leave();
  await until('broken failed', () => asset('broken'), (row) => row?.failure, FIRST_RUN_MS);
  const failed = page.locator('[data-note]', { hasText: "broken didn't render" });
  await failed.first().waitFor({ timeout: 15_000 });
  await page.waitForTimeout(21_000);
  await look();
  const ofBroken = [...notes].filter(([note, text]) => !before.has(note) && /broken/.test(text)).map(([, text]) => text);
  seen.broken = { notes: ofBroken, up: await failed.count(), copy: await failed.first().locator('.copy-error').count().catch(() => 0) };
  check('one_note_for_a_failed_run', ofBroken.length === 1, seen.broken);
  check('the_failure_stays_past_20s', seen.broken.up === 1, seen.broken);
  check('the_failure_has_copy', seen.broken.copy === 1, seen.broken);
  await shot('broken');

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch(died);
