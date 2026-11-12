// A tool goes on a box of the kind it makes (cl-881, Paul 11-11: "Tools are attached to their output type of node, not to their input type of
// node"; tools §9), in a real browser on a real Calcifer. Calcifer refuses a video tool on a picture, naming what it makes, before the project has
// read the tool; the palette's Video let go on the picture makes a new video box beside it that mentions the picture, which stays as it was;
// BiRefNet on the picture starts from `<self.1>`.
//   node kinds.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER.
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots. Prints what it saw as JSON and exits 1
// if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { mkdtempSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { projectFromPage, calcifer, signIn, STEP_MS, boxOf, fitCanvas, lastLook, launch, lookAt, noStall, png, until, upload } = require('./page');

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
  const held = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session') ?? 'null'));
  const api = (path, init = {}) => page.evaluate(async ({ path, init, token }) => {
    const answer = await fetch(path, { ...init, headers: { ...(token ? { Authorization: `Bearer ${token}` } : {}), 'Content-Type': 'application/json' } });
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
  /** Where `tool` reads a picture, as tools §9 says: its first required picture slot, else its first prompt. */
  const readsIn = (tool) => (tool.fields.find((field) => field.required && field.type === 'slot' && field.accept === 'image') ?? tool.fields.find((field) => field.type === 'prompt'))?.field;

  // The palette's Video, as Orbit is a video tool: its model is the first the palette has (H3's, a default).
  const row = page.locator('.tools [data-palette="tools"] .tool[data-operation="video"]');
  await row.waitFor();
  const videoId = await row.getAttribute('data-tool');
  const listed = (await api(`/v1/tools?project=${id}`)).body?.tools ?? [];
  const video = listed.find((tool) => tool.id === videoId);
  seen.video_tool = { id: videoId, reads: video && readsIn(video), makes: video?.makes };
  if (!video || !readsIn(video)) throw new Error(`the palette's Video (${videoId}) reads no picture: ${JSON.stringify(video?.fields?.map((field) => field.field))}`);

  // Calcifer refuses it on the picture, naming what it makes, before this project has read the tool: nothing has made a box of it.
  const refused = await api(`${at}/assets/pic`, { method: 'PATCH', body: JSON.stringify({ tool: videoId }) });
  seen.refused = { status: refused.status, error: refused.body?.error };
  check('calcifer_refuses_another_kind', refused.status === 400 && refused.body?.error?.code === 'asset_tool_kind' && refused.body.error.metadata?.makes === 'video' && refused.body.error.message.includes('video'), seen.refused);
  check('still_a_picture', !(await asset('pic'))?.tool && (await asset('pic'))?.kind === 'image', await asset('pic'));

  // Let go on the picture, it's a new video box beside it that mentions it, and the picture stays as it was.
  await leave();
  await fitCanvas(page);
  await box('pic').waitFor();
  await row.scrollIntoViewIfNeeded();
  const before = (await state()).assets.map((held) => held.name);
  const from = await row.boundingBox();
  await page.mouse.move(from.x + 20, from.y + 8);
  await page.mouse.down();
  await page.mouse.move(from.x + 40, from.y + 20, { steps: 4 });
  const to = await box('pic').locator('.card').boundingBox();
  await page.mouse.move(to.x + to.width / 2, to.y + to.height / 2, { steps: 12 });
  await page.mouse.up();
  const made = await until('a box of it', async () => (await state()).assets.find((held) => !before.includes(held.name)), Boolean).catch(() => null);
  seen.made = made && { name: made.name, kind: made.kind, tool: made.tool, fields: made.fields };
  check('a_video_tool_on_a_picture_makes_a_video_box', made?.tool === videoId && made.kind === 'video' && made.fields?.node_kind === 'video', seen.made);
  check('the_new_box_mentions_the_picture', made?.fields?.[readsIn(video)] === '<pic>', { reads: readsIn(video), fields: made?.fields });
  check('it_shows', made ? await box(made.name).waitFor().then(() => true, (why) => String(why)) === true : false);
  const cards = (await api(`${at}/canvas`)).body?.cards ?? {};
  seen.cards = { pic: cards.pic, made: made && cards[made.name] };
  check('it_is_beside_the_picture', Boolean(made && cards[made.name] && cards.pic && cards[made.name].x >= cards.pic.x + (cards.pic.w ?? 0)), seen.cards);
  const picture = await asset('pic');
  check('the_picture_stays_as_it_was', !picture?.tool && picture?.kind === 'image', picture);
  await shot('video-beside');

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
