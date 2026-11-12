// A video in Focus shows its take's segments and redoes from one, and a new box continues another's take (cl-933, cl-950, cl-951; Paul
// 11-12: "we can just regenerate entire segments, and nothing else … if we like segment 2 and 3, but want to regenerate segment 4, then we
// can do that"; "it has to be obvious that we're continuing another video"), in a real browser on a real Calcifer. shots.py's Shot,
// rendered here with no model, copies a 4 s clip and says its seam (frame 6: se-130's `seams`), as H3 does. In Focus its segments are
// blocks right under the player, the one the player is in marked; in segment 2, Redo, and Render keeps segment 1: `pickup.out_frame` is
// that seam, and the take it makes keeps it. Asked off a seam (`cozy run`), the tool starts at the seam before, and says so. The draft says what's kept and what's redone, and Cancel makes it a plain draft. Another box continues the take from its end, and
// its take says so. An empty picture and an empty video with no tool, in Focus, ask nothing.
//   node scrub.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER; $COZY: the cozy under test.
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots. Prints what it saw as JSON and exits 1
// if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { execFileSync } = require('child_process');
const { mkdtempSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { addBoxes, importFile, projectFromPage, calcifer, signIn, STEP_MS, boxOf, closeBox, fitCanvas, lastLook, launch, lookAt, noStall, openBox, until } = require('./page');

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

/** Seconds as the page says a cut, "0:04.46". */
const clock = (seconds) => `${Math.floor(seconds / 60)}:${(seconds % 60).toFixed(2).padStart(5, '0')}`;
const FIRST_RUN_MS = 10 * 60_000;
const SHOTS = join(__dirname, '..', 'project', 'shots.py');
const SEAM = 6;

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
  // Each changed view as Paul sees it (11-12: look at your own UI): 1440×900 and 1920×1080, paper and dark.
  const looks = async (name) => {
    if (!process.env.SHOT) return;
    for (const [width, height] of [[1440, 900], [1920, 1080]]) {
      await page.setViewportSize({ width, height });
      for (const theme of ['paper', 'dark']) {
        await page.evaluate((theme) => { if (theme === 'dark') document.documentElement.dataset.theme = 'dark'; else delete document.documentElement.dataset.theme; }, theme);
        await page.waitForTimeout(500);
        await page.screenshot({ path: `${process.env.SHOT}-${name}-${width}x${height}-${theme}.png` });
      }
    }
    await page.evaluate(() => { delete document.documentElement.dataset.theme; });
    await page.setViewportSize({ width: 1600, height: 1100 });
    await page.waitForTimeout(300);
  };

  await signIn(page);
  await page.waitForSelector('.top');
  const held = await page.evaluate(() => JSON.parse(sessionStorage.getItem('cozy.session') ?? 'null'));
  const api = (path, init = {}) => page.evaluate(async ({ path, init, token }) => {
    const answer = await fetch(path, { ...init, headers: { ...(token ? { Authorization: `Bearer ${token}`, 'cozy-project-storage': '3' } : {}), 'Content-Type': 'application/json', ...(init.headers ?? {}) } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  }, { path, init, token: held?.token });
  if (!(await api('/v1/status')).body?.local_usable) throw new Error('this computer runs no tools: the cell renders its own take here');

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-scrub-${Date.now()}`);
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const at = `/v1/projects/${id}`;
  const folder = (await api(at)).body.path;
  seen.project = { id, folder };
  const post = async (path, body, headers) => {
    const answer = await api(path, { method: 'POST', body: JSON.stringify(body), headers });
    if (answer.status >= 300) throw new Error(`${path}: ${JSON.stringify(answer)}`);
    return answer.body;
  };
  const row = async (name) => (await api(at)).body?.assets?.find((one) => one.name === name);
  const versionsOf = async (name) => (await api(`${at}/assets/${name}`)).body?.versions ?? [];

  // An empty picture and an empty video with no tool; a 4 s clip, shots.py's Shot of it, and a box to continue that.
  await post(`${at}/assets`, { tool: '', kind: 'image', name: 'blank_pic', exact: true, fields: { node_kind: 'image' }, at: { x: 600, y: 100 } });
  await post(`${at}/assets`, { tool: '', kind: 'video', name: 'blank_clip', exact: true, fields: { node_kind: 'video' }, at: { x: 1000, y: 100 } });
  const clip = join(mkdtempSync(join(tmpdir(), 'web-scrub-')), 'look.mp4');
  execFileSync('ffmpeg', ['-v', 'error', '-y', '-f', 'lavfi', '-i', 'testsrc2=size=320x180:rate=24:duration=4', '-c:v', 'libx264', '-pix_fmt', 'yuv420p', clip]);
  importFile(folder, clip, 'look');
  addBoxes(folder, [
    { name: 'shot', tool: `${SHOTS}/shoot`, kind: 'video', fields: { summary: 'On <look>.' }, at: { x: 100, y: 100 } },
    { name: 'next', tool: `${SHOTS}/shoot`, kind: 'video', fields: { summary: 'Then <look> again.' }, at: { x: 100, y: 600 } },
  ]);
  for (const name of ['shot', 'next', 'look', 'blank_pic', 'blank_clip']) await boxOf(page, name).waitFor();
  await fitCanvas(page);
  const focus = page.locator('.focus-viewport');
  const resultOf = async (name, number) => {
    const version = (await versionsOf(name)).find((one) => one.number === number);
    return { version, result: version?.runs?.length ? (await api(`${at}/runs/${version.runs.at(-1)}`)).body?.result : undefined };
  };
  // Render on its title row, here; this computer's tools ask first only when it's weak.
  const renderHere = async () => {
    await focus.locator('button[aria-label=Render]').first().click();
    const anyway = page.getByRole('button', { name: 'Run here anyway' });
    await anyway.first().waitFor({ timeout: 3000 }).then(() => anyway.first().click()).catch(() => {});
  };
  // Looking at a take, not its words: what a render from the words stays on (cl-907).
  const lookAtTake = async (number) => {
    const tile = focus.locator(`.take[aria-label="take ${number}"]`).first();
    if (await tile.count()) await tile.click();
  };

  // Rendered from the page: its run says where its segments start.
  await openBox(page, 'shot');
  await renderHere();
  await until('its take', async () => (await row('shot'))?.versions, (n) => n >= 1, FIRST_RUN_MS);
  // Its length, which Calcifer measures once the file is here, as nodes.js waits for its Clip's.
  await until('its length measured', () => row('shot'), (held) => held?.prefixes?.[0]?.main?.duration_ms > 0, FIRST_RUN_MS);
  const made = await resultOf('shot', 1);
  const main = (await row('shot')).prefixes[0].main;
  const fps = main.fps || 24;
  const last = Math.round((main.duration_ms / 1000) * fps);
  seen.take = { runs: made.version?.runs, seams: made.result?.seams, fps, last };
  check('its_run_says_its_seams', JSON.stringify(made.result?.seams) === `[${SEAM}]` && last > SEAM * 4, seen.take);
  await closeBox(page);

  // In Focus, right under the player: its take's segments as blocks, sized by their frames. One scrubber, the player's own.
  await openBox(page, 'shot');
  const segments = focus.locator('.focus-under .segments');
  await lookAtTake(1);
  // What the page reads them from: the take's run, its file, and the tool's pickup.
  const asset = await row('shot');
  const listed = ((await api(`/v1/tools?project=${id}&q=shots`)).body?.tools ?? []).find((tool) => tool.id === asset?.tool);
  seen.reads = { tool: asset?.tool, pickup: listed?.pickup, listed: Boolean(listed), prefixes: asset?.prefixes?.map((one) => ({ number: one.number, run: one.run, main: one.main && { field: one.main.field, duration_ms: one.main.duration_ms, fps: one.main.fps, width: one.main.width } })) };
  await segments.waitFor().catch(async (why) => { seen.under = await focus.locator('.focus-under').innerHTML().catch(() => null); throw why; });
  const blocks = segments.locator('.segment');
  const widths = await blocks.evaluateAll((all) => all.map((one) => one.getBoundingClientRect().width));
  seen.blocks = { said: await blocks.allInnerTexts(), widths };
  check('its_segments_are_blocks', JSON.stringify(seen.blocks.said) === '["1","2"]' && widths[1] > widths[0], seen.blocks);
  check('one_scrubber_the_players_own', (await focus.locator('.video-scrub, input[type=range]').count()) === 0, await focus.locator('input[type=range]').count());

  // The one the player is in is marked; in segment 1 there's no Redo, a new take is.
  const play = focus.locator('button[aria-label="Play"]').first();
  if (await play.count()) await play.click();
  const video = focus.locator('video').first();
  await video.waitFor({ state: 'attached' });
  await until('it plays', () => video.evaluate((element) => element.readyState >= 2), Boolean);
  const marked = () => segments.locator('.segment.here').innerText().catch(() => '');
  const redo = segments.locator('button.redo-segment');
  const put = async (seconds) => video.evaluate((element, seconds) => { element.pause(); element.currentTime = seconds; }, seconds);
  await put(SEAM / 3 / fps);
  seen.in_first = await until('segment 1 marked', marked, (text) => text === '1').catch(marked);
  check('in_segment_1_no_redo', seen.in_first === '1' && (await redo.count()) === 0, { marked: seen.in_first, redo: await redo.count() });
  await put(2);
  seen.at_2 = await until('Redo segment 2', () => redo.innerText().catch(() => ''), (text) => text === 'Redo segment 2').catch(() => redo.innerText().catch(() => ''));
  check('in_the_last_redo_it', seen.at_2 === 'Redo segment 2' && (await marked()) === '2', { redo: seen.at_2, marked: await marked() });
  // Chosen by its block: the player goes to its start.
  await blocks.nth(0).click();
  await until('segment 1 chosen', marked, (text) => text === '1').catch(marked);
  await blocks.nth(1).click();
  seen.chosen = await until('Redo segment 2', () => redo.innerText().catch(() => ''), (text) => text === 'Redo segment 2').catch(() => redo.innerText().catch(() => ''));
  await video.evaluate((element) => element.pause());
  check('a_block_chooses_its_segment', seen.chosen === 'Redo segment 2' && (await marked()) === '2', { redo: seen.chosen, marked: await marked() });
  const player = await focus.locator('.focus-image-transform > .media').first().boundingBox();
  const row2 = await segments.boundingBox();
  const button = await redo.boundingBox();
  seen.placed = { player, row: row2, button };
  check('right_under_the_player', Math.abs(row2.y - (player.y + player.height)) <= 2 && Math.abs(button.x + button.width - (player.x + player.width)) <= 2, seen.placed);
  check('redo_is_a_readable_button', button.height >= 28, button);
  const sizes = await segments.evaluate((row) => ({ block: getComputedStyle(row.querySelector('.segment')).fontSize, body: getComputedStyle(document.querySelector('.focus-card') ?? document.body).fontSize, height: row.querySelector('.segment').getBoundingClientRect().height }));
  seen.sizes = sizes;
  check('its_blocks_are_readable', parseFloat(sizes.block) >= parseFloat(sizes.body) && sizes.height >= 28, sizes);
  await looks('segments');

  // Redo: the draft says what's kept and what's made again, and nothing renders until Render, which keeps segment 1: out_frame 6.
  const asked = () => page.waitForRequest((request) => request.method() === 'POST' && /\/v1\/projects\/[^/]+\/runs$/.test(new URL(request.url()).pathname), { timeout: STEP_MS });
  await page.route('**/runs', (route) => (route.request().method() === 'POST' ? route.fulfill({ status: 409, contentType: 'application/json', body: JSON.stringify({ error: { kind: 'conflict', code: 'e2e_held', message: 'held by the cell' } }) }) : route.continue()));
  await redo.click();
  const leadIn = focus.locator('.lead-in');
  await leadIn.waitFor();
  const words = () => leadIn.locator('.lead-in-words b').innerText().catch(() => '');
  seen.lead_in = await until('what it keeps', words, (text) => text === 'Keeps segment 1 of <shot.1> · redoes 2').catch(words);
  check('the_draft_says_what_it_keeps_and_redoes', seen.lead_in === 'Keeps segment 1 of <shot.1> · redoes 2', seen.lead_in);
  check('redo_starts_no_render', (await row('shot')).versions === 1 && !(await row('shot')).making?.length, await row('shot'));
  await looks('redo-draft');
  const render = async () => {
    const request = asked();
    await focus.locator('button[aria-label=Render]').first().click();
    const anyway = page.getByRole('button', { name: 'Run here anyway' });
    await Promise.race([request, anyway.first().waitFor({ timeout: STEP_MS }).then(() => anyway.first().click())]).catch(() => {});
    return JSON.parse((await request).postData() ?? '{}');
  };
  const redone = await render();
  seen.redone = redone;
  check('redo_keeps_the_segments_before_it', redone.pickup?.take === '<shot.1>' && redone.pickup?.out_frame === SEAM, redone.pickup);
  // Cancel: a plain draft, which keeps nothing.
  await leadIn.locator('button:text-is("Cancel")').click();
  await leadIn.waitFor({ state: 'detached' });
  const plain = await render();
  seen.plain = plain;
  check('cancel_makes_a_plain_draft', plain.names?.includes('shot') && !plain.pickup, plain);
  await page.unroute('**/runs');

  // Redone for real: segment 1 stays, so the new take's seams start at its seam, and nothing was moved.
  await lookAtTake(1);
  await blocks.nth(1).click();
  await until('Redo segment 2', () => redo.innerText().catch(() => ''), (text) => text === 'Redo segment 2');
  await video.evaluate((element) => element.pause()).catch(() => {});
  await redo.click();
  await leadIn.waitFor();
  await renderHere();
  // The take it makes records the pickup; a render the cell held earlier may land beside it, plain.
  const picked = await until('the redone take', async () => (await versionsOf('shot')).find((one) => one.pickup), Boolean, FIRST_RUN_MS);
  const redid = await resultOf('shot', picked.number);
  seen.redid = { versions: (await versionsOf('shot')).map((one) => ({ number: one.number, pickup: one.pickup })), pickup: redid.version?.pickup, seams: redid.result?.seams, warnings: redid.result?.warnings };
  // From the take looked at: the plain render the cell held earlier can land first and be the one shown.
  check('a_redo_keeps_the_seam_it_starts_at', /^<shot\.\d+>$/.test(redid.version?.pickup?.take ?? '') && redid.version?.pickup?.out_frame === SEAM && redid.result?.seams?.[0] === SEAM && !redid.result?.warnings?.length, seen.redid);
  await closeBox(page);

  // Asked off a seam (only `cozy run` asks a frame of its own): the tool starts at the seam before it, says so, and keeps that seam.
  const off = execFileSync(process.env.COZY, ['--json', 'run', `${SHOTS}/shoot`, 'summary=On <look>.', 'pickup.take=<shot.1>', 'pickup.out_frame=50', '--on', 'local'], { encoding: 'utf8', cwd: folder, stdio: ['ignore', 'pipe', 'inherit'], timeout: FIRST_RUN_MS });
  const offRun = off.trim().split('\n').map((line) => { try { return JSON.parse(line); } catch { return null; } }).filter(Boolean).map((event) => event.run?.number).filter(Boolean).at(-1);
  const offResult = (await api(`${at}/runs/${offRun}`)).body?.result;
  seen.off_seam = { run: offRun, seams: offResult?.seams, warnings: offResult?.warnings };
  check('off_a_seam_it_starts_at_the_seam_before', offResult?.seams?.[0] === SEAM && (offResult?.warnings ?? []).some((said) => said.includes(`keeps up to frame ${SEAM}, not 50`)), seen.off_seam);

  // Another box continues the take from its end, a seam too: chosen in its draft, said in its frame, and sent; its take says where from.
  await openBox(page, 'next');
  const from = focus.locator('.continue-from select');
  await from.waitFor();
  await looks('continue-choice');
  await from.selectOption({ label: '<shot.1>' });
  await leadIn.waitFor();
  seen.continues = await until('what it continues', words, (text) => text.startsWith('Continues')).catch(words);
  check('it_says_it_continues_that_video', seen.continues === `Continues <shot.1> from its end · ${clock(last / fps)}`, seen.continues);
  await looks('continue-draft');
  const continued = await render();
  seen.continued = continued;
  check('it_continues_from_the_end', continued.pickup?.take === '<shot.1>' && continued.pickup?.out_frame === last && continued.pickup?.beats_from === 0, continued.pickup);
  await until('its take', async () => (await row('next'))?.versions, (n) => n >= 1, FIRST_RUN_MS);
  await until('its length measured', () => row('next'), (held) => held?.prefixes?.[0]?.main?.duration_ms > 0, FIRST_RUN_MS);
  const recorded = (await versionsOf('next'))[0]?.pickup;
  check('its_take_records_its_source', recorded?.take === '<shot.1>' && recorded?.out_frame === last, recorded);
  await focus.locator('.take[aria-label="take 1"]').click().catch(() => {});
  const says = focus.locator('.focus-under .continues');
  seen.source_said = await until('its source said', () => says.innerText().catch(() => ''), (text) => text.startsWith('Continues')).catch(() => says.innerText().catch(() => ''));
  check('focus_shows_its_source', seen.source_said === `Continues <shot.1> from ${clock(last / fps)}`, seen.source_said);
  await looks('continued-focus');
  await closeBox(page);
  const tile = boxOf(page, 'next').locator('.continues-rest');
  seen.tile_said = await tile.innerText().catch(() => '');
  check('its_tile_shows_its_source', seen.tile_said === 'from shot', seen.tile_said);
  await looks('continued-tile');

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
