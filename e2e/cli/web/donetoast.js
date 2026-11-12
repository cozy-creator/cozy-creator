// A toast when a render finishes (cl-937, Paul 11-12: "so I can view the result and I know when it's done"), in a real browser on a real
// Calcifer with a weightless local tool. A take landing says so bottom left: its box, its take, a small picture of it and View, which opens
// the box in Focus on that take and, the take being one that landed beside an approved one, clears its dot. A take landing while it's the
// one looked at isn't said. A render that fails says so, with its error to copy.
//   node donetoast.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
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
"""The donetoast cell's picture tool: a weightless still of one random colour, made in steps; asked to, it fails."""

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
    pace: float = 0.5
    fail: bool = False


@app.tool(label="Toast", named="toast", accelerator=False)
def draw(ctx: Context, payload: Drawing, adj: Adjustments, out: Outputs, tel: Telemetry) -> ImageResult:
    width, height = ImageRequest(payload.instructions.text, None, None, payload.seed).size(adj, None, SIZES, megapixels=0.25)
    with tel.stage("denoise"):
        for step in range(6):
            time.sleep(payload.pace)
            ctx.progress(step, 6)
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
  await page.fill('input[aria-label="New project\'s name"]', `web-donetoast-${Date.now()}`);
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const { id, path: folder } = await projectFromPage(page);
  seen.project = { id, folder };
  const session = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session') ?? 'null'));
  const api = async (path, method = 'GET', body) => {
    const answer = await fetch(base + path, {
      method,
      headers: { Authorization: `Bearer ${session.token}`, 'Content-Type': 'application/json', ...(method === 'POST' ? { 'Idempotency-Key': `donetoast-${Date.now()}-${Math.random()}` } : {}) },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const value = await answer.json().catch(() => null);
    if (!answer.ok) throw new Error(`${method} ${path}: ${answer.status} ${JSON.stringify(value)}`);
    return value;
  };
  const asset = async (name) => (await api(`/v1/projects/${id}`)).assets.find((row) => row.name === name);
  if (!(await api('/v1/status')).local_usable || process.env.ALONE) {
    check('renders_here', false, 'no local machine to render the weightless tool on');
    throw new Error('nothing to render on');
  }

  const tool = join(folder, `donetoast_${Date.now()}.py`);
  writeFileSync(tool, TOOL);
  // Their kind given, Calcifer reads nothing of the tool before its first render installs it.
  addBoxes(folder, [
    { name: 'card', tool: `${tool}/draw`, kind: 'image', fields: { instructions: 'A still of one colour.' } },
    { name: 'broken', tool: `${tool}/draw`, kind: 'image', fields: { instructions: 'Never made.', fail: true } },
  ]);
  const here = page.locator('.card.here[data-asset="card"]');
  const toast = (text) => page.locator('.notes .note', { hasText: text });
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
  const versions = async (name) => (await asset(name))?.versions ?? 0;

  // Its first take, rendered and left: it says so, with a picture of it and View.
  await render('card');
  await leave();
  await until('card has a take', () => versions('card'), (n) => n === 1, FIRST_RUN_MS);
  const first = toast('card finished');
  await first.waitFor({ timeout: 15_000 });
  const picture = await until('its picture', () => first.locator('img.note-picture').evaluate((img) => img.complete && img.naturalWidth > 0).catch(() => false), Boolean, 10_000).catch(() => false);
  check('finished_toast', /card finished: version 1/.test(await first.innerText()) && (await first.locator('button:text-is("View")').count()) === 1 && picture, await first.innerText());
  await shot('finished');
  await first.locator('button:text-is("View")').click();
  await here.waitFor();
  check('view_opens_it', (await page.locator('.focus-version-label').textContent()) === 'v1', await page.locator('.focus-version-label').textContent());

  // Approved, its next take lands beside it, unseen: View opens it on that take and its dot goes.
  await here.locator('.take[data-take="1"]').dblclick();
  await until('take 1 approved', () => asset('card'), (row) => row.pinned && row.approved === 1);
  await render('card');
  await leave();
  const second = await until('take 2 unseen', () => asset('card'), (row) => row.versions === 2 && JSON.stringify(row.unseen) === '[2]', FIRST_RUN_MS);
  const next = toast('card finished: version 2');
  await next.waitFor({ timeout: 15_000 });
  await next.locator('button:text-is("View")').click();
  await here.waitFor();
  const looked = await until('take 2 looked at', () => asset('card'), (row) => !row.unseen?.length, 15_000).catch(() => null);
  check('view_opens_that_take_and_clears_its_dot', second.approved === 1 && (await page.locator('.focus-version-label').textContent()) === 'v2' && looked !== null, { label: await page.locator('.focus-version-label').textContent(), looked });

  // Rendered and watched where it lands, nothing is said of it.
  const said = await page.locator('.notes .note').count();
  await (await renderOf(page, 'card')).click();
  await until('take 3', () => versions('card'), (n) => n === 3, FIRST_RUN_MS);
  await page.waitForTimeout(2500);
  check('watched_take_not_said', (await toast('card finished: version 3').count()) === 0, { before: said, notes: await page.locator('.notes .note').allInnerTexts() });
  await leave();

  // A render that fails says so, with its error to copy.
  await render('broken');
  await leave();
  await until('broken failed', () => asset('broken'), (row) => row?.failure, FIRST_RUN_MS);
  const failed = toast("broken didn't render");
  await failed.waitFor({ timeout: 15_000 });
  check('failure_toast_with_copy', (await failed.locator('.copy-error').count()) === 1, await failed.innerText());
  await shot('failed');

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch(died);
