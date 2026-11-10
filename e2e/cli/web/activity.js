// What each machine is doing, top right (Paul, 11-06): `local` among the machines, a click lists what it does, an install and a download each
// once, under the machine doing them, with the runs that wait on them, and a box whose render waits on its install says so instead of a
// percentage. Where this computer runs models; the download is SDXL's 335 MB VAE from Hugging Face, kept as a local model.
//   node activity.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $COZY: the CLI; $SHOT: where to keep its screenshots; $ALONE: on a shared
// computer, so it runs nothing. Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { spawn } = require('child_process');
const { writeFileSync } = require('fs');
const { join } = require('path');
const { calcifer, importYaml, signIn, STEP_MS, lastLook, launch, lookAt, noStall, openBox, renderOf, until } = require('./page');

const failures = [];
const seen = {};
noStall(seen, died);
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  if (!ok) failures.push(name);
}
const COZY = process.env.COZY;
// One this machine doesn't hold: a held source is no download.
const VAE = 'hf://stabilityai/sdxl-vae/diffusion_pytorch_model.safetensors';
const VAE_SAID = 'Sdxl Vae';
// A picture tool of its own, new to the machine, so its environment is installed while the box waits. It takes no GPU. A tool as
// e2e/cli/project/draw.py is, on the 0.22 line's Runtime (0.43.1: fields from cozy_runtime.fields, a picture's request from cozy_runtime.image).
const TOOL = `# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43.1", "msgspec>=0.19"]
# ///
"""The activity cell's picture tool: a weightless still of one random colour."""

import os

import msgspec
from cozy_runtime.author import Adjustments, App, ImageFrame, Outputs, Seed
from cozy_runtime.fields import Image, Prompt
from cozy_runtime.image import ImageRequest, ImageResult, Sizes

app = App()
SIZES = Sizes((0.25, 4.0), (64, 8192), 16, "the still's 0.25-4 MP")


class Drawing(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image] = Prompt()
    seed: Seed | None = None


@app.tool(label="Activity", named="waits", accelerator=False)
def draw(payload: Drawing, adj: Adjustments, out: Outputs) -> ImageResult:
    width, height = ImageRequest(payload.instructions.text, None, None, payload.seed).size(adj, None, SIZES, megapixels=0.25)
    image = out.save_image(ImageFrame(width, height, os.urandom(3) * (width * height)), format="png")
    return ImageResult(image, width, height, payload.seed or 0, payload.instructions.text)
`;

(async () => {
  const base = calcifer();
  const browser = await launch(playwright);
  const page = await (await browser.newContext({ viewport: { width: 1400, height: 900 } })).newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
  page.on('console', (m) => { if (m.type() === 'error' && !/status of 4\d\d|net::ERR_/.test(m.text())) errors.push(`console: ${m.text()}`); });
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());
  await signIn(page);
  await page.waitForSelector('.top');
  const { token } = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session')));
  const api = async (path) => (await fetch(base + path, { headers: { Authorization: `Bearer ${token}` } })).json();

  // `local` is a machine at the top right, whatever it's doing, where this computer runs models: macOS and Windows list none.
  const chip = page.locator('.mtab[data-machine="local"]');
  const usable = (await api('/v1/status')).local_usable;
  if (usable) await chip.waitFor();
  check('local_has_a_chip', (await chip.count()) === (usable ? 1 : 0), { usable, chips: await page.locator('.mtab').allInnerTexts() });
  if (!usable || process.env.ALONE) {
    seen.ran = false;
    return finish();
  }
  seen.ran = true;

  await page.goto(`${base}/#/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-activity-${Date.now()}`);
  await page.click('form button[type=submit]');
  await page.waitForURL(/#\/projects\/[^/]+$/);
  const id = page.url().match(/#\/projects\/([^/]+)/)[1];
  const folder = (await api(`/v1/projects/${id}`)).path;
  seen.project = { id, folder };
  const tool = join(folder, `activity_${Date.now()}.py`);
  writeFileSync(tool, TOOL);
  importYaml(folder, `version: 4\nassets:\n  waits:\n    tool: ${tool}/draw\n`);

  // Its render waits on its package's install, a second or two from uv's cache: the box says so, and the chip's list has the install once,
  // with the run that waits on it. Everything they say is kept as it shows, so a short install is seen whole.
  await page.locator('.card[data-asset="waits"]').waitFor();
  await page.evaluate(() => {
    const said = (window.said = { box: [], rows: [], marks: [] });
    const keep = (list, text) => text && !list.includes(text) && list.push(text);
    new MutationObserver(() => {
      keep(said.box, document.querySelector('.card[data-asset="waits"] .prog b')?.textContent);
      for (const row of document.querySelectorAll('.activity [data-doing]')) keep(said.rows, `${row.dataset.doing}: ${row.innerText.replace(/\s+/g, ' ')}`);
      for (const mark of document.querySelectorAll('.mtab[data-machine="local"] .mark, .mtab[data-machine="local"] .dot')) keep(said.marks, mark.className);
    }).observe(document.body, { subtree: true, childList: true, characterData: true, attributes: true });
  });
  await openBox(page, 'waits');
  await (await renderOf(page, 'waits')).click();
  await page.keyboard.press('Escape');
  await chip.click();
  const list = page.locator('.activity');
  await list.waitFor();
  // A render that fails ends the wait with its own words.
  const waits = async () => (await api(`/v1/projects/${id}`)).assets.find((asset) => asset.name === 'waits');
  const first = await until('its first take', waits, (asset) => asset?.version || asset?.failure, 180_000);
  if (!first.version) throw new Error(`its render failed: ${first.failure.message}`);
  const said = await page.evaluate(() => window.said);
  seen.said = said;
  check('box_said_installing', said.box.some((text) => /^Installing activity_\d+…$/.test(text)), said.box);
  check('install_listed_with_its_waiter', said.rows.some((row) => /^install: activity_\d+ Install .* for run \d+$/.test(row)), said.rows);
  check('a_run_waited_on_it', said.rows.some((row) => /^run: .*Waits on the install of activity_\d+$/.test(row)), said.rows);
  check('chip_marked_an_install', said.marks.includes('mark install'), said.marks);
  check('install_done_leaves_the_list', (await list.locator('[data-doing=install]').count()) === 0);

  // A download: listed once, by the bytes it fetches, with its rate and time left, and the run that waits on it.
  const name = `local/activity-vae-${Date.now()}`;
  const download = spawn(COZY, ['model', 'download', VAE, name, '--on', 'local'], { cwd: folder, stdio: 'ignore' });
  const ended = new Promise((done) => download.on('exit', done));
  const fetching = list.locator('[data-doing=download]');
  await until('the download listed under local', () => fetching.count(), (n) => n === 1, 120_000);
  const row = await until('its bytes moving', () => fetching.first().innerText(), (text) => /[1-9][\d.]* [KMG]iB \/ /.test(text), 60_000);
  seen.download = row;
  check('download_counts_its_bytes', /iB \/ \d+ MiB/.test(row), row);
  check('download_names_its_waiter', /for run \d+/.test(row), row);
  check('download_named_as_people_say_it', row.startsWith(VAE_SAID), row);
  check('its_run_waits_on_the_download', (await list.locator('[data-doing=run]').filter({ hasText: `Waits on the download of ${VAE_SAID}` }).count()) === 1, await list.innerText());
  check('chip_marks_a_download', (await chip.locator('.mark.download').count()) === 1);
  check('a_download_is_listed_once', (await fetching.count()) === 1);
  await shot('download');

  // Escape closes it, and so does a click elsewhere.
  await page.keyboard.press('Escape');
  check('escape_closes', (await list.count()) === 0);
  await chip.click();
  await list.waitFor();
  await page.mouse.click(700, 600);
  check('a_click_elsewhere_closes', (await list.count()) === 0);

  const code = await ended;
  check('download_ran', code === 0, code);
  // Nothing of it stays listed once it's done.
  await chip.click();
  await until('the download gone from the list', () => list.locator('[data-doing=download]').count(), (n) => n === 0, 30_000);
  check('done_leaves_the_list', true);
  return finish();

  async function finish() {
    check('no_page_errors', errors.length === 0, errors);
    console.log(JSON.stringify({ failures, seen }, null, 2));
    await browser.close();
    process.exit(failures.length ? 1 : 0);
  }
})().catch(died);

async function died(error) {
  console.error(error);
  await lastLook();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(2);
}
