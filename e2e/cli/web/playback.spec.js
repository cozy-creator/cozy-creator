// playback (cl-1208). Paul, 11-17, on rc/0.27.0 with cl-1190 (#3814) and cl-1198 (#3831) in: "why is my kyogre_orbit node fucked up? the
// video is not playing when I'm inspecting it", "it's not spinning it's just displaying a single picture to me", "neither of them are playing
// now get them both to play, both the orbit videos should play". web-orbit and web-video stayed green: they read an orbit's angle and a
// player's clock, not what's on screen, and only on the page built into cozy, while his laptop serves the page from Vite (dev = true), React's
// development build. So this reads the screen, on both pages: on the canvas a video's preview moves; in Focus a video plays (its clock runs
// and its picture moves), an orbit's picture turns with a drag and moves with Spin, and both again after Focus goes from one to the other; and
// every video in every window plays at once (web-windows, cl-1215). And a video being made in segments (cl-1210) grows on screen in Focus and its
// take plays on, on both pages: the development build detaches every ref once as it mounts, which emptied a growing video there.
const { spawn } = require('child_process');
const { closeSync, existsSync, openSync, readFileSync, writeFileSync } = require('fs');
const { homedir } = require('os');
const { join, resolve } = require('path');
const { test, expect, focus, canvasCard, canvasVisible, hoverBox, fixtureTool, previewState, screenshotPixels } = require('./fixtures');
const { ffmpeg, fit, leave, rest } = require('./hands');

const TURN = fixtureTool('orbit.py/turn');
const SEGMENTS = fixtureTool('segments.py/segments');
const FRAMES = 48;

/** What's on screen over `box`'s middle (a 16 × 9 grid over its middle 70 % across and 60 % down, clear of an orbit's bar and hint). */
async function look(page, box) {
  const at = screenshotPixels(await page.screenshot({ scale: 'css' }));
  const seen = [];
  for (let row = 0; row < 9; row++) {
    for (let column = 0; column < 16; column++) seen.push(at(box.x + box.width * (0.15 + 0.7 * column / 15), box.y + box.height * (0.2 + 0.6 * row / 8)));
  }
  return seen;
}
/** How many of the grid's points changed between two looks. */
const moved = (a, b) => a.filter((rgb, i) => Math.max(...rgb.map((value, c) => Math.abs(value - b[i][c]))) > 40).length;
/** Moving: at least this many of 144 points changed. */
const MOVES = 6;

const clock = (video) => video.evaluate((el) => ({ at: el.currentTime, paused: el.paused, ready: el.readyState, seeking: el.seeking, src: Boolean(el.currentSrc) }));

async function made({ page, project, cli }) {
  await cli.json('machine', 'use', 'local');
  const clip = test.info().outputPath('walk.mp4');
  ffmpeg('ffmpeg')('-v', 'error', '-y', '-f', 'lavfi', '-i', 'testsrc2=size=320x180:rate=24:duration=10', '-c:v', 'libx264', '-preset', 'veryfast', '-pix_fmt', 'yuv420p', clip);
  await project.import(readFileSync(clip), { name: 'walk', at: { x: 520, y: 0 } });
  await project.add({ name: 'turned', tool: TURN, kind: 'video', fields: { frames: FRAMES }, at: { x: 0, y: 0 } });
  const rendered = await cli('--json', 'render', 'turned', '--on', 'local');
  expect(rendered.code, `cozy render: ${rendered.stderr.slice(-400)}`).toBe(0);
  await expect.poll(async () => Boolean((await project.asset('turned'))?.prefixes?.at(-1)?.main?.metadata?.orbit), { message: "turned's orbit take", timeout: 120_000 }).toBe(true);
  // Two segments of 8 s, the second 4 s after the first: its take lands while the first still plays.
  const beats = [{ description: 'The cliffs.', duration: 8 }, { description: 'The harbour.', duration: 8 }];
  await project.add({ name: 'grows', tool: SEGMENTS, kind: 'video', fields: { summary: 'Along the coast.', beats, wait: 4 }, at: { x: 0, y: 400 } });
}

/** Every check, on the page as it's served now. */
async function plays(page, project) {
  const region = page.getByRole('region', { name: 'Focused asset' });
  for (const name of ['turned', 'walk', 'grows']) await expect.poll(() => canvasVisible(page, name)).toBe(true);
  await fit(page);

  await test.step('on the canvas: a video rested on previews, and its picture moves', async () => {
    await page.mouse.move(4, 600);
    await expect.poll(async () => (await canvasCard(page, 'walk'))?.rendered?.media.ready ?? false, { message: "walk's poster", timeout: 60_000 }).toBe(true);
    await hoverBox(page, 'walk');
    const body = (await canvasCard(page, 'walk')).body;
    await rest(page, body);
    await expect.poll(async () => (await previewState(page, 'walk')).playing, { message: "walk's preview plays", timeout: 45_000 }).toBe(true);
    const was = await look(page, body);
    await expect.poll(async () => moved(was, await look(page, body)), { message: "walk's preview moves on screen" }).toBeGreaterThanOrEqual(MOVES);
    await page.mouse.move(4, 600);
  });

  for (const round of [1, 2]) {
    await test.step(`${round}: in Focus a video plays: its clock runs and its picture moves`, async () => {
      await focus(page, 'walk');
      await region.locator('.tl-transport').getByRole('button', { name: 'Play', exact: true }).click();
      const live = region.locator('video.live');
      await expect.poll(async () => (await live.count()) ? (await clock(live)).at : 0, { message: 'its clock runs', timeout: 30_000 }).toBeGreaterThan(0.5);
      const box = await live.boundingBox();
      const [was, then] = [await look(page, box), await clock(live)];
      await expect.poll(async () => moved(was, await look(page, box)), { message: 'its picture moves on screen' }).toBeGreaterThanOrEqual(MOVES);
      expect((await clock(live)).at, 'its clock went on').toBeGreaterThan(then.at);
      await region.locator('.tl-transport').getByRole('button', { name: 'Pause', exact: true }).click();
      await leave(page);
    });

    await test.step(`${round}: in Focus an orbit turns on screen with a drag, and moves with Spin`, async () => {
      await page.emulateMedia({ reducedMotion: 'reduce' });
      await focus(page, 'turned');
      const orbit = region.getByRole('slider', { name: 'Turn', exact: true });
      const video = region.locator('video.orbit-video');
      await expect(region.locator('.orbit.ready'), 'its video drawn').toBeVisible({ timeout: 30_000 });
      await expect.poll(async () => Number(await orbit.getAttribute('data-angle')), { message: 'at its front' }).toBe(0);
      const box = await orbit.boundingBox();
      const front = await look(page, box);
      const y = box.y + box.height / 2;
      await page.mouse.move(box.x + box.width * 0.25, y);
      await page.mouse.down();
      await page.mouse.move(box.x + box.width * 0.5, y, { steps: 8 });
      await page.mouse.up();
      await expect.poll(async () => Number(await orbit.getAttribute('data-angle')), { message: 'a quarter of its width turns it a quarter round' }).toBeCloseTo(90, -1);
      // Frame 12 of 48, its middle: the element seeked there and drew it.
      await expect.poll(async () => { const now = await clock(video); return !now.seeking && Math.abs(now.at - 12.5 / 24) < 0.03; }, { message: 'its video at frame 12' }).toBe(true);
      await expect.poll(async () => moved(front, await look(page, box)), { message: 'the drag turned its picture on screen' }).toBeGreaterThanOrEqual(MOVES);

      await region.locator('.orbit-bar').getByRole('button', { name: 'Spin', exact: true }).click();
      await expect.poll(async () => (await clock(video)).paused, { message: 'Spin plays it' }).toBe(false);
      const [was, then] = [await look(page, box), await clock(video)];
      await expect.poll(async () => moved(was, await look(page, box)), { message: 'spinning, its picture moves on screen' }).toBeGreaterThanOrEqual(MOVES);
      expect((await clock(video)).at, 'its clock went on').not.toBeCloseTo(then.at, 2);
      await region.locator('.orbit-bar').getByRole('button', { name: 'Stop', exact: true }).click();
      await page.emulateMedia({ reducedMotion: null });
      await leave(page);
    });
  }

  await test.step('in Focus a video being made grows on screen, and its take plays on from there', async () => {
    await focus(page, 'grows');
    const before = (await project.asset('grows'))?.prefixes?.length ?? 0;
    await region.getByRole('button', { name: 'Render', exact: true }).click();
    await region.locator('.here-side .take.making').first().click();
    const glimpse = region.locator('.glimpse video.shown');
    await expect.poll(() => glimpse.evaluate((el) => el.readyState >= 2).catch(() => false), { message: 'its first segment here', timeout: 120_000 }).toBe(true);
    await region.locator('.tl-transport').getByRole('button', { name: 'Play', exact: true }).click();
    await expect.poll(async () => (await clock(glimpse)).at, { message: "what's made so far plays" }).toBeGreaterThan(0.3);
    const grew = await look(page, await glimpse.boundingBox());
    await expect.poll(async () => moved(grew, await look(page, await glimpse.boundingBox())), { message: 'its picture moves on screen' }).toBeGreaterThanOrEqual(MOVES);
    await expect.poll(async () => (await project.asset('grows'))?.prefixes?.length ?? 0, { message: 'its take', timeout: 120_000 }).toBe(before + 1);
    const live = region.locator('video.live');
    await expect.poll(async () => {
      if (!(await live.count())) return 'no player yet';
      const now = await clock(live);
      return !now.paused && now.at > 0.3 && !(await live.evaluate((el) => el.classList.contains('taking'))) ? 'playing' : JSON.stringify(now);
    }, { message: 'its take plays on', timeout: 60_000 }).toBe('playing');
    await expect(region.locator('.glimpse'), 'nothing of the glimpse once its take plays').toHaveCount(0);
    const landed = await look(page, await live.boundingBox());
    await expect.poll(async () => moved(landed, await look(page, await live.boundingBox())), { message: "its take's picture moves on screen" }).toBeGreaterThanOrEqual(MOVES);
    await region.locator('.tl-transport').getByRole('button', { name: 'Pause', exact: true }).click();
    await leave(page);
  });
}

test('playback', async ({ page, project, cli }) => {
  await made({ page, project, cli });
  await plays(page, project);
});

/** config.toml in dev mode, its page from this Vite (web/access.js's devConfig). */
function devConfig(text) {
  const lines = text.split('\n').filter((line) => !/^\s*dev\s*=/.test(line) && !/^\s*development_web\s*=/.test(line));
  const table = lines.findIndex((line) => /^\s*\[calcifer\]\s*$/.test(line));
  if (table >= 0) lines.splice(table + 1, 0, 'development_web = "127.0.0.1:5173"');
  else lines.push('[calcifer]', 'development_web = "127.0.0.1:5173"');
  return ['dev = true', ...lines].join('\n') + '\n';
}

// As Paul's laptop serves it: Calcifer in dev mode, the page React's development build from Vite (StrictMode's second mount and all), the
// web of the cozy under test ($COZY_WEB_SOURCE, e2e.yaml), else this checkout's.
test('playback: the page from Vite, as dev = true serves it', { tag: '@solo' }, async ({ page, project, cli, calciferControl, guards }) => {
  test.setTimeout(900_000);
  const web = join(resolve(process.env.COZY_WEB_SOURCE || join(__dirname, '../../..')), 'web/app');
  expect(existsSync(join(web, 'node_modules/.bin/vite')), `Vite installed in ${web} (e2e.yaml installs it for web-playback)`).toBe(true);
  await made({ page, project, cli });
  const log = openSync(test.info().outputPath('vite.log'), 'a');
  const vite = spawn(join(web, 'node_modules/.bin/vite'), ['--host', '127.0.0.1', '--port', '5173', '--strictPort'], { cwd: web, detached: true, stdio: ['ignore', log, log] });
  // React's development build says what it warns of as console errors: kept with the cell, not its verdict, which is the screen's.
  const said = [];
  guards.allow(/^console error:/);
  page.on('console', (message) => said.push(`${message.type()}: ${message.text()}`));
  try {
    await expect.poll(() => fetch('http://127.0.0.1:5173/@vite/client').then((r) => r.status, () => 0), { message: 'Vite answers', timeout: 120_000 }).toBe(200);
    const config = join(homedir(), '.cozy', 'config.toml');
    writeFileSync(config, devConfig(existsSync(config) ? readFileSync(config, 'utf8') : ''));
    await calciferControl.stop();
    await calciferControl.start();
    await page.goto(project.url(), { timeout: 180_000 });
    await expect(page.locator('.stage')).toBeVisible({ timeout: 180_000 });
    expect(await page.evaluate(() => Boolean(document.querySelector('script[src="/@vite/client"]'))), "the page is Vite's").toBe(true);
    await plays(page, project);
  } finally {
    writeFileSync(test.info().outputPath('console.log'), said.join('\n'));
    // Its server gone under an open page, Vite's client polls for it from a blob SharedWorker (vite client.mjs, waitForSuccessfulPing), which
    // the page's policy refuses: this test's doing, not the page's (run 44234311382 said it once the cell had already failed).
    guards.over();
    try { process.kill(-vite.pid, 'SIGKILL'); } catch { /* gone */ }
    closeSync(log);
  }
});
