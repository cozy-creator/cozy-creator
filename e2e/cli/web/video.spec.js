// video (cl-973 core cell 19): videos at rest and in Focus, and one rendered in segments (project/segments.py, H3's stand-in). At rest each
// shows its poster, not black for a clip that opens dark, then a muted looping preview. In Focus a video plays on the transport, its sound
// on; a click pauses its sharp current frame and another resumes, with no corner Stop. Two segments rendered: segment 1 plays on its take's tile while segment 2 renders, the run has reached its
// frames and its beat is done on the box (cl-885); played in Focus as the
// take lands, the joined take, one video with its sound, takes its place within 2 px and plays on. Its segments are blocks under the
// player and a block seeks; Redo says what it keeps and asks for segment 2 alone.
const { readFileSync } = require('fs');
const { join } = require('path');
const { test, expect, box, focus, fixtureTool, DARK_OPEN, canvasCard, canvasVisible, hoverBox, screenshotPixels } = require('./fixtures');
const { ffmpeg, fit, leave, rest, posterSpread } = require('./hands');

const SEGMENTS = fixtureTool('segments.py/segments');
const FPS = 24;
// Each segment's seconds, and how long segment 2 takes after segment 1 lands.
const SECONDS = 8;
const WAIT = 8;
const SEAM = SECONDS * FPS;
const RUN_MS = 120_000;

/** Ten seconds of a test pattern under a tone, as a person's clip is. */
function clip(path) {
  ffmpeg('ffmpeg')('-v', 'error', '-y', '-f', 'lavfi', '-i', `testsrc2=size=320x180:rate=${FPS}:duration=10`, '-f', 'lavfi', '-i', 'sine=frequency=330:duration=10',
    '-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-c:a', 'aac', '-shortest', path);
  return readFileSync(path);
}

// A hybrid desktop/touch context exercises real mouse clicks and taps on the same focused player.
test.use({ hasTouch: true });

const playing = (video) => video.evaluate((el) => ({ at: el.currentTime, paused: el.paused, muted: el.muted, volume: el.volume, loop: el.loop, controls: el.controls, ready: el.readyState, duration: el.duration }));

test('video', async ({ page, project, api, guards }) => {
  // Held on purpose: the Redo's render is asked, and answered here.
  guards.allow(/status of 409\b.*\/runs$/);
  await project.import(clip(test.info().outputPath('walk.mp4')), { name: 'walk', at: { x: 0, y: 0 } });
  await project.import(DARK_OPEN, { name: 'dark', at: { x: 360, y: 0 } });
  await project.add({ name: 'shot', tool: SEGMENTS, kind: 'video', fields: { summary: 'Along the coast, segment after segment.', beats: [{ description: 'The cliffs.', duration: SECONDS }, { description: 'The harbour.', duration: SECONDS }], wait: WAIT }, at: { x: 0, y: 320 } });
  await expect.poll(async () => (await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}`)).tools.some((tool) => tool.local && tool.id === SEGMENTS), { message: 'the project lists its local tool' }).toBe(true);
  for (const name of ['walk', 'dark', 'shot']) await expect.poll(() => canvasVisible(page, name)).toBe(true);
  await fit(page);
  const region = page.getByRole('region', { name: 'Focused asset' });

  await test.step('at rest, a poster, not black for one that opens dark, then a muted looping preview', async () => {
    for (const name of ['walk', 'dark']) {
      await page.mouse.move(4, 600);
      await expect.poll(async () => (await canvasCard(page, name))?.rendered?.media.ready ?? false, { message: `${name}'s poster`, timeout: 60_000 }).toBe(true);
      expect.soft(await posterSpread(page, name), `${name}'s poster shows something, not a black frame`).toBeGreaterThanOrEqual(12);
      await hoverBox(page, name);
      await rest(page, box(page, name).locator('.media'));
      const preview = box(page, name).locator('video.preview.on');
      await expect.poll(async () => (await preview.count()) && (await playing(preview)).at > 0.3, { message: `${name}'s preview plays`, timeout: 45_000 }).toBe(true);
      expect.soft(await playing(preview), `${name}'s preview: muted and looping`).toMatchObject({ muted: true, loop: true });
      await page.mouse.move(4, 600);
      await expect(page.locator('video.preview.on')).toHaveCount(0);
    }
  });

  await test.step('in Focus video clicks pause the sharp current frame and resume, with no corner Stop', async () => {
    await focus(page, 'walk');
    await region.locator('.tl-transport').getByRole('button', { name: 'Play', exact: true }).click();
    const live = region.locator('video.live');
    await expect.poll(async () => (await live.count()) && (await playing(live)).at > 0.2, { message: 'it plays' }).toBe(true);
    expect.soft(await playing(live), 'its sound on, the transport its control').toMatchObject({ paused: false, muted: false, controls: false });
    expect.soft((await playing(live)).volume, 'its volume').toBeGreaterThan(0);
    await expect.soft(region.locator('.tl-transport').getByRole('button', { name: 'Pause', exact: true }), 'the transport says Pause').toBeVisible();
    await expect.soft(region.getByRole('button', { name: 'Stop', exact: true }), 'no corner Stop on the video').toHaveCount(0);
    const held = await live.elementHandle();
    const point = await live.boundingBox();
    await page.mouse.click(point.x + point.width / 2, point.y + point.height / 2);
    await expect.poll(async () => (await playing(live)).paused, { message: 'clicking the video pauses once' }).toBe(true);
    await expect(live, 'the decoded video remains visible while paused').toBeVisible();
    const paused = await playing(live);
    expect.soft(await held.evaluate(el => ({ connected: el.isConnected, originalSize: [el.videoWidth, el.videoHeight], decoded: el.readyState >= 2 && Boolean(el.currentSrc), filter: getComputedStyle(el).filter })),
      'pause keeps the original decoded player, not a thumbnail replacement').toEqual({ connected: true, originalSize: [320, 180], decoded: true, filter: 'none' });
    await page.evaluate(() => new Promise(done => requestAnimationFrame(() => requestAnimationFrame(done))));
    const first = screenshotPixels(await live.screenshot({ path: test.info().outputPath('video-paused-first.png'), scale: 'css' }));
    // A declared 12-frame observation window: no playing clock, lower-resolution swap or changing pixels after pause.
    await page.evaluate(() => new Promise(done => { let left = 12; const tick = () => --left ? requestAnimationFrame(tick) : done(); requestAnimationFrame(tick); }));
    expect.soft((await playing(live)).at, 'the paused playhead holds its frame').toBeCloseTo(paused.at, 3);
    const still = screenshotPixels(await live.screenshot({ path: test.info().outputPath('video-paused-held.png'), scale: 'css' }));
    expect.soft([still.width, still.height], 'pause keeps the same displayed size').toEqual([first.width, first.height]);
    let changed = 0;
    for (let y = 2; y < Math.min(first.height, still.height) - 2; y += 4) for (let x = 2; x < Math.min(first.width, still.width) - 2; x += 4) {
      if (first(x, y).some((value, channel) => value !== still(x, y)[channel])) changed++;
    }
    expect.soft(changed, 'the paused decoded frame keeps identical screenshot pixels').toBe(0);
    await page.touchscreen.tap(point.x + point.width / 2, point.y + point.height / 2);
    await expect.poll(async () => { const now = await playing(live); return !now.paused && now.at > paused.at + 0.1; }, { message: 'tapping the paused video resumes once' }).toBe(true);
    // The bottom transport still controls pause/seek/play, and natural end keeps the final decoded frame too.
    await region.locator('.tl-transport').getByRole('button', { name: 'Pause', exact: true }).click();
    await expect.poll(async () => (await playing(live)).paused).toBe(true);
    await region.getByRole('slider', { name: 'Scrub', exact: true }).focus();
    await page.keyboard.press('End');
    await expect.poll(async () => (await playing(live)).at, { message: 'the native scrubber seeks near the end while paused' }).toBeGreaterThan(9.8);
    await region.locator('.tl-transport').getByRole('button', { name: 'Play', exact: true }).click();
    await expect.poll(() => live.evaluate(el => el.ended && el.paused && el.readyState >= 2), { message: 'natural end retains its decoded final frame' }).toBe(true);
    await expect(live).toBeVisible();
    await leave(page);
    expect.soft(await held.evaluate(el => !el.isConnected && !el.getAttribute('src') && el.paused), 'leaving Focus still releases the video source').toBe(true);
    await held.dispose();
  });

  let run;
  const where = {};
  await test.step('two segments rendered: segment 1 plays on its tile while segment 2 renders', async () => {
    await focus(page, 'shot');
    await region.getByRole('button', { name: 'Render', exact: true }).click();
    await expect.poll(async () => (run = (await project.asset('shot'))?.making?.[0]), { message: 'its run', timeout: RUN_MS }).toBeTruthy();
    const glimpse = region.locator('.here-side .take.making video.take-glimpse');
    await expect.poll(async () => {
      const stage = (await api.get(project.at(`/runs/${run}`))).progress?.stage ?? '';
      const tile = (await glimpse.count()) ? await playing(glimpse) : null;
      return Boolean(tile && tile.ready >= 2 && tile.at > 0 && /Segment 2 of 2/.test(stage));
    }, { message: "segment 1 playing on its take's tile while segment 2 renders", timeout: RUN_MS }).toBe(true);
    expect.soft(await playing(glimpse), 'silent on its tile').toMatchObject({ muted: true });
    // As segment 1 lands, the run has reached its frames, and on the box its beat is done (cl-885: H3's joined segments read as none).
    const reached = async () => (await api.get(project.at(`/runs/${run}?detail=full`))).video_progress ?? null;
    await expect.configure({ soft: true }).poll(async () => {
      const now = await reached();
      return { frames: now?.completed_frames, segments: now?.segments?.map((one) => one.status).join() };
    }, { message: 'segment 1 reached as it lands' }).toEqual({ frames: SEAM, segments: 'completed,in_progress' });
    await expect.configure({ soft: true }).poll(() => region.locator('.beat-progress').evaluateAll((bars) => bars.map((bar) => Number(bar.getAttribute('aria-valuenow')))), { message: 'beat 1 done on the box as segment 1 lands' }).toEqual([100, 0]);
  });

  await test.step('played in Focus as it lands: the take takes its place within 2 px and plays on', async () => {
    await region.locator('.here-side .take.making').first().click();
    const glimpse = region.locator('.glimpse video');
    await expect(region.locator('.glimpse')).toHaveAttribute('data-label', 'Segment 1 of 2');
    // Played with a few seconds of segment 1 left as segment 2 ends: still playing when the take lands.
    await expect.poll(async () => {
      const said = (await api.get(project.at(`/runs/${run}`))).progress;
      return /Segment 2 of 2/.test(said?.stage ?? '') && (said?.completed ?? 0) >= 6;
    }, { message: 'segment 2 nearly done', timeout: RUN_MS }).toBe(true);
    await region.locator('.tl-transport').getByRole('button', { name: 'Play', exact: true }).click();
    await expect.poll(async () => (await playing(glimpse)).paused, { message: 'segment 1 plays' }).toBe(false);
    expect.soft(await playing(glimpse), 'with its sound, on the transport').toMatchObject({ muted: false, controls: false });
    Object.assign(where, await region.locator('.focus-image-transform > .media').first().boundingBox());
    await expect.poll(async () => (await project.asset('shot'))?.prefixes?.length ?? 0, { message: 'its take', timeout: RUN_MS }).toBe(1);
    const live = region.locator('video.live');
    await expect.poll(async () => {
      if (!(await live.count())) return 'no player yet';
      const now = await playing(live);
      return !now.paused && Number.isFinite(now.duration) ? 'playing' : JSON.stringify(now);
    }, { message: 'the take plays on' }).toBe('playing');
    const landed = await playing(live);
    expect.soft(landed.at, 'on from where segment 1 was, not its start, s').toBeGreaterThanOrEqual(1);
    expect.soft(landed.duration, 'one video of both segments, s').toBeCloseTo(2 * SECONDS, 0);
    await expect.soft(region.locator('.glimpse'), 'nothing of the glimpse once it lands').toHaveCount(0);
    const now = await region.locator('.focus-image-transform > .media').first().boundingBox();
    expect.soft(['x', 'y', 'width', 'height'].filter((side) => Math.abs(now[side] - where[side]) > 2), `in its place: ${JSON.stringify({ where, now })}`).toEqual([]);
    const main = (await project.asset('shot')).prefixes[0].main;
    const streams = ffmpeg('ffprobe')('-v', 'error', '-show_entries', 'stream=codec_type', '-of', 'csv=p=0', join(project.path, main.path)).trim().split('\n');
    expect.soft(streams.sort(), 'its file: a picture and a sound').toEqual(['audio', 'video']);
  });

  await test.step('its segments are blocks under the player; a block seeks; Redo keeps segment 1 and asks for segment 2', async () => {
    const live = region.locator('video.live');
    await region.locator('.tl-transport').getByRole('button', { name: 'Pause', exact: true }).click();
    const track = region.locator('.tl-take');
    const blocks = track.locator('.segment');
    await expect(blocks).toHaveText(['1', '2']);
    const [player, transport, under] = [await region.locator('.focus-image-transform > .media').first().boundingBox(), await region.locator('.focus-under .tl-transport').boundingBox(), await track.boundingBox()];
    expect.soft(Math.abs(transport.y - (player.y + player.height)), 'the transport right under the player, px').toBeLessThanOrEqual(4);
    expect.soft(under.y, 'the blocks under the transport').toBeGreaterThanOrEqual(transport.y + transport.height - 2);
    const redo = track.getByRole('button', { name: 'Redo segment 2', exact: true });
    await blocks.nth(1).click();
    await expect.poll(async () => (await playing(live)).at, { message: 'block 2 seeks to its start' }).toBeCloseTo(SEAM / FPS, 1);
    await expect(redo).toBeVisible();
    await blocks.nth(0).click();
    await expect.poll(async () => (await playing(live)).at, { message: 'block 1 seeks to its start' }).toBeLessThanOrEqual(1 / FPS);
    await expect.soft(redo, 'segment 1 is a new take, not a redo').toHaveCount(0);
    await blocks.nth(1).click();
    const asked = [];
    await page.route((url) => /\/v1\/projects\/[^/]+\/runs$/.test(url.pathname), (route) => {
      if (route.request().method() !== 'POST') return route.fallback();
      asked.push(route.request().postDataJSON());
      return route.fulfill({ status: 409, json: { error: { kind: 'conflict', code: 'e2e_held', message: 'held by the cell' } } });
    });
    await redo.click();
    await expect(region.locator('.lead-in .lead-in-words b')).toHaveText('Keeps segment 1 of <shot.1> · redoes 2');
    expect.soft(asked, 'Redo renders nothing until Render').toEqual([]);
    await region.getByRole('button', { name: 'Render', exact: true }).click();
    await expect.poll(() => asked.length, { message: 'its render asked' }).toBe(1);
    expect.soft(asked[0].pickup, 'segment 1 kept, segment 2 asked for').toMatchObject({ take: '<shot.1>', out_frame: SEAM });
    expect.soft((await project.asset('shot')).versions, 'no take made').toBe(1);
  });
});
