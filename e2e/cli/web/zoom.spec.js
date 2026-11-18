// zoom (cl-1190; Paul, 11-17: "on videos, especially the 360 orbits, I should be able to zoom in on a section of the video"): Focus is one
// viewer for a picture, a video and an orbit. The wheel zooms about the pointer, what was under it stays under it; zoomed, a drag pans; a
// double-click or Fit goes back to the fit. A video zoomed is its own file at its own size, drawn as its pixels are past 1:1, and plays and
// scrubs on. An orbit's drag still turns it when zoomed, as far as at the fit, and a Shift-drag pans it; its bar isn't zoomed. Its looks,
// zoomed, at 1440×900 and 1920×1080 on paper and dark, are kept for a person to look at.
const { mkdirSync, readFileSync } = require('fs');
const { join } = require('path');
const { test, expect, canvasVisible, focus, fixtureTool, themed } = require('./fixtures');
const { ffmpeg, fit, leave } = require('./hands');

const CLIP = { name: 'clip', width: 1280, height: 720 };
const TURN = fixtureTool('orbit.py/turn');
const soft = expect.configure({ soft: true });

/** `name` in Focus, double-clicked on the canvas fitted. */
async function focusOn(page, name) {
  const region = page.getByRole('region', { name: 'Focused asset' });
  if (await region.count()) {
    if (await region.getAttribute('data-focused-asset') === name) return region;
    await leave(page);
  }
  await fit(page);
  return focus(page, name);
}

/** How Focus draws what it shows now: its zoom and pan, and `selector`'s box on screen, source, own size and how its pixels are drawn. */
const view = (page, selector) => page.evaluate((selector) => {
  const frame = document.querySelector('.focus-viewer .focus-image-transform');
  const el = document.querySelector(`.focus-viewer ${selector}`);
  if (!frame || !el) return null;
  const m = new DOMMatrix(getComputedStyle(frame).transform);
  const r = el.getBoundingClientRect();
  return {
    scale: m.a, x: m.e, y: m.f,
    box: { x: r.x, y: r.y, width: r.width, height: r.height },
    src: el.currentSrc ? new URL(el.currentSrc, location.href).pathname : null,
    natural: el.videoWidth ? [el.videoWidth, el.videoHeight] : null,
    at: el.currentTime ?? null,
    paused: el.paused ?? null,
    rendering: getComputedStyle(el).imageRendering,
    fitShown: Boolean(document.querySelector('.focus-viewer .focus-image-zoom [aria-label="Fit to frame"]')),
  };
}, selector);

/** The middle of the frame as fitted, which zooming never moves: on what's looked at, under none of its controls. */
const middleOf = (page) => page.evaluate(() => {
  const r = document.querySelector('.focus-viewer .focus-frame').getBoundingClientRect();
  return { x: r.x + r.width / 2, y: r.y + r.height / 2 };
});

/** Where `point` is in `box`, as fractions of its width and height. */
const within = (box, point) => ({ u: (point.x - box.x) / box.width, v: (point.y - box.y) / box.height });

/** Every pan Focus draws from now, by its frame's style as it's set, and every pointer move and up under it, each at its time: `stop()`
 *  returns them. A pan goes only the way the pointer went, and is where it went within a frame or two of the up (cl-1208: 44222701495
 *  read 90, 30 of a 120, 40 pan 9 ms after the up). */
const recordPan = (page) => page.evaluate(() => {
  const frame = document.querySelector('.focus-viewer .focus-image-transform');
  const seen = [], quit = new AbortController();
  const at = () => Math.round(performance.now() * 10) / 10;
  const drawn = () => { const m = new DOMMatrix(getComputedStyle(frame).transform); seen.push({ t: at(), drawn: [Math.round(m.e), Math.round(m.f)] }); };
  const observer = new MutationObserver(drawn);
  observer.observe(frame, { attributes: true, attributeFilter: ['style'] });
  for (const type of ['pointermove', 'pointerup']) addEventListener(type, (event) => seen.push({ t: at(), [type]: [event.clientX, event.clientY] }), { capture: true, signal: quit.signal });
  window.__panStop = () => { observer.disconnect(); quit.abort(); return seen; };
}).then(() => ({ stop: () => page.evaluate(() => window.__panStop()) }));

/** The pan's record: kept with the cell, and it never went back against the pointer, on either axis. */
function panWent(seen, sign, what) {
  test.info().annotations.push({ type: `pan: ${what}`, description: JSON.stringify(seen) });
  console.log(`pan, ${what}: ${JSON.stringify(seen)}`);
  const drawn = seen.filter((one) => one.drawn).map((one) => one.drawn);
  const back = drawn.findIndex((now, i) => i > 0 && [0, 1].some((axis) => (now[axis] - drawn[i - 1][axis]) * sign[axis] < -1));
  expect.soft(back, `${what}: drawn only the way the pointer went, never back (${drawn.map((one) => one.join(',')).join(' → ')})`).toBe(-1);
}

test('zoom', async ({ page, project, cli }) => {
  await cli.json('machine', 'use', 'local');
  const shots = join(process.env.OUT, 'web', 'zoom');
  mkdirSync(shots, { recursive: true });
  const clip = test.info().outputPath('clip.mp4');
  ffmpeg('ffmpeg')('-v', 'error', '-y', '-f', 'lavfi', '-i', `testsrc2=size=${CLIP.width}x${CLIP.height}:rate=24:duration=4`, '-c:v', 'libx264', '-preset', 'veryfast', '-pix_fmt', 'yuv420p', clip);
  await project.import(readFileSync(clip), { name: CLIP.name, at: { x: 0, y: 0 } });
  await project.add({ name: 'turned', tool: TURN, kind: 'video', fields: { frames: 48 }, at: { x: 520, y: 0 } });
  const rendered = await cli('--json', 'render', 'turned', '--on', 'local');
  expect(rendered.code, `cozy render: ${rendered.stderr.slice(-400)}`).toBe(0);
  await expect.poll(async () => Boolean((await project.asset('turned'))?.prefixes?.at(-1)?.main), { message: "turned's take", timeout: 120_000 }).toBe(true);
  for (const name of [CLIP.name, 'turned']) await expect.poll(() => canvasVisible(page, name), { timeout: 60_000 }).toBe(true);
  const file = project.at(`/${(await project.asset(CLIP.name)).path.split('/').map(encodeURIComponent).join('/')}`);
  await page.setViewportSize({ width: 1440, height: 900 });
  const region = page.getByRole('region', { name: 'Focused asset' });

  await test.step('a video: the wheel zooms it about the pointer, its own file at its own pixels', async () => {
    await focusOn(page, CLIP.name);
    await expect.poll(async () => (await view(page, 'video.live'))?.natural ?? null, { message: 'its own frame drawn' }).toEqual([CLIP.width, CLIP.height]);
    const fitted = await view(page, 'video.live');
    expect.soft(fitted.src, 'its own file, not a thumbnail or a preview').toBe(file);
    expect.soft(fitted.rendering, 'fitted, smoothed').toBe('auto');
    const pointer = { x: fitted.box.x + fitted.box.width * 0.3, y: fitted.box.y + fitted.box.height * 0.4 };
    await page.mouse.move(pointer.x, pointer.y);
    await page.mouse.wheel(0, -600);
    await soft.poll(async () => (await view(page, 'video.live'))?.scale ?? 1, { message: 'the wheel zooms the video' }).toBeGreaterThan(2.5);
    const zoomed = await view(page, 'video.live');
    test.info().annotations.push({ type: 'video zoomed', description: JSON.stringify({ fitted, zoomed }) });
    const held = within(zoomed.box, pointer);
    expect.soft(Math.abs(held.u - 0.3) + Math.abs(held.v - 0.4), `what was under the pointer stays there: ${JSON.stringify(held)}`).toBeLessThan(0.02);
    expect.soft(zoomed.box.width / fitted.box.width, 'drawn at its zoom').toBeCloseTo(zoomed.scale, 1);
    expect.soft(zoomed.src, 'zoomed, still its own file').toBe(file);
    await soft.poll(async () => (await view(page, 'video.live'))?.rendering, { message: `at ${(zoomed.box.width / CLIP.width).toFixed(2)} screen pixels to each of its own, its pixels as they are` }).toBe('pixelated');
    expect.soft(zoomed.fitShown, 'Fit offered while zoomed').toBe(true);
  });

  await test.step('zoomed, it plays and scrubs on, a drag pans it, a double-click and Fit go back to the fit', async () => {
    await region.getByRole('button', { name: 'Play', exact: true }).click();
    const playing = await view(page, 'video.live');
    await soft.poll(async () => (await view(page, 'video.live'))?.at ?? 0, { message: 'it plays, zoomed' }).toBeGreaterThan(playing.at + 0.3);
    await region.getByRole('button', { name: 'Pause', exact: true }).click();
    await region.getByRole('slider', { name: 'Scrub', exact: true }).fill('36');
    await soft.poll(async () => (await view(page, 'video.live'))?.at ?? 0, { message: 'it scrubs, zoomed' }).toBeGreaterThan(1.4);
    const before = await view(page, 'video.live');
    expect.soft(before.scale, 'still zoomed after playing and scrubbing').toBeGreaterThan(2.5);
    const from = await middleOf(page);
    await page.mouse.move(from.x, from.y);
    const videoPan = await recordPan(page);
    await page.mouse.down();
    await page.mouse.move(from.x - 160, from.y - 60, { steps: 8 });
    await page.mouse.up();
    // Exactly where it went, within a second: the last moves' render can come a frame after the pointer's up. 44222701495 read the orbit's
    // pan 9 ms after it, at its sixth move of eight (90, 30), while the page's DOM held 120, 40 before and after.
    await soft.poll(async () => { const panned = await view(page, 'video.live'); return { dx: Math.round(panned.x - before.x), dy: Math.round(panned.y - before.y) }; }, { message: 'a drag pans it as far as it went', timeout: 1_000 }).toEqual({ dx: -160, dy: -60 });
    panWent(await videoPan.stop(), [-1, -1], 'the video');
    const middle = await middleOf(page);
    await page.mouse.dblclick(middle.x, middle.y);
    await soft.poll(async () => (await view(page, 'video.live'))?.scale, { message: 'a double-click: the fit' }).toBe(1);
    expect.soft(await page.evaluate(() => Boolean(document.fullscreenElement)), 'zoomed, a double-click is no full screen').toBe(false);
    await region.getByRole('button', { name: 'Zoom in image', exact: true }).click();
    await region.getByRole('button', { name: 'Fit to frame', exact: true }).click();
    await soft.poll(async () => (await view(page, 'video.live'))?.scale, { message: 'Fit: the fit' }).toBe(1);
  });

  await test.step("an orbit: zoomed, a drag still turns it as far as at the fit, a Shift-drag pans it, its bar isn't zoomed", async () => {
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await focusOn(page, 'turned');
    const orbit = region.getByRole('slider', { name: 'Turn', exact: true });
    await expect(region.locator('.orbit.ready')).toBeVisible();
    const bar = async () => (await region.locator('.orbit-bar').boundingBox())?.height ?? 0;
    const barFitted = await bar();
    const fitted = await view(page, '.orbit');
    await page.mouse.move(fitted.box.x + fitted.box.width / 2, fitted.box.y + fitted.box.height / 2);
    await page.mouse.wheel(0, -500);
    await soft.poll(async () => (await view(page, '.orbit'))?.scale ?? 1, { message: 'the wheel zooms the orbit' }).toBeGreaterThan(2);
    const zoomed = await view(page, '.orbit');
    expect.soft(Math.abs((await bar()) - barFitted), `its bar the size it was (${barFitted}px)`).toBeLessThan(1);
    await soft.poll(async () => (await view(page, '.orbit-video'))?.rendering, { message: 'its pixels as they are, zoomed' }).toBe('pixelated');
    // A quarter of the frame's fitted width turns it a quarter round, zoomed as at the fit.
    const middle = await middleOf(page);
    const y = middle.y, x0 = middle.x - fitted.box.width / 8;
    await page.mouse.move(x0, y);
    await page.mouse.down();
    await page.mouse.move(x0 + fitted.box.width / 4, y, { steps: 8 });
    await page.mouse.up();
    await soft.poll(async () => Number(await orbit.getAttribute('data-angle')), { message: 'zoomed, a drag turns it a quarter round' }).toBeCloseTo(90, -1);
    const turned = await view(page, '.orbit');
    expect.soft({ dx: Math.round(turned.x - zoomed.x), dy: Math.round(turned.y - zoomed.y) }, 'and pans nothing').toEqual({ dx: 0, dy: 0 });
    const angle = Number(await orbit.getAttribute('data-angle'));
    await page.keyboard.down('Shift');
    await page.mouse.move(middle.x, y);
    const orbitPan = await recordPan(page);
    await page.mouse.down();
    await page.mouse.move(middle.x + 120, y + 40, { steps: 8 });
    await page.mouse.up();
    await page.keyboard.up('Shift');
    await soft.poll(async () => { const panned = await view(page, '.orbit'); return { dx: Math.round(panned.x - turned.x), dy: Math.round(panned.y - turned.y) }; }, { message: 'a Shift-drag pans it', timeout: 1_000 }).toEqual({ dx: 120, dy: 40 });
    panWent(await orbitPan.stop(), [1, 1], 'the orbit');
    expect.soft(Number(await orbit.getAttribute('data-angle')), 'and turns nothing').toBeCloseTo(angle, 3);
    await page.mouse.dblclick(middle.x, y);
    await soft.poll(async () => (await view(page, '.orbit'))?.scale, { message: 'a double-click: the fit' }).toBe(1);
    await page.emulateMedia({ reducedMotion: null });
  });

  await test.step('its looks, zoomed, at 1440×900 and 1920×1080 on paper and dark', async () => {
    for (const theme of ['paper', 'dark']) for (const [width, height] of [[1440, 900], [1920, 1080]]) {
      const tag = `${width}x${height}-${theme}`;
      await project.look(theme);
      await page.setViewportSize({ width, height });
      await page.goto(project.url());
      await expect(page.locator('.stage')).toBeVisible();
      await themed(page, theme);
      for (const [name, selector] of [[CLIP.name, 'video.live'], ['turned', '.orbit']]) {
        await focusOn(page, name);
        await expect.poll(async () => (await view(page, selector))?.box.width ?? 0, { message: `${tag}: ${name} drawn` }).toBeGreaterThan(0);
        const at = (await view(page, selector)).box;
        await page.mouse.move(at.x + at.width * 0.35, at.y + at.height * 0.45);
        await page.mouse.wheel(0, -500);
        await expect.poll(async () => (await view(page, selector))?.scale ?? 1).toBeGreaterThan(1.5);
        await page.screenshot({ path: join(shots, `${name}-zoomed-${tag}.png`) });
      }
    }
  });
});
