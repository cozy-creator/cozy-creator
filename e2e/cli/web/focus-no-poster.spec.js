// focus-no-poster (cl-1214). Paul, 11-18: "please stop on the webui, when you are in focus mode on a video, showing the image frame in the
// middle with the border. Don't do that. Just display the video; delete this little 'fallback thumbnail' or whatever it is and do not show
// it again when focused on a video node" (and 11-17: "display the full-resolution video / frames always"). A video in Focus is its own
// frames at their own size: no picture is ever in its frame, from the moment it opens (watched every frame), playing, paused, scrubbed,
// while another tab plays it too (cl-1215: it keeps playing), a take still being made playing its segments, and an orbit. Pictures of
// each state at 1440×900, paper and dark: $OUT/web/focus-no-poster/.
// Never black either (cl-1221; the same brief: "until the first frame decodes the frame shows nothing, never a thumbnail or black"): every
// frame Chrome puts on the screen (its screencast) is read from before a video opens until its first picture is up: opened from the
// canvas, a take landing with nothing of it shown before, a take landing while its first segment plays (it plays on from there), and in
// dark. The player's box was black from its `loadeddata` until its first picture reached the screen, for as long as a seek to where the
// segment was playing took (e2e 44243147622's trace: 55 to 80 ms).
const { mkdirSync, readFileSync } = require('fs');
const { join } = require('path');
const { test, expect, focus, canvasVisible, fixtureTool, screenshotPixels, themed } = require('./fixtures');
const { ffmpeg, fit, leave } = require('./hands');

const TURN = fixtureTool('orbit.py/turn');
const SEGMENTS = fixtureTool('segments.py/segments');
const RUN_MS = 120_000;

/** From now, every picture in Focus's frame, looked for at every change and every frame: `seen()` returns what was ever there. */
const watchFrame = (page) => page.evaluate(() => {
  const seen = new Set();
  const look = () => {
    for (const img of document.querySelectorAll('.focus-image-transform img')) seen.add(`${img.className || 'img'} ${(img.currentSrc || img.src).split('/').pop()?.slice(0, 48)}`);
  };
  const observer = new MutationObserver(look);
  observer.observe(document, { subtree: true, childList: true, attributes: true, attributeFilter: ['src', 'class'] });
  let raf = requestAnimationFrame(function frame() { look(); raf = requestAnimationFrame(frame); });
  window.__frameSeen = () => [...seen];
  window.__frameStop = () => { observer.disconnect(); cancelAnimationFrame(raf); return [...seen]; };
}).then(() => ({ seen: () => page.evaluate(() => window.__frameSeen()), stop: () => page.evaluate(() => window.__frameStop()) }));

/** Every frame Chrome puts on the screen from now, as its screencast gives them (quarter size, so each is taken quickly): `stop()` returns
 *  them in order, each `{ png, ms }`, `ms` from the first. */
async function watchScreen(page) {
  const cdp = await page.context().newCDPSession(page);
  const frames = [];
  cdp.on('Page.screencastFrame', ({ data, metadata, sessionId }) => {
    cdp.send('Page.screencastFrameAck', { sessionId }).catch(() => {});
    if (frames.length < 6000) frames.push({ png: Buffer.from(data, 'base64'), at: metadata.timestamp ?? 0 });
  });
  const { width, height } = page.viewportSize();
  await cdp.send('Page.startScreencast', { format: 'png', everyNthFrame: 1, maxWidth: Math.round(width / 4), maxHeight: Math.round(height / 4) });
  return {
    stop: async () => {
      await cdp.send('Page.stopScreencast').catch(() => {});
      await cdp.detach().catch(() => {});
      return frames.map(({ png, at }) => ({ png, ms: Math.round((at - frames[0].at) * 1000) }));
    },
  };
}

/** Of `frames`, those black over `box` (a video's place on the page): more than half of a 12 × 8 grid across its middle. */
function blackOver(frames, box, viewport) {
  const black = [];
  for (const [n, { png, ms }] of frames.entries()) {
    const at = screenshotPixels(png), scale = at.width / viewport.width;
    let dark = 0;
    for (let row = 0; row < 8; row++) {
      for (let column = 0; column < 12; column++) {
        const [x, y] = [(box.x + box.width * (0.1 + 0.8 * column / 11)) * scale, (box.y + box.height * (0.1 + 0.8 * row / 7)) * scale];
        if (Math.max(...at(Math.min(x, at.width - 1), Math.min(y, at.height - 1))) <= 8) dark++;
      }
    }
    if (dark > 48) black.push(`frame ${n + 1} of ${frames.length}, at ${ms} ms: ${dark} of 96 points black`);
  }
  return black;
}

const clock = (video) => video.evaluate((el) => ({ at: el.currentTime, paused: el.paused, ready: el.readyState, seeking: el.seeking }));

test('focus-no-poster', async ({ page, project, cli, context, signIn }) => {
  test.setTimeout(420_000);
  await cli.json('machine', 'use', 'local');
  const shots = join(process.env.OUT, 'web', 'focus-no-poster');
  mkdirSync(shots, { recursive: true });
  const clip = test.info().outputPath('walk.mp4');
  ffmpeg('ffmpeg')('-v', 'error', '-y', '-f', 'lavfi', '-i', 'testsrc2=size=1280x720:rate=24:duration=10', '-c:v', 'libx264', '-preset', 'veryfast', '-pix_fmt', 'yuv420p', clip);
  await project.import(readFileSync(clip), { name: 'walk', at: { x: 0, y: 0 } });
  await project.add({ name: 'turned', tool: TURN, kind: 'video', fields: { frames: 48 }, at: { x: 520, y: 0 } });
  await project.add({ name: 'single', tool: SEGMENTS, kind: 'video', fields: { summary: 'The harbour.', beats: [{ description: 'The harbour.', duration: 4 }], wait: 0 }, at: { x: 520, y: 400 } });
  // Its first segment is long enough to be playing when the take lands.
  await project.add({ name: 'shot', tool: SEGMENTS, kind: 'video', fields: { summary: 'Along the coast.', beats: [{ description: 'The cliffs.', duration: 14 }, { description: 'The harbour.', duration: 4 }], wait: 10 }, at: { x: 0, y: 400 } });
  const rendered = await cli('--json', 'render', 'turned', '--on', 'local');
  expect(rendered.code, `cozy render: ${rendered.stderr.slice(-400)}`).toBe(0);
  await expect.poll(async () => Boolean((await project.asset('turned'))?.prefixes?.at(-1)?.main?.metadata?.orbit), { message: "turned's orbit take", timeout: RUN_MS }).toBe(true);
  await page.setViewportSize({ width: 1440, height: 900 });
  for (const name of ['walk', 'turned', 'shot', 'single']) await expect.poll(() => canvasVisible(page, name), { timeout: 60_000 }).toBe(true);
  await fit(page);
  const region = page.getByRole('region', { name: 'Focused asset' });
  const live = region.locator('video.live');
  const transport = (name) => region.locator('.tl-transport').getByRole('button', { name, exact: true });
  /** From `screen`'s start until `video`'s first picture is up, its place on the page was never black. */
  const neverBlack = async (screen, video, state) => {
    await expect.poll(async () => { const now = await clock(video); return now.ready >= 2 && !now.seeking; }, { message: `${state}: its picture decoded` }).toBe(true);
    await page.waitForTimeout(400);
    const frames = await screen.stop();
    const gap = frames.reduce((most, frame, n) => Math.max(most, n ? frame.ms - frames[n - 1].ms : 0), 0);
    console.log(`NOTE focus-no-poster ${state}: ${frames.length} frames read over ${frames.at(-1)?.ms ?? 0} ms, the longest gap ${gap} ms`);
    expect.soft(frames.length, `${state}: the screen was watched`).toBeGreaterThan(0);
    expect.soft(blackOver(frames, await video.boundingBox(), page.viewportSize()), `${state}: its frame never black before its first picture`).toEqual([]);
  };
  const none = async (watch, state) => {
    expect.soft(await watch.seen(), `${state}: no picture in the video's frame, ever`).toEqual([]);
    await page.screenshot({ path: join(shots, `${state.replace(/\W+/g, '-')}.png`) });
  };

  await test.step('a video in Focus: just opened, playing, paused and scrubbed, its own frames and no picture', async () => {
    const watch = await watchFrame(page);
    const screen = await watchScreen(page);
    await focus(page, 'walk');
    await expect.poll(async () => (await live.count()) ? (await clock(live)).ready : 0, { message: 'its first frame decoded', timeout: 30_000 }).toBeGreaterThanOrEqual(2);
    await neverBlack(screen, live, 'just opened');
    await none(watch, 'just opened');
    await transport('Play').click();
    await expect.poll(async () => (await clock(live)).at, { message: 'it plays' }).toBeGreaterThan(0.5);
    await none(watch, 'playing');
    await transport('Pause').click();
    await expect.poll(async () => (await clock(live)).paused).toBe(true);
    await none(watch, 'paused');
    await region.getByRole('slider', { name: 'Scrub', exact: true }).fill('120');
    await expect.poll(async () => (await clock(live)).at, { message: 'it scrubs' }).toBeGreaterThan(4);
    await none(watch, 'scrubbed');
    await watch.stop();
  });

  await test.step('another tab plays it too: this one keeps playing, still no picture in its frame', async () => {
    const watch = await watchFrame(page);
    const others = [];
    for (const name of ['turned', 'walk']) {
      const tab = await context.newPage();
      await signIn(tab, project.url());
      await expect(tab.locator('.stage')).toBeVisible();
      await expect.poll(() => canvasVisible(tab, name), { timeout: 60_000 }).toBe(true);
      await fit(tab);
      await focus(tab, name);
      others.push(tab);
    }
    await page.bringToFront();
    await region.getByRole('slider', { name: 'Scrub', exact: true }).fill('0');
    await transport('Play').click();
    await expect.poll(async () => (await clock(live)).paused, { message: 'it plays from its start' }).toBe(false);
    const theirs = others[1].getByRole('region', { name: 'Focused asset' });
    await others[1].bringToFront();
    await theirs.locator('.tl-transport').getByRole('button', { name: 'Play', exact: true }).click();
    await expect.poll(async () => (await theirs.locator('video.live').count()) ? (await clock(theirs.locator('video.live'))).at : 0, { message: 'the other tab plays it' }).toBeGreaterThan(0.5);
    await page.bringToFront();
    await expect(live, 'its player stays').toHaveCount(1);
    const was = await clock(live);
    await expect.poll(async () => (await clock(live)).at, { message: 'it keeps playing' }).toBeGreaterThan(was.at + 0.5);
    expect.soft((await clock(live)).paused, 'still playing here').toBe(false);
    expect.soft((await clock(theirs.locator('video.live'))).paused, 'still playing in the other tab').toBe(false);
    await none(watch, 'while another tab plays it');
    await watch.stop();
    for (const tab of others) await tab.close();
  });

  await test.step('an orbit in Focus: opened and turned, no picture in its frame', async () => {
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await leave(page);
    const watch = await watchFrame(page);
    await focus(page, 'turned');
    const orbit = region.getByRole('slider', { name: 'Turn', exact: true });
    await expect(region.locator('.orbit.ready')).toBeVisible({ timeout: 30_000 });
    await none(watch, 'an orbit opened');
    const box = await orbit.boundingBox(), y = box.y + box.height / 2;
    await page.mouse.move(box.x + box.width * 0.25, y);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width * 0.5, y, { steps: 8 });
    await page.mouse.up();
    await expect.poll(async () => Number(await orbit.getAttribute('data-angle'))).toBeCloseTo(90, -1);
    await none(watch, 'an orbit turned');
    await watch.stop();
    await page.emulateMedia({ reducedMotion: null });
  });

  await test.step('a take lands in Focus with nothing of it shown before: its frame never black', async () => {
    await leave(page);
    await focus(page, 'single');
    const screen = await watchScreen(page);
    await region.getByRole('button', { name: 'Render', exact: true }).click();
    await expect.poll(async () => (await live.count()) ? (await clock(live)).ready : 0, { message: 'its take landed, its first frame decoded', timeout: RUN_MS }).toBeGreaterThanOrEqual(2);
    await neverBlack(screen, live, 'a take just landed');
    await page.screenshot({ path: join(shots, 'a-take-just-landed.png') });
  });

  await test.step('a take being made, its segments playing in Focus: no picture in its frame, and never black as the take lands', async () => {
    await leave(page);
    await focus(page, 'shot');
    await region.getByRole('button', { name: 'Render', exact: true }).click();
    await expect.poll(async () => (await project.asset('shot'))?.making?.[0], { message: 'its run', timeout: RUN_MS }).toBeTruthy();
    await expect.poll(async () => region.locator('.here-side .take.making video.take-glimpse').count(), { message: 'segment 1 on its tile', timeout: RUN_MS }).toBeGreaterThan(0);
    await region.locator('.here-side .take.making').first().click();
    const glimpse = region.locator('.glimpse video');
    await expect(glimpse).toHaveCount(1);
    const watch = await watchFrame(page);
    const screen = await watchScreen(page);
    await transport('Play').click();
    await expect.poll(async () => (await clock(glimpse)).at, { message: 'segment 1 plays in Focus' }).toBeGreaterThan(0.5);
    await none(watch, 'a take being made');
    await watch.stop();
    await expect(live, 'the take lands while segment 1 plays').toHaveCount(1, { timeout: RUN_MS });
    await neverBlack(screen, live, 'a take landing as it plays');
    expect.soft((await clock(live)).at, 'the take plays on from where segment 1 was').toBeGreaterThan(1);
    await page.screenshot({ path: join(shots, 'a-take-landing-as-it-plays.png') });
  });

  await test.step('to look at: each state in dark', async () => {
    await leave(page);
    await project.look('dark');
    await page.reload();
    await expect(page.locator('.stage')).toBeVisible();
    await themed(page, 'dark');
    await fit(page);
    for (const name of ['walk', 'turned']) {
      const watch = await watchFrame(page);
      const screen = await watchScreen(page);
      await focus(page, name);
      if (name === 'walk') await expect.poll(async () => (await live.count()) ? (await clock(live)).ready : 0, { timeout: 30_000 }).toBeGreaterThanOrEqual(2);
      else await expect(region.locator('.orbit.ready')).toBeVisible({ timeout: 30_000 });
      await neverBlack(screen, name === 'walk' ? live : region.locator('video.orbit-video'), `${name} dark`);
      await none(watch, `${name} dark`);
      await watch.stop();
      await leave(page);
    }
    await project.look('paper');
  });
});
