// video (cl-973 core cell 19): videos at rest and in Focus, and one rendered in segments (project/segments.py, H3's stand-in). At rest each
// shows its poster, not black for a clip that opens dark, then a muted looping preview. In Focus a video plays on the transport, its sound
// on, and its stop is a square. Two segments rendered: segment 1 plays on its take's tile while segment 2 renders; played in Focus as the
// take lands, the joined take, one video with its sound, takes its place within 2 px and plays on. Its segments are blocks under the
// player and a block seeks; Redo says what it keeps and asks for segment 2 alone.
const { readFileSync } = require('fs');
const { join } = require('path');
const { test, expect, box, focus, fixtureTool, DARK_OPEN } = require('./fixtures');
const { ffmpeg, fit, leave, rest, spread } = require('./hands');

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

const playing = (video) => video.evaluate((el) => ({ at: el.currentTime, paused: el.paused, muted: el.muted, volume: el.volume, loop: el.loop, controls: el.controls, ready: el.readyState, duration: el.duration }));

test('video', async ({ page, project, api, guards }) => {
  // Held on purpose: the Redo's render is asked, and answered here.
  guards.allow(/status of 409\b.*\/runs$/);
  await project.import(clip(test.info().outputPath('walk.mp4')), { name: 'walk', at: { x: 0, y: 0 } });
  await project.import(DARK_OPEN, { name: 'dark', at: { x: 360, y: 0 } });
  await project.add({ name: 'shot', tool: SEGMENTS, kind: 'video', fields: { summary: 'Along the coast, segment after segment.', segments: 2, seconds: SECONDS, wait: WAIT }, at: { x: 0, y: 320 } });
  await expect.poll(async () => (await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}`)).tools.some((tool) => tool.local && tool.id === SEGMENTS), { message: 'the project lists its local tool' }).toBe(true);
  for (const name of ['walk', 'dark', 'shot']) await expect(box(page, name)).toBeVisible();
  await fit(page);
  const region = page.getByRole('region', { name: 'Focused asset' });

  await test.step('at rest, a poster, not black for one that opens dark, then a muted looping preview', async () => {
    for (const name of ['walk', 'dark']) {
      const poster = box(page, name).locator('.media .scrub img').first();
      await expect.poll(() => poster.evaluate((img) => img.complete && img.naturalWidth > 0), { message: `${name}'s poster`, timeout: 60_000 }).toBe(true);
      expect.soft(await spread(poster), `${name}'s poster shows something, not a black frame`).toBeGreaterThanOrEqual(12);
      await rest(page, box(page, name).locator('.media'));
      const preview = box(page, name).locator('video.preview.on');
      await expect.poll(async () => (await preview.count()) && (await playing(preview)).at > 0.3, { message: `${name}'s preview plays`, timeout: 45_000 }).toBe(true);
      expect.soft(await playing(preview), `${name}'s preview: muted and looping`).toMatchObject({ muted: true, loop: true });
      await page.mouse.move(4, 600);
      await expect(page.locator('video.preview.on')).toHaveCount(0);
    }
  });

  await test.step('in Focus it plays on the transport with its sound, and stops by a square', async () => {
    await focus(page, 'walk');
    await region.locator('.tl-transport').getByRole('button', { name: 'Play', exact: true }).click();
    const live = region.locator('video.live');
    await expect.poll(async () => (await live.count()) && (await playing(live)).at > 0.2, { message: 'it plays' }).toBe(true);
    expect.soft(await playing(live), 'its sound on, the transport its control').toMatchObject({ paused: false, muted: false, controls: false });
    expect.soft((await playing(live)).volume, 'its volume').toBeGreaterThan(0);
    await expect.soft(region.locator('.tl-transport').getByRole('button', { name: 'Pause', exact: true }), 'the transport says Pause').toBeVisible();
    const stop = region.getByRole('button', { name: 'Stop', exact: true });
    await expect.soft(stop, 'its stop').toHaveAttribute('title', 'Stop');
    await expect.soft(stop.locator('svg'), 'a square, not ✕').toHaveAttribute('data-icon', 'stop');
    await stop.click();
    await expect(live, 'stopped').toHaveCount(0);
    await leave(page);
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
