// A rendering box shows what its run is making (cr-282): the newest preview of its latents, faded under how far it's got,
// asked for while the box shows it, and moving as the run does; its take replaces it. On the fixture tool project/latents.py,
// rendered on this computer.
//   node previews.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $COZY: the CLI; $SHOT: where to keep its screenshots; $ALONE: on a
// shared computer, so it runs nothing; $ARGV_LOG: where each cozy call is logged for coverage. Prints what it saw as JSON and exits 1 if
// anything failed.
const { chromium } = require(process.env.PLAYWRIGHT);
const { execFile } = require('child_process');
const { appendFileSync } = require('fs');
const { join } = require('path');
const { projectFolder, calcifer, lastLook, lookAt, projectAction, projectFromPage, signIn, STEP_MS, until, canvasBox, canvasView, canvasNames, selectedBoxes, hoverBox, screenshotPixels } = require('./page');

const failures = [];
const seen = {};
let during = 'the start';
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  during = name;
  if (!ok) failures.push(name);
}

const RUN_MS = 300_000;
const TOOL = join(__dirname, '..', 'project', 'latents.py');

/** The box named `name` as the page draws it: its preview, if one shows, and whether how far it's got is over it. */
const LOOK = (name) => {
  const box = document.querySelector('.canvas-root')?.canvasInspection?.card(name), drawn = box?.rendered;
  return { drawn: Boolean(drawn), preview: drawn?.latent ?? null, progress: Boolean(drawn?.progress), making: ['queued', 'running'].includes(drawn?.state), take: Boolean(drawn?.media.ready), body: drawn?.body };
};

(async () => {
  const base = calcifer();
  const browser = await chromium.launch({ executablePath: process.env.CHROME, headless: true });
  const page = await (await browser.newContext({ viewport: { width: 1600, height: 1000 } })).newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  seen.page_errors = errors;
  page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());

  await signIn(page);
  await page.waitForSelector('.top');
  const { token } = await page.evaluate(() => JSON.parse(sessionStorage.getItem('cozy.session')));
  const api = async (path) => (await fetch(base + path, { headers: { Authorization: `Bearer ${token}`, 'cozy-project-storage': '3' } })).json().catch(() => null);
  const local = Boolean((await api('/v1/status'))?.local_usable) && !process.env.ALONE;
  seen.ran = local;
  if (!local) {
    console.log(JSON.stringify({ failures, seen }, null, 2));
    await browser.close();
    return;
  }

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s folder"]', projectFolder(`web-previews-${Date.now()}`));
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const folder = (await api(`/v1/projects/${id}`)).path;
  seen.project = { id, folder };
  const cozy = (...words) => new Promise((done, fail) => {
    if (process.env.ARGV_LOG) appendFileSync(process.env.ARGV_LOG, `${words.map((word) => `${word}\x1f`).join('')}\x1e`);
    execFile(process.env.COZY, words, { cwd: folder, encoding: 'utf8', maxBuffer: 1 << 26, timeout: RUN_MS }, (error, stdout, stderr) => {
      if (error) fail(new Error(`cozy ${words.join(' ')}: ${error.message}: ${stdout}${stderr}`));
      else done({ stdout, stderr });
    });
  });
  const project = (...words) => projectAction(folder, ...words);
  const look = async () => {
    await page.mouse.move(4, 4);
    const now = await page.evaluate(LOOK, 'shown');
    if (now.body && (now.preview || now.take)) {
      const read = screenshotPixels(await page.screenshot()), b = now.body;
      now.pixels = [[0.25,0.25],[0.75,0.25],[0.25,0.75],[0.75,0.75]].map(([x,y]) => read(b.x + b.width*x, b.y + b.height*y));
    }
    return now;
  };

  // Its environment readied first, so the render watched is all steps.
  during = 'readying the tool';
  await cozy('--json', 'run', `${TOOL}/latents`, 'steps:=1', 'seconds:=0', '--on', 'local');
  await project('add', `${TOOL}/latents`, 'shown', 'at=700,120,320', 'instructions=A denoised still', 'steps:=24', 'seconds:=0.5');
  await until('its box', look, (now) => now.drawn);
  await project('render', 'shown', '--on', 'local', '--queue');

  during = 'a preview on its box';
  const first = await until('a preview on its box', look, (now) => now.preview !== null || now.take, RUN_MS);
  await shot('previewing');
  seen.previews = { first };
  check('preview_pixels_are_not_a_flat_placeholder', first.pixels?.some(color => color.some((value, c) => Math.abs(value - first.pixels[0][c]) > 12)), first.pixels);
  check('a_preview_shows_while_it_renders', first.preview !== null && !first.take, first);
  check('the_preview_is_under_how_far_it_has_got', first.progress, first);
  check('the_preview_is_faded', first.preview !== null && first.preview.opacity < 1, first.preview);
  check('the_preview_is_its_latents_size', first.preview?.width === 64, first.preview);

  during = 'the preview moving';
  const tags = new Set(first.preview ? [first.preview.tag] : []);
  const moved = await until('a newer preview', async () => {
    const now = await look();
    if (now.preview) tags.add(now.preview.tag);
    return now;
  }, (now) => tags.size >= 2 || now.take, RUN_MS);
  seen.previews.tags = [...tags];
  check('newer_previews_replace_it', tags.size >= 2 && JSON.stringify(first.pixels) !== JSON.stringify(moved.pixels), { tags: [...tags], now: moved });

  during = 'the take replacing it';
  const landed = await until('its take', look, (now) => now.take && !now.preview && !now.making, RUN_MS);
  await shot('landed');
  check('its_take_replaces_the_preview', landed.take && !landed.preview, landed);
  check('no_page_errors', errors.length === 0, errors);

  console.log(JSON.stringify({ failures, seen }, null, 2));
  await browser.close();
  process.exit(failures.length ? 1 : 0);
})().catch(died);

async function died(error) {
  console.error(error);
  await lastLook();
  console.log(JSON.stringify({ failures, seen, during }, null, 2));
  process.exit(2);
}
