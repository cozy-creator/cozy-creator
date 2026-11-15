// Deleting is a trash can; ✕ only closes (cl-939, Paul 11-12: "if you want a delete button it should probably be a trash can icon ... This can
// be undone with the undo history"), in a real browser on a real Calcifer. A video with two takes, playing in Focus: its player stops by a
// stop square, not ✕; the playing take's discard is a trash can, discards it, and Undo brings it back; Focus's close is ✕. On the canvas, a
// box's delete is a trash can.
//   node trash.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER.
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots. Prints what it saw as JSON and exits 1
// if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { mkdtempSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { holderToken, projectFolder, projectFromPage, calcifer, signIn, DARK_OPEN, MP4, STEP_MS, boxOf, closeBox, fitCanvas, lastLook, launch, lookAt, noStall, openBox, until, upload } = require('./page');

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
  const held = { token: holderToken() };
  const api = (path, init = {}) => page.evaluate(async ({ path, init }) => {
    const answer = await fetch(path, { ...init, headers: { 'cozy-project-storage': '3', 'Content-Type': 'application/json' } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  }, { path, init });

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s folder"]', projectFolder(`web-trash-${Date.now()}`));
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const at = `/v1/projects/${id}`;
  const takes = async () => ((await api(at)).body?.assets?.find((row) => row.name === 'shot')?.prefixes ?? []).map((version) => version.number);
  seen.project = { id, folder: (await api(at)).body.path };
  const post = async (path, body) => {
    const answer = await api(path, { method: 'POST', body: JSON.stringify(body) });
    if (answer.status >= 300) throw new Error(`${path}: ${JSON.stringify(answer)}`);
    return answer.body;
  };

  // A video with two takes: H3's Shot.
  const tools = (await api(`/v1/tools?project=${id}&q=minimax`)).body?.tools ?? [];
  const shotTool = tools.find((tool) => tool.id === 'minimax-h3/generate') ?? tools.find((tool) => tool.package === 'minimax-h3' && tool.makes === 'video');
  if (!shotTool) throw new Error(`the Hub lists no H3 Shot: ${JSON.stringify(tools.map((tool) => tool.id))}`);
  const folder = mkdtempSync(join(tmpdir(), 'web-trash-'));
  // Two different files: the same one twice is one take.
  writeFileSync(join(folder, 'take1.mp4'), MP4);
  writeFileSync(join(folder, 'take2.mp4'), DARK_OPEN);
  await post(`${at}/assets`, { tool: shotTool.id, kind: 'video', name: 'shot', exact: true, fields: { summary: 'a lighthouse at dusk', beats: [{ description: 'Waves.', duration: 5 }] }, at: { x: 100, y: 100 } });
  for (const file of ['take1.mp4', 'take2.mp4']) await post(`${at}/assets/shot/versions`, { source: await upload(held?.token, join(folder, file), id) });
  await boxOf(page, 'shot').waitFor();
  await until('two takes', takes, (numbers) => numbers.length === 2);
  await fitCanvas(page);

  // On the canvas, a box's delete is a trash can.
  const canvasDelete = await boxOf(page, 'shot').locator('button[aria-label="Delete"] svg').first().getAttribute('data-icon').catch(() => null);
  seen.canvas_delete = canvasDelete;
  check('a_boxs_delete_is_a_trash_can', canvasDelete === 'trash', canvasDelete);

  // Watched in Focus: the take's discard is a trash can; Focus's close is ✕.
  await openBox(page, 'shot');
  const focus = page.locator('.focus-viewport');
  const close = await focus.locator('button[aria-label="Close focus"] svg').getAttribute('data-icon');
  seen.focus_close = close;
  check('focus_close_is_x', close === 'close', close);
  // Playing, its player stops by a stop square, not ✕.
  const play = focus.locator('button[aria-label="Play"]').first();
  if (await play.count()) await play.click();
  const stop = focus.locator('button[aria-label="Stop"]').first();
  await stop.waitFor({ state: 'attached', timeout: 15_000 }).catch(() => {});
  const stopIcon = await stop.locator('svg').getAttribute('data-icon').catch(() => null);
  const stopTitle = await stop.getAttribute('title').catch(() => null);
  seen.stop = { icon: stopIcon, title: stopTitle };
  check('the_players_stop_is_a_stop_square', stopIcon === 'stop' && stopTitle === 'Stop', seen.stop);

  // The take playing is the one deleted: by its trash can, and Undo brings it back.
  const playing = Number(await focus.locator('.take[aria-pressed="true"][data-take]').first().getAttribute('data-take', { timeout: 5_000 }).catch(() => null));
  seen.playing_take = { playing, paused: await focus.locator('video').first().evaluate((video) => video.paused).catch(() => null) };
  // The take the player is on is pressed in its column; with none, the newest stands in only so the rest can still be judged.
  check('a_take_is_the_one_playing', playing > 0, seen.playing_take);
  const second = playing || Math.max(...(await takes()));
  const tile = focus.locator(`.take[data-take="${second}"]`).first();
  await tile.hover();
  const discard = focus.locator(`button[aria-label="Discard take ${second}"]`).first();
  await discard.waitFor({ state: 'visible' });
  const icon = await discard.locator('svg').getAttribute('data-icon');
  const title = await discard.getAttribute('title');
  seen.discard = { icon, title };
  check('a_takes_discard_is_a_trash_can_saying_what_it_discards', icon === 'trash' && /Discard take/.test(title ?? ''), seen.discard);
  await shot('focus-trash');
  await discard.click();
  const after = await until('the take discarded', takes, (numbers) => numbers.length === 1).catch(() => takes());
  seen.after_discard = after;
  check('it_discards_the_playing_take', after.length === 1 && !after.includes(second), after);

  // Undo brings it back.
  await page.locator('button[aria-label="Undo"]').first().click();
  const back = await until('the take back', takes, (numbers) => numbers.length === 2).catch(() => takes());
  seen.after_undo = back;
  check('undo_brings_the_take_back', back.length === 2 && back.includes(second), back);
  await closeBox(page);

  // On the canvas, the box's trash can deletes it, saying so, and Undo brings it back.
  const exists = async () => Boolean((await api(at)).body?.assets?.some((row) => row.name === 'shot'));
  const tileDelete = boxOf(page, 'shot').locator('button[aria-label="Delete"]').first();
  const tileTitle = await tileDelete.getAttribute('title');
  seen.tile_delete = { icon: await tileDelete.locator('svg').getAttribute('data-icon'), title: tileTitle };
  check('the_tiles_delete_is_a_trash_can_saying_delete', seen.tile_delete.icon === 'trash' && /^Delete/.test(tileTitle ?? ''), seen.tile_delete);
  await tileDelete.click();
  const gone = await until('the box deleted', exists, (there) => !there).then(() => true, () => false);
  check('the_tiles_trash_can_deletes_the_box', gone);
  await page.locator('button[aria-label="Undo"]').first().click();
  const again = await until('the box back', exists, Boolean).then(() => true, () => false);
  check('undo_brings_the_box_back', again);
  await shot('canvas-undone');

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch((why) => died(why.stack ?? why));
