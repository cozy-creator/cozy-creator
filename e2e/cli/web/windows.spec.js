// windows (cl-1215). Paul, 11-18: "I have 3 browser windows open, on the same canvas, why can't I have all 3 videos playing at the same
// time? currently if I play video-A it stops video-B from playing, which stops video-C, and so on. Why? stop this", and "a video playing in
// the browser can play (it's cached in the browser, and doesn't need a persistent connection), it doesn't need a live connection". Three
// windows of one browser (one pool of six connections to Calcifer) on one project, each in Focus: two videos playing and an orbit spinning.
// For 20 s every clock and every picture moves, and every window's calls answer within a second. And none of them holds a connection,
// playing or paused (cl-1220), read from this browser's sockets to Calcifer (web/sockets.js): none holding bytes at two looks in a row
// while they play, none 6 s after all three pause. That's the property whichever way the bytes come: a file read whole into the page
// (cl-1215) or asked for in pieces (cl-1220). Its videos are a take's size (120 MB each), more than a browser reads ahead, so an answer
// that stays open holds bytes in its socket and is red here (the whole remainder unrationed, which froze the page; a DevTools request
// can stay open with nothing held, so it isn't what's read). Pictures of all three playing: $OUT/web/windows/.
const { mkdirSync, readFileSync } = require('fs');
const { join } = require('path');
const { test, expect, focus, canvasVisible, fixtureTool, screenshotPixels } = require('./fixtures');
const { ffmpeg, fit } = require('./hands');
const { sockets } = require('./sockets');

const TURN = fixtureTool('orbit.py/turn');
const PLAY_MS = 20_000;
// Between looks: not a multiple of the orbit's 2 s loop, so a look never lands on the frame it saw.
// Also the bound on a socket holding bytes while they play: a megabyte piece in flight takes about 1.06 s on the slowest link cl-1220
// measured (the page held to 1 MB/s, 44241061810: 10.3 MB in 10.4 s), so one holding bytes at two looks 3.3 s apart is held.
const LOOK_MS = 3_300;

/** What's on screen over `box`'s middle: a 16 × 9 grid over its middle 70 % across and 60 % down, clear of the orbit's bar and hint. */
async function look(page, box) {
  const at = screenshotPixels(await page.screenshot({ scale: 'css' }));
  const seen = [];
  for (let row = 0; row < 9; row++) for (let column = 0; column < 16; column++) seen.push(at(box.x + box.width * (0.15 + 0.7 * column / 15), box.y + box.height * (0.2 + 0.6 * row / 8)));
  return seen;
}
const moved = (a, b) => a.filter((rgb, i) => Math.max(...rgb.map((value, c) => Math.abs(value - b[i][c]))) > 40).length;

/** One of the page's own calls, timed: how long its project took to answer. */
const call = (page, id) => page.evaluate(async (id) => {
  const began = performance.now();
  const answer = await fetch(`/v1/projects/${encodeURIComponent(id)}?assets=false`, { cache: 'no-store' });
  await answer.text();
  return { status: answer.status, ms: Math.round(performance.now() - began) };
}, id);

test('windows', async ({ page, project, cli, context, signIn }) => {
  test.setTimeout(420_000);
  await cli.json('machine', 'use', 'local');
  const shots = join(process.env.OUT, 'web', 'windows');
  mkdirSync(shots, { recursive: true });
  // 40 s at a constant 24 Mbit/s (filler where the picture needs less): 120 MB, a take's size.
  for (const [name, source, x] of [['walk', 'testsrc2', 0], ['rush', 'mandelbrot', 520]]) {
    const clip = test.info().outputPath(`${name}.mp4`);
    ffmpeg('ffmpeg')('-v', 'error', '-y', '-f', 'lavfi', '-i', `${source}=size=640x360:rate=24`, '-t', '40', '-c:v', 'libx264', '-preset', 'veryfast',
      '-b:v', '24M', '-minrate', '24M', '-maxrate', '24M', '-bufsize', '6M', '-x264-params', 'nal-hrd=cbr:force-cfr=1', '-pix_fmt', 'yuv420p', clip);
    const bytes = readFileSync(clip);
    expect(bytes.length, `${name}: 40 s at 24 Mbit/s`).toBeGreaterThan(100e6);
    await project.import(bytes, { name, at: { x, y: 0 } });
  }
  await project.add({ name: 'turned', tool: TURN, kind: 'video', fields: { frames: 48 }, at: { x: 1040, y: 0 } });
  const rendered = await cli('--json', 'render', 'turned', '--on', 'local');
  expect(rendered.code, `cozy render: ${rendered.stderr.slice(-400)}`).toBe(0);
  await expect.poll(async () => Boolean((await project.asset('turned'))?.prefixes?.at(-1)?.main?.metadata?.orbit), { message: "turned's orbit take", timeout: 120_000 }).toBe(true);

  const windows = [{ page, name: 'walk' }, { name: 'rush' }, { name: 'turned' }];
  for (const one of windows) {
    if (!one.page) {
      one.page = await context.newPage();
      await signIn(one.page, project.url());
    }
    await one.page.setViewportSize({ width: 1280, height: 800 });
    await expect(one.page.locator('.stage')).toBeVisible();
    for (const name of ['walk', 'rush', 'turned']) await expect.poll(() => canvasVisible(one.page, name), { timeout: 60_000 }).toBe(true);
    await fit(one.page);
    one.region = one.page.getByRole('region', { name: 'Focused asset' });
  }

  await test.step('each window plays its own: two videos and an orbit spinning, all at once', async () => {
    for (const one of windows) {
      await one.page.bringToFront();
      await focus(one.page, one.name);
      if (one.name === 'turned') {
        await expect(one.region.locator('.orbit.ready')).toBeVisible({ timeout: 30_000 });
        await one.region.locator('.orbit-bar').getByRole('button', { name: 'Spin', exact: true }).click();
        one.video = one.region.locator('video.orbit-video');
        one.box = await one.region.getByRole('slider', { name: 'Turn', exact: true }).boundingBox();
      } else {
        await one.region.locator('.tl-transport').getByRole('button', { name: 'Play', exact: true }).click();
        one.video = one.region.locator('video.live');
        await expect.poll(async () => (await one.video.count()) && one.video.evaluate((el) => el.currentTime), { message: `${one.name} plays`, timeout: 30_000 }).toBeGreaterThan(0.2);
        one.box = await one.video.boundingBox();
      }
    }
  });

  const port = new URL(project.url()).port;
  const held = [];
  await test.step(`for ${PLAY_MS / 1000} s every clock and picture moves in every window, every window's calls answer within a second, and no socket stays holding bytes`, async () => {
    let holding = sockets(port).holding;
    for (const one of windows) {
      one.was = { clock: await one.video.evaluate((el) => ({ at: el.currentTime, paused: el.paused })), picture: await look(one.page, one.box) };
      one.calls = [];
    }
    const began = Date.now();
    let round = 0;
    while (Date.now() - began < PLAY_MS) {
      await new Promise((done) => setTimeout(done, LOOK_MS));
      round++;
      for (const one of windows) {
        const clock = await one.video.evaluate((el) => ({ at: el.currentTime, paused: el.paused }));
        const picture = await look(one.page, one.box);
        expect.soft(clock.paused, `round ${round}: ${one.name} still playing`).toBe(false);
        expect.soft(clock.at, `round ${round}: ${one.name}'s clock moved`).not.toBeCloseTo(one.was.clock.at, 2);
        expect.soft(moved(one.was.picture, picture), `round ${round}: ${one.name}'s picture moved`).toBeGreaterThanOrEqual(6);
        one.was = { clock, picture };
        one.calls.push(await call(one.page, project.id));
      }
      if (round === 2) for (const [i, one] of windows.entries()) await one.page.screenshot({ path: join(shots, `window-${i + 1}-${one.name}.png`) });
      const now = sockets(port);
      held.push({ round, ...now, still: now.holding.filter((local) => holding.includes(local)) });
      holding = now.holding;
    }
    for (const one of windows) {
      expect.soft(one.calls.filter((answer) => answer.status !== 200 || answer.ms > 1000), `${one.name}: every call answered within a second`).toEqual([]);
    }
    expect.soft(Math.max(...held.map((one) => one.open)), "this browser's sockets to Calcifer are seen").toBeGreaterThan(0);
    expect.soft(held.filter((one) => one.still.length).map(({ round, still, unread_kb, unsent_kb }) => ({ round, still, unread_kb, unsent_kb })),
      'no socket holds bytes at two looks in a row while they play').toEqual([]);
  });

  await test.step('6 s after all three pause, no socket holds bytes', async () => {
    for (const one of windows) {
      await one.page.bringToFront();
      const button = one.name === 'turned' ? one.region.locator('.orbit-bar').getByRole('button', { name: 'Stop', exact: true })
        : one.region.locator('.tl-transport').getByRole('button', { name: 'Pause', exact: true });
      await button.click();
      await expect.poll(() => one.video.evaluate((el) => el.paused), { message: `${one.name} paused` }).toBe(true);
    }
    await new Promise((done) => setTimeout(done, 6_000));
    const paused = sockets(port);
    const measured = { calls: windows.map((one) => ({ window: one.name, calls: one.calls })), playing: held, paused };
    console.log(`windows, measured: ${JSON.stringify(measured)}`);
    test.info().annotations.push({ type: 'measured', description: JSON.stringify(measured) });
    expect.soft({ unread: paused.unread, unsent: paused.unsent }, 'no socket holds bytes 6 s after all three pause').toEqual({ unread: 0, unsent: 0 });
  });
});
