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
const { holderToken, projectFolder, addBoxes, calcifer, lastLook, launch, lookAt, noStall, openBox, projectFromPage, renderOf, signIn, storedOf, until, STEP_MS } = require('./page');

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
  await page.fill('input[aria-label="New project\'s folder"]', projectFolder(`web-newtakes-${Date.now()}`));
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
  const asset = async () => (await api(`/v1/projects/${id}`)).assets.find((row) => row.name === 'card');
  if (!(await api('/v1/status')).local_usable || process.env.ALONE) {
    check('renders_here', false, 'no local machine to render the weightless tool on');
    throw new Error('nothing to render on');
  }

  const tool = join(folder, `newtakes_${Date.now()}.py`);
  writeFileSync(tool, TOOL);
  // Its kind given, Calcifer reads nothing of the tool before its first render installs it.
  addBoxes(folder, [{ name: 'card', tool: `${tool}/draw`, kind: 'image', fields: { instructions: 'A still of one colour.', pace: 0.6 } }]);
  const rest = page.locator('.react-flow__node .card[data-asset="card"]');
  const here = page.locator('.card.here[data-asset="card"]');
  const take = (number) => here.locator(`.take[data-take="${number}"]`);
  const nameDots = () => rest.locator('.head .state-dot.unseen').count();
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
      if (!m) return;
      const now = {
        rendering: card.classList.contains('rendering'),
        wave: Boolean(wave),
        outside: Boolean(wave && media && !media.contains(wave) && Math.abs(w.top - m.top) < 1.5 && Math.abs(w.bottom - m.bottom) < 1.5 && Math.abs(w.width - m.width) < 1.5),
        breathes: wave ? getComputedStyle(wave, '::before').animationName : '',
        shows: card.querySelector('.node-version-label')?.textContent ?? '',
        line: card.querySelector('.run-face .face-machine')?.textContent ?? '',
        meter: card.querySelector('.tile-meter')?.getAttribute('aria-valuenow') ?? null,
        // The meter along the top of the box, its picture's top edge (Paul, 11-12).
        meterAtTop: (() => { const at = card.querySelector('.tile-meter')?.getBoundingClientRect(); return at ? Math.abs(at.top - m.top) < 2 : null; })(),
        size: (() => {
          const zoom = new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform).a;
          const [c, n] = [card.getBoundingClientRect(), card.closest('.react-flow__node').getBoundingClientRect()];
          return [c.width, c.height, n.width, n.height].map((v) => Math.round((v / zoom) * 10) / 10).join('x');
        })(),
      };
      if (JSON.stringify(said.at(-1)) !== JSON.stringify(now)) said.push(now);
    };
    // Its state dot, wherever the box is shown: left of its name at rest, or in Focus's header. Each change, and where it was.
    const states = (window.states = []);
    const dot = () => {
      const rest = document.querySelector('.react-flow__node .card[data-asset="card"]');
      const focus = document.querySelector('[data-focused-asset="card"] .focus-title');
      const [held, name] = rest ? [rest.querySelector('.head .state-dot'), rest.querySelector('.head .nm')] : focus ? [focus.querySelector('.state-dot'), focus.querySelector('.focus-name')] : [null, null];
      if (!rest && !focus) return;
      const now = { state: held?.dataset.state ?? '', where: rest ? 'rest' : 'focus', left: held && name ? held.getBoundingClientRect().right <= name.getBoundingClientRect().left + 0.5 : null };
      if (states.at(-1)?.state !== now.state || states.at(-1)?.where !== now.where) states.push(now);
    };
    new MutationObserver(() => { look(); dot(); }).observe(document.body, { subtree: true, childList: true, attributes: true, characterData: true });
    look();
    dot();
  });
  const render = async (count) => {
    await openBox(page, 'card');
    await watch();
    await (await renderOf(page, 'card')).click();
    await leave();
    const row = await until(`card's take ${count}`, asset, (now) => now?.versions === count && !now.making?.length, FIRST_RUN_MS);
    await until(`its wave gone with take ${count}`, () => rest.locator('.render-wave').count(), (n) => n === 0, 15_000).catch(() => {});
    return { row, said: await page.evaluate(() => window.said), states: await page.evaluate(() => window.states) };
  };

  // Nothing approved: at rest it breathes while it renders, then shows its new take, with no dot.
  const first = await render(1);
  seen.first_render = first.said;
  const going = first.said.filter((now) => now.rendering);
  check('rendering_breathes_at_rest', going.length > 0 && going.every((now) => now.wave && now.outside && now.breathes === 'render-wave'), first.said);
  check('wave_gone_after', (await rest.locator('.render-wave').count()) === 0 && !(await rest.evaluate((card) => card.classList.contains('rendering'))));
  check('first_take_shown', (await until('take 1 shown', label, (text) => text === 'v1', 15_000).catch(() => null)) === 'v1', await label());
  check('first_take_no_dot', (await nameDots()) === 0 && !first.row.unseen?.length && !first.states.some((now) => now.state === 'unseen'), { row: first.row, states: first.states });
  await shot('first');

  // Approved, a new take lands beside it: the box keeps the approved take, while it renders and after, and says it has one unseen.
  await openBox(page, 'card');
  await take(1).dblclick();
  await until('take 1 approved', asset, (row) => row.pinned && row.approved === 1);
  await leave();
  const sized = await rest.evaluate((card) => {
    const zoom = new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform).a;
    const [c, n] = [card.getBoundingClientRect(), card.closest('.react-flow__node').getBoundingClientRect()];
    return [c.width, c.height, n.width, n.height].map((v) => Math.round((v / zoom) * 10) / 10).join('x');
  });
  const second = await render(2);
  seen.second_render = second.said;
  const kept = second.said.filter((now) => now.rendering);
  check('approved_breathes_and_stays_shown', kept.length > 0 && kept.every((now) => now.wave && now.outside && now.shows === 'v1'), second.said);
  // Its second run has an estimate (the first's): a meter of the whole job, only with line 1's 'so far / whole job', and it never goes back.
  const metered = kept.filter((now) => now.meter !== null).map((now) => ({ ...now, meter: Number(now.meter) }));
  check('meter_of_the_whole_job', metered.length > 0 && metered.every((now, i) => now.meter >= 0 && now.meter <= 99 && (i === 0 || now.meter >= metered[i - 1].meter) && / - \d+[hms]( \d+[ms])? \/ \d+[hms]/.test(now.line) && now.meterAtTop), kept);
  // Its dot, left of its name (Paul, 11-12): grey queued, blue running, then green made and not looked at.
  const order = second.states.map((now) => now.state);
  const at = (state, from = 0) => order.indexOf(state, from);
  check('dot_queued_running_unseen', at('queued') >= 0 && at('running', at('queued')) > at('queued') && at('unseen', at('running', at('queued'))) > at('running', at('queued')) && second.states.every((now) => now.left !== false), second.states);
  check('approved_take_still_shown', (await label()) === 'v1', await label());
  // Its whole render, from the ask to the take landing beside it, leaves its size as it was (cl-936).
  check('render_leaves_its_size', second.said.filter((now) => now.size).every((now) => now.size === sized), { sized, sizes: [...new Set(second.said.map((now) => now.size))] });
  check('new_take_unseen', second.row.approved === 1 && JSON.stringify(second.row.unseen) === '[2]', second.row);
  check('name_dot', (await until('a dot by its name', nameDots, (n) => n === 1, 15_000).catch(() => 0)) === 1);
  await shot('unseen');

  // A reload keeps it: it's Calcifer's, not the page's. It's no project data: project.db's boxes say nothing of it.
  await page.reload();
  await rest.waitFor();
  check('reload_keeps_name_dot', (await until('the dot after a reload', nameDots, (n) => n === 1, 15_000).catch(() => 0)) === 1);
  check('not_in_the_project', !/unseen/.test(JSON.stringify(storedOf(folder))), storedOf(folder));

  // Open: the approved take is what's shown, and only the new take has a dot in its list.
  await openBox(page, 'card');
  await take(2).waitFor();
  check('focus_shows_approved', (await page.locator('.focus-version-label').textContent()) === 'v1', await page.locator('.focus-version-label').textContent());
  check('take_dot', (await take(2).locator('.take-unseen').count()) === 1 && (await take(1).locator('.take-unseen').count()) === 0);
  check('focus_dot_left_of_its_name', await page.locator('[data-focused-asset="card"] .focus-title').evaluate((title) => {
    const [dot, name] = [title.querySelector('.state-dot.unseen'), title.querySelector('.focus-name')];
    return Boolean(dot && name && dot.getBoundingClientRect().right <= name.getBoundingClientRect().left + 0.5);
  }));
  await shot('focus-unseen');

  // Looking at it clears both dots, here and after a reload.
  await take(2).click();
  const looked = await until('take 2 seen', asset, (row) => !row.unseen?.length, 15_000).catch(() => null);
  check('looking_clears_it', looked !== null && looked.approved === 1, looked ?? (await asset()));
  check('take_dot_cleared', (await until('its take dot gone', () => take(2).locator('.take-unseen').count(), (n) => n === 0, 15_000).catch(() => 1)) === 0);
  await leave();
  check('name_dot_cleared', (await nameDots()) === 0 && (await rest.locator('.head .state-dot').count()) === 0);
  check('approved_shown_again', (await label()) === 'v1', await label());
  await page.reload();
  await rest.waitFor();
  // Its take drawn again, from the same read that carries its dots: only then does no dot mean something.
  await until('its take drawn again', label, (text) => text === 'v1', 15_000).catch(() => {});
  check('reload_keeps_it_seen', (await nameDots()) === 0 && !(await asset()).unseen?.length);
  await shot('seen');

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch(died);
