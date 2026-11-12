// Right click, Download (cl-891, Paul 11-12), in a real browser on a real Calcifer: an ordinary browser download of the file's own bytes, named after
// its box. A box saves the version it shows as `hei.png`, another take beside the open box as `hei.2.png`, and, where this computer runs models, a run's
// output in its drawer as `image.png`. Calcifer answers `?download=` with `Content-Disposition: attachment` and a name no file system refuses.
//   node download.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $COZY: the CLI; $SHOT: where to keep its screenshots; $ALONE: on a shared
// computer, so it runs nothing. Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { execFileSync } = require('child_process');
const { createHash } = require('crypto');
const { mkdtempSync, readFileSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { projectFromPage, projectUrl, calcifer, signIn, STEP_MS, boxOf, lastLook, launch, lookAt, noStall, openBox, png, settled, until, upload } = require('./page');

const FIRST_RUN_MS = 300_000;
const failures = [];
const seen = {};
noStall(seen, died);
let during = 'the start';
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  during = name;
  if (!ok) failures.push(name);
}
async function died(why) {
  failures.push(`${during}: ${why}`);
  await lastLook();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(1);
}
const sha = (bytes) => createHash('sha256').update(bytes).digest('hex');
/** A digest as Calcifer says it, `sha256:<hex>`, as hex. */
const hex = (digest) => digest?.replace(/^sha256:/, '');

(async () => {
  const base = calcifer();
  const browser = await launch(playwright);
  const context = await browser.newContext({ viewport: { width: 1600, height: 1100 }, acceptDownloads: true });
  const page = await context.newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
  page.on('console', (m) => { if (m.type() === 'error' && !/status of 4\d\d|net::ERR_/.test(m.text())) errors.push(`console: ${m.text()}`); });
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());

  await signIn(page);
  await page.waitForSelector('.top');
  const held = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session') ?? 'null'));
  const api = (path, init = {}) => page.evaluate(async ({ path, init, token }) => {
    const answer = await fetch(path, { ...init, headers: { ...(token ? { Authorization: `Bearer ${token}` } : {}), 'Content-Type': 'application/json' } });
    return { status: answer.status, headers: Object.fromEntries(answer.headers), body: await answer.json().catch(() => null) };
  }, { path, init, token: held?.token });
  const post = (path, body) => api(path, { method: 'POST', body: JSON.stringify(body) });

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-download-${Date.now()}`);
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const at = `/v1/projects/${id}`;
  const folder = (await api(at)).body.path;
  seen.project = { id, folder };

  // `hei`: two takes, each its own picture.
  const pictures = mkdtempSync(join(tmpdir(), 'web-download-'));
  const picture = (shade) => {
    const path = join(pictures, `${shade}.png`);
    writeFileSync(path, png(shade));
    return path;
  };
  const takes = { 1: picture(40), 2: picture(80) };
  await post(`${at}/imports`, { source: await upload(held?.token, takes[1]), name: 'hei', exact: true, at: { x: 100, y: 100 } });
  await post(`${at}/assets/hei/versions`, { source: await upload(held?.token, takes[2]) });
  const row = await until('hei with two takes', async () => (await api(at)).body?.assets?.find((asset) => asset.name === 'hei'), (asset) => asset?.prefixes?.length === 2);
  const showing = row.prefixes.find((take) => take.number === row.version) ?? row.prefixes.at(-1);
  seen.showing = showing.number;

  /** A right click on `target`, and Download from its menu: what the browser saved, and as what. */
  const downloaded = async (target) => {
    await target.click({ button: 'right' });
    const menu = page.locator('.asset-menu');
    await menu.waitFor();
    const items = await menu.locator('[role=menuitem]').allTextContents();
    const [download] = await Promise.all([page.waitForEvent('download'), menu.getByRole('menuitem', { name: 'Download', exact: true }).click()]);
    const bytes = readFileSync(await download.path());
    return { items, name: download.suggestedFilename(), sha256: sha(bytes), bytes: bytes.length, closed: (await menu.count()) === 0 };
  };

  // A box: the version it shows, named after the box.
  await page.keyboard.press('f');
  await settled(page);
  const box = await downloaded(boxOf(page, 'hei').locator('.nm'));
  seen.box = box;
  check('box_menu_has_download', box.items.join() === 'Download,Star,Duplicate,Rename,Delete', box.items);
  check('box_saves_as_its_name', box.name === 'hei.png', box.name);
  check('box_saves_the_take_it_shows', box.sha256 === sha(readFileSync(takes[showing.number])) && box.sha256 === hex(showing.main.sha256), { box, showing: showing.main });
  check('menu_shuts', box.closed);

  // A take beside the open box, not the one it shows: its own file, named with its number.
  const other = showing.number === 1 ? 2 : 1;
  await openBox(page, 'hei');
  const take = page.locator(`.card.here[data-asset="hei"] .here-side .take[data-take="${other}"]`);
  await take.waitFor();
  const saved = await downloaded(take);
  seen.take = { ...saved, number: other };
  check('take_menu_is_download', saved.items.join() === 'Download', saved.items);
  check('take_saves_as_its_version', saved.name === `hei.${other}.png`, saved.name);
  check('take_saves_its_own_bytes', saved.sha256 === sha(readFileSync(takes[other])) && saved.sha256 !== box.sha256, saved);
  await shot('take');

  // Calcifer's answer: the file's own type, as an attachment, named so no file system refuses it.
  const file = `${at}/files/${showing.main.path.split('/').pop()}`;
  const plain = await api(file);
  const named = await api(`${file}?download=hei.2`);
  const odd = await api(`${file}?download=${encodeURIComponent('a"b/c\\ü')}`);
  seen.answers = { plain: plain.headers['content-disposition'], named: named.headers['content-disposition'], odd: odd.headers['content-disposition'] };
  check('shown_in_place_unasked', plain.headers['content-disposition'] === 'inline');
  check('download_is_an_attachment', named.status === 200 && named.headers['content-type'] === 'image/png' && named.headers['content-disposition'] === `attachment; filename="hei.2.png"; filename*=UTF-8''hei.2.png`, named.headers);
  check('download_name_is_safe', odd.headers['content-disposition'] === `attachment; filename="a_b_c__.png"; filename*=UTF-8''a_b_c_%C3%BC.png`, odd.headers['content-disposition']);

  // A run's output, in its drawer.
  const local = (await api('/v1/status')).body.local_usable && !process.env.ALONE;
  seen.ran = local;
  if (local) {
    const fixture = join(__dirname, '..', 'project', 'tool.py');
    execFileSync(process.env.COZY, ['--json', 'run', `${fixture}/still`, 'prompt=download', '--on', 'local'], { encoding: 'utf8', cwd: folder, stdio: ['ignore', 'pipe', 'inherit'], timeout: FIRST_RUN_MS });
    const run = await until('its output landed', async () => (await api('/v1/runs?limit=5')).body?.runs?.find((held) => held.target.includes('still') && held.project_id === id), (held) => held?.output?.some((item) => item.name === 'image' && item.url));
    const item = run.output.find((held) => held.name === 'image');
    const said = item.label || item.name;
    await page.goto(await projectUrl(id, `/runs/${run.number}`));
    const output = page.locator('figure.output').filter({ has: page.locator('.nm', { hasText: said }) }).first();
    await output.locator('button:has-text("Download")').waitFor();
    const out = await downloaded(output);
    seen.output = { ...out, item: item.sha256, said };
    check('output_saves_as_its_name', out.name === `${said}.png`, out.name);
    check('output_saves_its_bytes', out.sha256 === hex(item.sha256), { out, item });
    await shot('output');
  }

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch((why) => died(why.stack ?? why));
