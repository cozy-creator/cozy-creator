// focus-no-poster (cl-1214). Paul, 11-18: "please stop on the webui, when you are in focus mode on a video, showing the image frame in the
// middle with the border. Don't do that. Just display the video; delete this little 'fallback thumbnail' or whatever it is and do not show
// it again when focused on a video node" (and 11-17: "display the full-resolution video / frames always"). A video in Focus is its own
// frames at their own size: no picture is ever in its frame, from the moment it opens (watched every frame), playing, paused, scrubbed,
// after another tab takes its player, a take still being made playing its segments, and an orbit. Pictures of each state at 1440×900,
// paper and dark: $OUT/web/focus-no-poster/.
const { mkdirSync, readFileSync } = require('fs');
const { join } = require('path');
const { test, expect, focus, canvasVisible, fixtureTool, themed } = require('./fixtures');
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

const clock = (video) => video.evaluate((el) => ({ at: el.currentTime, paused: el.paused, ready: el.readyState }));

test('focus-no-poster', async ({ page, project, cli, context, signIn }) => {
  test.setTimeout(420_000);
  await cli.json('machine', 'use', 'local');
  const shots = join(process.env.OUT, 'web', 'focus-no-poster');
  mkdirSync(shots, { recursive: true });
  const clip = test.info().outputPath('walk.mp4');
  ffmpeg('ffmpeg')('-v', 'error', '-y', '-f', 'lavfi', '-i', 'testsrc2=size=1280x720:rate=24:duration=10', '-c:v', 'libx264', '-preset', 'veryfast', '-pix_fmt', 'yuv420p', clip);
  await project.import(readFileSync(clip), { name: 'walk', at: { x: 0, y: 0 } });
  await project.add({ name: 'turned', tool: TURN, kind: 'video', fields: { frames: 48 }, at: { x: 520, y: 0 } });
  await project.add({ name: 'shot', tool: SEGMENTS, kind: 'video', fields: { summary: 'Along the coast.', beats: [{ description: 'The cliffs.', duration: 4 }, { description: 'The harbour.', duration: 4 }], wait: 12 }, at: { x: 0, y: 400 } });
  const rendered = await cli('--json', 'render', 'turned', '--on', 'local');
  expect(rendered.code, `cozy render: ${rendered.stderr.slice(-400)}`).toBe(0);
  await expect.poll(async () => Boolean((await project.asset('turned'))?.prefixes?.at(-1)?.main?.metadata?.orbit), { message: "turned's orbit take", timeout: RUN_MS }).toBe(true);
  await page.setViewportSize({ width: 1440, height: 900 });
  for (const name of ['walk', 'turned', 'shot']) await expect.poll(() => canvasVisible(page, name), { timeout: 60_000 }).toBe(true);
  await fit(page);
  const region = page.getByRole('region', { name: 'Focused asset' });
  const live = region.locator('video.live');
  const transport = (name) => region.locator('.tl-transport').getByRole('button', { name, exact: true });
  const none = async (watch, state) => {
    expect.soft(await watch.seen(), `${state}: no picture in the video's frame, ever`).toEqual([]);
    await page.screenshot({ path: join(shots, `${state.replace(/\W+/g, '-')}.png`) });
  };

  await test.step('a video in Focus: just opened, playing, paused and scrubbed, its own frames and no picture', async () => {
    const watch = await watchFrame(page);
    await focus(page, 'walk');
    await expect.poll(async () => (await live.count()) ? (await clock(live)).ready : 0, { message: 'its first frame decoded', timeout: 30_000 }).toBeGreaterThanOrEqual(2);
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

  await test.step("another tab's play takes its player: still no picture in its frame", async () => {
    const watch = await watchFrame(page);
    const others = [];
    for (const name of ['turned', 'walk']) {
      const tab = await context.newPage();
      await signIn(tab, project.url());
      await expect(tab.locator('.stage')).toBeVisible();
      await expect.poll(() => canvasVisible(tab, name), { timeout: 60_000 }).toBe(true);
      await fit(tab);
      await focus(tab, name);
      if (name === 'walk') await tab.getByRole('region', { name: 'Focused asset' }).locator('.tl-transport').getByRole('button', { name: 'Play', exact: true }).click();
      others.push(tab);
    }
    await page.bringToFront();
    await expect(live, 'its player let go').toHaveCount(0);
    await none(watch, 'after its player was taken');
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

  await test.step('a take being made, its segments playing in Focus: no picture in its frame', async () => {
    await leave(page);
    await focus(page, 'shot');
    await region.getByRole('button', { name: 'Render', exact: true }).click();
    await expect.poll(async () => (await project.asset('shot'))?.making?.[0], { message: 'its run', timeout: RUN_MS }).toBeTruthy();
    await expect.poll(async () => region.locator('.here-side .take.making video.take-glimpse').count(), { message: 'segment 1 on its tile', timeout: RUN_MS }).toBeGreaterThan(0);
    await region.locator('.here-side .take.making').first().click();
    const glimpse = region.locator('.glimpse video');
    await expect(glimpse).toHaveCount(1);
    const watch = await watchFrame(page);
    await transport('Play').click();
    await expect.poll(async () => (await clock(glimpse)).at, { message: 'segment 1 plays in Focus' }).toBeGreaterThan(0.5);
    await none(watch, 'a take being made');
    await watch.stop();
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
      await focus(page, name);
      if (name === 'walk') await expect.poll(async () => (await live.count()) ? (await clock(live)).ready : 0, { timeout: 30_000 }).toBeGreaterThanOrEqual(2);
      else await expect(region.locator('.orbit.ready')).toBeVisible({ timeout: 30_000 });
      await none(watch, `${name} dark`);
      await watch.stop();
      await leave(page);
    }
    await project.look('paper');
  });
});
