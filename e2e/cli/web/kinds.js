// A tool goes on a box of the kind it makes (cl-881, Paul 11-11: "Tools are attached to their output type of node, not to their input type of
// node"; tools §9), in a real browser on a real Calcifer. Calcifer refuses a video tool on a picture, naming what it makes, before the project has
// read the tool. Let go on a box of another kind (the palette's Video on a picture, its Image on a video), a tool changes nothing and makes no
// box, and a note at the bottom left says why (cl-899). BiRefNet on the picture starts from `<self.1>`.
//   node kinds.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER.
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots. Prints what it saw as JSON and exits 1
// if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { mkdtempSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { implementationRow, projectFromPage, calcifer, carry, signIn, MP4, STEP_MS, boxOf, fitCanvas, lastLook, launch, lookAt, noStall, png, until, upload } = require('./page');

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

(async () => {
  const base = calcifer();
  const browser = await launch(playwright);
  const context = await browser.newContext({ viewport: { width: 1600, height: 1100 } });
  const page = await context.newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
  page.on('console', (m) => { if (m.type() === 'error' && !/status of 4\d\d|net::ERR_/.test(m.text())) errors.push(`console: ${m.text()}`); });
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());

  await signIn(page);
  await page.waitForSelector('.top');
  const held = await page.evaluate(() => JSON.parse(sessionStorage.getItem('cozy.session') ?? 'null'));
  const api = (path, init = {}) => page.evaluate(async ({ path, init, token }) => {
    const answer = await fetch(path, { ...init, headers: { ...(token ? { Authorization: `Bearer ${token}`, 'cozy-project-storage': '3' } : {}), 'Content-Type': 'application/json' } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  }, { path, init, token: held?.token });

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-kinds-${Date.now()}`);
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const at = `/v1/projects/${id}`;
  const state = async () => (await api(at)).body;
  const asset = async (name) => (await state())?.assets?.find((row) => row.name === name);
  seen.project = { id, folder: (await state()).path };
  const box = (name) => boxOf(page, name);

  // A picture, a box of its own.
  const pictures = mkdtempSync(join(tmpdir(), 'web-kinds-'));
  writeFileSync(join(pictures, 'pic.png'), png(90));
  await api(`${at}/imports`, { method: 'POST', body: JSON.stringify({ source: await upload(held?.token, join(pictures, 'pic.png')), name: 'pic', exact: true, at: { x: 100, y: 100 } }) });
  await box('pic').waitFor();

  /** Nothing open. */
  const leave = async () => {
    if (!(await page.locator('.card.here').count())) return;
    await page.keyboard.press('Escape');
    await page.locator('.card.here').waitFor({ state: 'detached' });
  };

  // A clip, a video box of its own.
  writeFileSync(join(pictures, 'clip.mp4'), MP4);
  await api(`${at}/imports`, { method: 'POST', body: JSON.stringify({ source: await upload(held?.token, join(pictures, 'clip.mp4')), name: 'clip', exact: true, at: { x: 700, y: 100 } }) });
  await box('clip').waitFor();

  // The palette's Video, as Orbit is a video tool: its model is the first the palette has (H3's, a default).
  const videoId = await (await implementationRow(page, 'video')).getAttribute('data-tool');
  seen.video_tool = videoId;

  // Calcifer refuses it on the picture, naming what it makes, before this project has read the tool: nothing has made a box of it.
  const refused = await api(`${at}/assets/pic`, { method: 'PATCH', body: JSON.stringify({ tool: videoId }) });
  seen.refused = { status: refused.status, error: refused.body?.error };
  check('calcifer_refuses_another_kind', refused.status === 400 && refused.body?.error?.code === 'asset_tool_kind' && refused.body.error.metadata?.makes === 'video' && refused.body.error.message.includes('video'), seen.refused);
  check('still_a_picture', !(await asset('pic'))?.tool && (await asset('pic'))?.kind === 'image', await asset('pic'));

  // Let go on a box of another kind, a tool changes nothing and makes no box: a note at the bottom left says why (cl-899).
  const notes = page.locator('.notes .note .line');
  const dropOnto = async (operation, name, says, tag) => {
    await leave();
    await fitCanvas(page);
    await box(name).waitFor();
    const row = await implementationRow(page, operation);
    const tool = await row.getAttribute('data-tool');
    const before = (await state()).assets;
    const was = before.find((held) => held.name === name);
    const to = await box(name).locator('.card').boundingBox();
    await carry(page, row, { x: to.x + to.width / 2, y: to.y + to.height / 2 });
    const note = await until(`${tag}: the note`, () => notes.allInnerTexts(), (lines) => lines.some((line) => line.includes(says)), 10_000).catch(() => null);
    const where = await notes.filter({ hasText: says }).first().boundingBox().catch(() => null);
    const view = page.viewportSize();
    await page.waitForTimeout(500);
    const after = (await state()).assets;
    const now = after.find((held) => held.name === name);
    const same = (row) => row && JSON.stringify({ tool: row.tool ?? null, kind: row.kind, fields: row.fields, versions: row.versions, sha256: row.sha256 });
    seen[tag] = { tool, note, where, was: same(was), now: same(now), names: after.map((held) => held.name) };
    check(`${tag}_says_why`, Boolean(note), seen[tag]);
    check(`${tag}_note_at_the_bottom_left`, Boolean(where && where.x < view.width / 2 && where.y + where.height > view.height * 0.6), { where, view });
    check(`${tag}_leaves_the_box_as_it_was`, Boolean(was) && same(was) === same(now), seen[tag]);
    check(`${tag}_makes_no_box`, after.length === before.length, seen[tag].names);
    await shot(tag);
  };
  await dropOnto('video', 'pic', 'makes video: drop it on a video box', 'video_tool_on_a_picture');
  await dropOnto('image', 'clip', 'makes images: drop it on an image box', 'image_tool_on_a_video');

  // An image tool on the picture starts from it: BiRefNet's picture slot is `<self.1>` (tools §9).
  const searched = (await api(`/v1/tools?project=${id}&q=birefnet`)).body?.tools ?? [];
  const cutout = searched.find((tool) => tool.package === 'birefnet' && tool.makes === 'image');
  seen.birefnet_tool = cutout?.id ?? searched.map((tool) => tool.id);
  if (!cutout) throw new Error('the Hub lists no BiRefNet tool');
  const put = await api(`${at}/assets/pic`, { method: 'PATCH', body: JSON.stringify({ tool: cutout.id }) });
  const on = await until('BiRefNet on it', () => asset('pic'), (held) => held?.tool === cutout.id).catch(() => null);
  seen.birefnet = { status: put.status, error: put.body?.error, asset: on && { tool: on.tool, kind: on.kind, fields: on.fields, versions: on.versions } };
  check('birefnet_starts_from_self', on?.fields?.image === '<self.1>' && on.kind === 'image', seen.birefnet);
  await leave();
  await shot('birefnet-on-picture');

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch((why) => died(why.stack ?? why));
