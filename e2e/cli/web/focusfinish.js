// The take looked at in Focus is the one shown when it finishes (cl-942, Paul 11-12: "a persistent annoying bug"), in a real browser on a
// real Calcifer with a weightless local tool. Two takes finished, the third rendered from Focus and looked at as it's made: when it lands,
// the viewer's picture is the third, as the takes panel and the version label say, and no other take's picture shows in between. The
// same with Focus left and opened again while it renders (the box approved, so its take lands beside the approved one, unseen), and
// with it opened by the finish toast's View. The viewer's picture is what the page draws (`.buffered-image`'s displayed file), not a label.
//   node focusfinish.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots; $ALONE: on a shared computer, so
// no run. Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { writeFileSync } = require('fs');
const { join } = require('path');
const { addBoxes, calcifer, lastLook, launch, lookAt, noStall, openBox, projectFromPage, renderOf, signIn, until, STEP_MS } = require('./page');

const FIRST_RUN_MS = 300_000;
const TOOL = `# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43.1", "msgspec>=0.19"]
# ///
"""The focusfinish cell's picture tool: a weightless still of one random colour, made in steps."""

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


@app.tool(label="Finish", named="finish", accelerator=False)
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
  await page.fill('input[aria-label="New project\'s name"]', `web-focusfinish-${Date.now()}`);
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const { id, path: folder } = await projectFromPage(page);
  seen.project = { id, folder };
  const session = await page.evaluate(() => JSON.parse(sessionStorage.getItem('cozy.session') ?? 'null'));
  const api = async (path) => {
    const answer = await fetch(base + path, { headers: { Authorization: `Bearer ${session.token}`, 'cozy-project-storage': '3' } });
    const value = await answer.json().catch(() => null);
    if (!answer.ok) throw new Error(`GET ${path}: ${answer.status} ${JSON.stringify(value)}`);
    return value;
  };
  const asset = async () => (await api(`/v1/projects/${id}`)).assets.find((row) => row.name === 'card');
  if (!(await api('/v1/status')).local_usable || process.env.ALONE) {
    check('renders_here', false, 'no local machine to render the weightless tool on');
    throw new Error('nothing to render on');
  }

  const tool = join(folder, `focusfinish_${Date.now()}.py`);
  writeFileSync(tool, TOOL);
  // Its kind given, Calcifer reads nothing of the tool before its first render installs it.
  addBoxes(folder, [{ name: 'card', tool: `${tool}/draw`, kind: 'image', fields: { instructions: 'A still of one colour.' } }]);
  const here = page.locator('.card.here[data-asset="card"]');
  const take = (number) => here.locator(`.here-side .take[data-take="${number}"]`);
  const toast = (text) => page.locator('.notes .note', { hasText: text });
  const leave = async () => {
    for (let i = 0; i < 3 && (await here.count()); i++) {
      await page.keyboard.press('Escape');
      await page.waitForTimeout(200);
    }
    await here.waitFor({ state: 'detached' });
  };
  const shaOf = (row, number) => row.prefixes.find((version) => version.number === number)?.main?.sha256?.replace(/^sha256:/, '');

  // What Focus's viewer draws, each time it changes: whether it's waiting on a take being made, whether its pixels can be seen, the file
  // its picture is, the version its label says and the take its panel has picked.
  await page.evaluate(() => {
    const log = (window.viewerLog = []);
    const look = () => {
      const focus = document.querySelector('[data-focused-asset="card"]');
      if (!focus) return { open: false };
      const media = focus.querySelector('.focus-viewer .focus-image-transform > .media');
      const pixels = media?.querySelector(':scope > .media-pixels');
      const host = pixels?.querySelector('.buffered-image');
      const picture = host?.querySelector('img');
      return {
        open: true,
        pending: Boolean(media?.classList.contains('pending-image')),
        visible: Boolean(pixels && picture && getComputedStyle(pixels).visibility !== 'hidden'),
        sha: (picture && host.dataset.displayedSha?.replace(/^sha256:/, '')) || null,
        loaded: Boolean(picture?.complete && picture.naturalWidth > 0),
        label: focus.querySelector('.focus-version-label')?.textContent ?? null,
        on: focus.querySelector('.here-side .take.on[data-take]')?.getAttribute('data-take') ?? null,
        making: Boolean(focus.querySelector('.here-side .take.making.on')),
      };
    };
    window.viewerNow = look;
    const note = () => {
      const now = look();
      const last = log.at(-1);
      if (!last || JSON.stringify({ ...last, t: 0 }) !== JSON.stringify({ ...now, t: 0 })) log.push({ ...now, t: Math.round(performance.now()) });
    };
    new MutationObserver(note).observe(document.body, { subtree: true, childList: true, attributes: true, characterData: true });
    note();
  });
  const mark = () => page.evaluate(() => window.viewerLog.length);
  const logFrom = (from) => page.evaluate((from) => window.viewerLog.slice(from), from);
  const now = () => page.evaluate(() => window.viewerNow());
  // Take `number`, drawn in the viewer as its label and panel say, its picture loaded.
  const showing = (number, sha) => until(`take ${number} drawn in Focus`, now, (look) => look.visible && look.loaded && look.sha === sha && look.label === `v${number}` && look.on === String(number), 20_000).catch(() => null);
  // Every look from the take being made on (or all of them, `fromStart`) whose pixels show a picture: only take `number`'s.
  const strays = (log, sha, fromStart = false) => {
    const begun = fromStart ? 0 : log.findIndex((look) => look.pending);
    return { begun, others: begun < 0 ? [] : log.slice(begun).filter((look) => look.visible && look.sha && look.sha !== sha) };
  };
  const render = async () => {
    await openBox(page, 'card');
    await (await renderOf(page, 'card')).click();
  };
  const landed = (count) => until(`card's take ${count}`, asset, (row) => row?.versions === count && !row.making?.length, FIRST_RUN_MS);

  // Two takes finished, made from Focus.
  await render();
  await landed(1);
  await render();
  const two = await landed(2);
  check('two_takes_finished', Boolean(shaOf(two, 1) && shaOf(two, 2)), two.prefixes);

  // A: the third, rendered from Focus and looked at as it's made. When it finishes, the viewer draws it and nothing else.
  const fromA = await mark();
  await (await renderOf(page, 'card')).click();
  await until('take 3 looked at as it is made', now, (look) => look.pending && look.making, 30_000);
  await shot('a-making');
  const three = await landed(3);
  const sha3 = shaOf(three, 3);
  const finalA = await showing(3, sha3);
  const logA = await logFrom(fromA);
  const strayA = strays(logA, sha3);
  seen.a = { sha3, earlier: [shaOf(three, 1), shaOf(three, 2)], strays: strayA.others.slice(0, 4), final: finalA ?? (await now()) };
  check('a_finished_take_is_drawn', finalA !== null, seen.a.final);
  check('a_no_other_take_drawn_on_finish', strayA.begun >= 0 && strayA.others.length === 0, { begun: strayA.begun, strays: strayA.others.slice(0, 4), log: logA.slice(-12) });
  await shot('a-finished');

  // B: approved, its next take rendered from Focus, Focus left and opened again while it's made. When it finishes, it's drawn.
  await take(1).dblclick();
  await until('take 1 approved', asset, (row) => row.pinned && row.approved === 1);
  await (await renderOf(page, 'card')).click();
  await until('take 4 being made', asset, (row) => row.making?.length === 1, 30_000);
  await leave();
  await page.waitForTimeout(500);
  const fromB = await mark();
  await openBox(page, 'card');
  const reopened = await until('take 4 looked at again, still being made', now, (look) => look.pending && look.making, 15_000).catch(() => null);
  check('b_reopened_on_the_take_being_made', reopened !== null && ((await asset()).making?.length ?? 0) === 1, reopened ?? (await now()));
  const four = await landed(4);
  const sha4 = shaOf(four, 4);
  const finalB = await showing(4, sha4);
  const logB = await logFrom(fromB);
  const strayB = strays(logB, sha4);
  seen.b = { sha4, approved: four.approved, strays: strayB.others.slice(0, 4), final: finalB ?? (await now()) };
  check('b_finished_take_is_drawn', finalB !== null && four.approved === 1, seen.b.final);
  check('b_no_other_take_drawn_on_finish', strayB.begun >= 0 && strayB.others.length === 0, { begun: strayB.begun, strays: strayB.others.slice(0, 4), log: logB.slice(-12) });
  const looked = await until('take 4 looked at', asset, (row) => !row.unseen?.length, 15_000).catch(() => null);
  check('b_looked_at_clears_its_dot', looked !== null, (await asset()).unseen);

  // C: the next one finishes away from Focus; the toast's View opens it on that take, and only that take is drawn.
  await (await renderOf(page, 'card')).click();
  await until('take 5 being made', asset, (row) => row.making?.length === 1, 30_000);
  await leave();
  await landed(5);
  const note = toast('card finished: version 5');
  await note.waitFor({ timeout: 15_000 });
  const five = await asset();
  const sha5 = shaOf(five, 5);
  const fromC = await mark();
  await note.locator('button:text-is("View")').click();
  await here.waitFor();
  const finalC = await showing(5, sha5);
  const logC = await logFrom(fromC);
  const strayC = strays(logC, sha5, true);
  seen.c = { sha5, strays: strayC.others.slice(0, 4), final: finalC ?? (await now()) };
  check('c_view_draws_that_take', finalC !== null, seen.c.final);
  check('c_no_other_take_drawn', strayC.others.length === 0, { strays: strayC.others.slice(0, 4), log: logC.slice(-12) });
  await shot('c-view');

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch(died);
