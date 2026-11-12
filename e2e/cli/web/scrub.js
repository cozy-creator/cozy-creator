// A video in Focus picks up from where its player is (cl-933, cl-950, Paul 11-12: "when you are viewing a video in focused mode, you
// can display the pick up from here, probably put that underneath the thumbnail. Also give us a scrubber of some sort. Also if you click on
// an empty image / empty video node, with nothing in it, there is no point in displaying 'instructions' or anything"), in a real browser on a
// real Calcifer. H3's Shot with a four-second take: in Focus, scrubbed a frame at a time, the time says where, and Pick up from here then
// Render asks to keep up to that frame (on the tool's grid). An empty picture and an empty video with no tool, in Focus, ask nothing.
//   node scrub.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER.
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots. Prints what it saw as JSON and exits 1
// if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { execFileSync } = require('child_process');
const { mkdtempSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { projectFromPage, calcifer, signIn, STEP_MS, boxOf, closeBox, fitCanvas, lastLook, launch, lookAt, noStall, openBox, until, upload } = require('./page');

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

/** Where a pickup lands on a tool's frame grid: its last point at or before `frame` (ranges.ts pickupAt, cl-950). */
const pickupAt = (frame, last, snap) => {
  const at = Math.min(frame, last);
  return !snap ? at : at < snap.first ? null : Math.max(snap.first, snap.offset + Math.floor((at - snap.offset) / snap.every) * snap.every);
};
/** Seconds as the button says them, "0:12.96". */
const clock = (seconds) => `${Math.floor(seconds / 60)}:${(seconds % 60).toFixed(2).padStart(5, '0')}`;

(async () => {
  const base = calcifer();
  const browser = await launch(playwright);
  const context = await browser.newContext({ viewport: { width: 1600, height: 1100 } });
  const page = await context.newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
  page.on('console', (m) => { if (m.type() === 'error' && !/status of 4\d\d|status of 5\d\d|net::ERR_/.test(m.text())) errors.push(`console: ${m.text()}`); });
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());

  await signIn(page);
  await page.waitForSelector('.top');
  const held = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session') ?? 'null'));
  const api = (path, init = {}) => page.evaluate(async ({ path, init, token }) => {
    const answer = await fetch(path, { ...init, headers: { ...(token ? { Authorization: `Bearer ${token}` } : {}), 'Content-Type': 'application/json' } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  }, { path, init, token: held?.token });

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-scrub-${Date.now()}`);
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const at = `/v1/projects/${id}`;
  seen.project = { id, folder: (await api(at)).body.path };
  const post = async (path, body) => {
    const answer = await api(path, { method: 'POST', body: JSON.stringify(body) });
    if (answer.status >= 300) throw new Error(`${path}: ${JSON.stringify(answer)}`);
    return answer.body;
  };

  // H3's Shot with a four-second take of its own.
  const folder = mkdtempSync(join(tmpdir(), 'web-scrub-'));
  const clip = join(folder, 'take.mp4');
  execFileSync('ffmpeg', ['-v', 'error', '-y', '-f', 'lavfi', '-i', 'testsrc2=size=320x180:rate=24:duration=4', '-c:v', 'libx264', '-pix_fmt', 'yuv420p', clip]);
  const tools = (await api(`/v1/tools?project=${id}&q=minimax`)).body?.tools ?? [];
  const shotTool = tools.find((tool) => tool.id === 'minimax-h3/generate') ?? tools.find((tool) => tool.package === 'minimax-h3' && tool.pickup);
  seen.tool = shotTool && { id: shotTool.id, pickup: shotTool.pickup };
  if (!shotTool?.pickup) throw new Error(`the Hub lists no H3 tool that picks up: ${JSON.stringify(tools.map((tool) => [tool.id, tool.pickup]))}`);
  await post(`${at}/assets`, { tool: shotTool.id, kind: 'video', name: 'shot', exact: true, fields: { summary: 'a lighthouse at dusk', beats: [{ description: 'Waves.', duration: 4 }] }, at: { x: 100, y: 100 } });
  await post(`${at}/assets/shot/versions`, { source: await upload(held?.token, clip) });
  // An empty picture and an empty video with no tool.
  await post(`${at}/assets`, { tool: '', kind: 'image', name: 'blank_pic', exact: true, fields: { node_kind: 'image' }, at: { x: 600, y: 100 } });
  await post(`${at}/assets`, { tool: '', kind: 'video', name: 'blank_clip', exact: true, fields: { node_kind: 'video' }, at: { x: 1000, y: 100 } });
  for (const name of ['shot', 'blank_pic', 'blank_clip']) await boxOf(page, name).waitFor();
  await until('the take', async () => (await api(at)).body?.assets?.find((row) => row.name === 'shot')?.versions, (n) => n >= 1);
  await fitCanvas(page);

  // In Focus, right under the player's bottom right: Pick up from where the player is (cl-950). One scrubber, the player's own.
  await openBox(page, 'shot');
  const focus = page.locator('.focus-viewport');
  const pickUp = focus.locator('.focus-under button.pick-up');
  await pickUp.waitFor();
  check('one_scrubber_the_players_own', (await focus.locator('.video-scrub, input[type=range]').count()) === 0, await focus.locator('input[type=range]').count());
  const player = await focus.locator('.focus-image-transform > .media').first().boundingBox();
  const button = await pickUp.boundingBox();
  seen.placed = { player, button };
  check('pick_up_is_right_under_the_players_bottom_right', Math.abs(button.y - (player.y + player.height)) <= 2 && Math.abs(button.x + button.width - (player.x + player.width)) <= 2, seen.placed);
  check('pick_up_is_a_readable_button', button.height >= 28, button);

  // Its label follows the player: played a moment, then put at two seconds by the player's own position.
  const label = () => pickUp.innerText();
  seen.label_at_start = await label();
  const play = focus.locator('button[aria-label="Play"]').first();
  if (await play.count()) await play.click();
  const video = focus.locator('video').first();
  await video.waitFor({ state: 'attached' });
  await until('it plays', () => video.evaluate((element) => element.currentTime), (time) => time > 0.3).catch(() => null);
  seen.label_while_playing = await label();
  await video.evaluate((element) => { element.pause(); element.currentTime = 2; });
  // Where the cut lands: the tool's grid point at or before the player's 2 s, never after it.
  const wanted = pickupAt(48, 96, shotTool.pickup);
  const lands = `Pick up from ${clock(wanted / 24)}`;
  seen.wanted = { out_frame: wanted, lands };
  const said = await until('its label at 2 s', label, (text) => text.trim() === lands).catch(() => label());
  seen.label_at_2s = said;
  check('its_label_is_where_the_cut_lands_and_follows_the_player', said.trim() === lands && wanted <= 48 && seen.label_while_playing !== seen.label_at_start, { start: seen.label_at_start, playing: seen.label_while_playing, said, lands });
  await shot('pick-up-under');

  // Picked up: Render asks to keep up to that frame, the one the label said.
  const asked = page.waitForRequest((request) => request.method() === 'POST' && /\/v1\/projects\/[^/]+\/runs$/.test(new URL(request.url()).pathname), { timeout: STEP_MS });
  await page.route('**/runs', (route) => (route.request().method() === 'POST' ? route.fulfill({ status: 409, contentType: 'application/json', body: JSON.stringify({ error: { kind: 'conflict', code: 'e2e_held', message: 'held by the cell' } }) }) : route.continue()));
  await pickUp.click();
  await focus.locator('button[aria-label=Render]').first().click();
  const anyway = page.getByRole('button', { name: 'Run here anyway' });
  await Promise.race([asked, anyway.first().waitFor({ timeout: STEP_MS }).then(() => anyway.first().click())]).catch(() => {});
  const body = JSON.parse((await asked).postData() ?? '{}');
  seen.asked = body;
  check('pick_up_keeps_up_to_where_the_player_was', body.pickup?.out_frame === wanted && body.pickup?.take === '<shot.1>', { out: body.pickup, wanted });
  await page.unroute('**/runs');
  await closeBox(page);

  // An empty picture and an empty video with no tool ask nothing.
  for (const name of ['blank_pic', 'blank_clip']) {
    await openBox(page, name);
    await page.waitForTimeout(400);
    const asks = await page.locator('.focus-viewport .mention-text, .focus-viewport [data-field], .focus-viewport button[aria-label=Render]').count();
    seen[name] = { asks, text: (await focus.innerText()).slice(0, 300) };
    check(`${name}_with_no_tool_asks_nothing`, asks === 0, seen[name]);
    await shot(name);
    await closeBox(page);
  }

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch((why) => died(why.stack ?? why));
