// A rendering box at rest, and new takes not looked at (cl-928, Paul 11-12), in a real browser on a real Calcifer with a weightless local
// tool. At rest a render breathes around its box, outside its edge, with a meter of the whole job once there's an estimate. With nothing
// approved, a box shows its new take and no dot. Approved,
// it keeps showing the approved take while it renders and after, with a green dot left of its name and on the new take in Focus's list;
// a reload keeps them (Calcifer's own state.db, never the project); looking at the take clears both, and a reload keeps that too.
//   node newtakes.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots; $ALONE: on a shared computer, so
// no run. Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { writeFileSync } = require('fs');
const { join } = require('path');
const { calcifer, importYaml, lastLook, launch, lookAt, noStall, openBox, projectFromPage, renderOf, signIn, until, yamlOf, STEP_MS } = require('./page');

const FIRST_RUN_MS = 300_000;
// A still made in steps of `pace` s, so a render lasts long enough to be seen at rest.
const TOOL = `# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43.1", "msgspec>=0.19"]
# ///
"""The newtakes cell's picture tool: a weightless still of one random colour, made in steps."""

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
    pace: float = 0.8


@app.tool(label="Takes", named="takes", accelerator=False)
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
  await page.fill('input[aria-label="New project\'s name"]', `web-newtakes-${Date.now()}`);
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

  const tool = join(folder, `newtakes_${Date.now()}.py`);
  writeFileSync(tool, TOOL);
  importYaml(folder, `version: 4\nassets:\n  card:\n    tool: ${tool}/draw\n    instructions: A still of one colour.\n    pace: 0.6\n`);
  const rest = page.locator('.react-flow__node .card[data-asset="card"]');
  const here = page.locator('.card.here[data-asset="card"]');
  const take = (number) => here.locator(`.take[data-take="${number}"]`);
  const nameDots = () => rest.locator('.head .unseen-dot').count();
  const label = () => rest.locator('.node-version-label').textContent().catch(() => null);
  const leave = async () => {
    for (let i = 0; i < 3 && (await here.count()); i++) {
      await page.keyboard.press('Escape');
      await page.waitForTimeout(200);
    }
    await here.waitFor({ state: 'detached' });
    await rest.waitFor();
  };
  // Its box at rest, each change: rendering, its wave (a sibling of its picture, the picture's size, breathing) and the take it shows.
  const watch = () => page.evaluate(() => {
    const said = (window.said = []);
    const look = () => {
      const card = document.querySelector('.react-flow__node .card[data-asset="card"]');
      if (!card) return;
      const wave = card.querySelector(':scope > .render-wave');
      const media = card.querySelector(':scope > .media');
      const [w, m] = [wave?.getBoundingClientRect(), media?.getBoundingClientRect()];
      const now = {
        rendering: card.classList.contains('rendering'),
        wave: Boolean(wave),
        outside: Boolean(wave && media && !media.contains(wave) && Math.abs(w.top - m.top) < 1.5 && Math.abs(w.bottom - m.bottom) < 1.5 && Math.abs(w.width - m.width) < 1.5),
        breathes: wave ? getComputedStyle(wave, '::before').animationName : '',
        shows: card.querySelector('.node-version-label')?.textContent ?? '',
        line: card.querySelector('.media .run-face .face-machine')?.textContent ?? '',
        meter: card.querySelector('.media .run-face .face-meter')?.getAttribute('aria-valuenow') ?? null,
      };
      if (JSON.stringify(said.at(-1)) !== JSON.stringify(now)) said.push(now);
    };
    new MutationObserver(look).observe(document.body, { subtree: true, childList: true, attributes: true, characterData: true });
    look();
  });
  const render = async (count) => {
    await openBox(page, 'card');
    await watch();
    await (await renderOf(page, 'card')).click();
    await leave();
    const row = await until(`card's take ${count}`, asset, (now) => now?.versions === count && !now.making?.length, FIRST_RUN_MS);
    await until(`its wave gone with take ${count}`, () => rest.locator('.render-wave').count(), (n) => n === 0, 15_000).catch(() => {});
    return { row, said: await page.evaluate(() => window.said) };
  };

  // Nothing approved: at rest it breathes while it renders, then shows its new take, with no dot.
  const first = await render(1);
  seen.first_render = first.said;
  const going = first.said.filter((now) => now.rendering);
  check('rendering_breathes_at_rest', going.length > 0 && going.every((now) => now.wave && now.outside && now.breathes === 'render-wave'), first.said);
  check('wave_gone_after', (await rest.locator('.render-wave').count()) === 0 && !(await rest.evaluate((card) => card.classList.contains('rendering'))));
  check('first_take_shown', (await until('take 1 shown', label, (text) => text === 'v1', 15_000).catch(() => null)) === 'v1', await label());
  check('first_take_no_dot', (await nameDots()) === 0 && !first.row.unseen?.length, first.row);
  await shot('first');

  // Approved, a new take lands beside it: the box keeps the approved take, while it renders and after, and says it has one unseen.
  await openBox(page, 'card');
  await take(1).dblclick();
  await until('take 1 approved', asset, (row) => row.pinned && row.approved === 1);
  await leave();
  const second = await render(2);
  seen.second_render = second.said;
  const kept = second.said.filter((now) => now.rendering);
  check('approved_breathes_and_stays_shown', kept.length > 0 && kept.every((now) => now.wave && now.outside && now.shows === 'v1'), second.said);
  // Its second run has an estimate (the first's): a meter of the whole job, only with line 1's 'so far / whole job', and it never goes back.
  const metered = kept.filter((now) => now.meter !== null).map((now) => ({ ...now, meter: Number(now.meter) }));
  check('meter_of_the_whole_job', metered.length > 0 && metered.every((now, i) => now.meter >= 0 && now.meter <= 99 && (i === 0 || now.meter >= metered[i - 1].meter) && / - \d+[hms]( \d+[ms])? \/ \d+[hms]/.test(now.line)), kept);
  check('approved_take_still_shown', (await label()) === 'v1', await label());
  check('new_take_unseen', second.row.approved === 1 && JSON.stringify(second.row.unseen) === '[2]', second.row);
  check('name_dot', (await until('a dot by its name', nameDots, (n) => n === 1, 15_000).catch(() => 0)) === 1);
  await shot('unseen');

  // A reload keeps it: it's Calcifer's, not the page's. It's no project data: the export says nothing of it.
  await page.reload();
  await rest.waitFor();
  check('reload_keeps_name_dot', (await until('the dot after a reload', nameDots, (n) => n === 1, 15_000).catch(() => 0)) === 1);
  check('not_in_the_project', !/unseen/.test(yamlOf(folder)), yamlOf(folder));

  // Open: the approved take is what's shown, and only the new take has a dot in its list.
  await openBox(page, 'card');
  await take(2).waitFor();
  check('focus_shows_approved', (await page.locator('.focus-version-label').textContent()) === 'v1', await page.locator('.focus-version-label').textContent());
  check('take_dot', (await take(2).locator('.take-unseen').count()) === 1 && (await take(1).locator('.take-unseen').count()) === 0);
  await shot('focus-unseen');

  // Looking at it clears both dots, here and after a reload.
  await take(2).click();
  const looked = await until('take 2 seen', asset, (row) => !row.unseen?.length, 15_000).catch(() => null);
  check('looking_clears_it', looked !== null && looked.approved === 1, looked ?? (await asset()));
  check('take_dot_cleared', (await until('its take dot gone', () => take(2).locator('.take-unseen').count(), (n) => n === 0, 15_000).catch(() => 1)) === 0);
  await leave();
  check('name_dot_cleared', (await nameDots()) === 0);
  check('approved_shown_again', (await label()) === 'v1', await label());
  await page.reload();
  await rest.waitFor();
  await page.waitForTimeout(1000);
  check('reload_keeps_it_seen', (await nameDots()) === 0 && !(await asset()).unseen?.length);
  await shot('seen');

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch(died);
