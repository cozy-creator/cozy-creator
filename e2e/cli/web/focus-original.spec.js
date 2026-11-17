// focus-original (cl-1184; Paul, 11-17, a picture open in Focus: "my screen (in focus mode on the node) is displaying full resolution
// right?"): Focus draws a picture's own file at its own pixels, never the thumbnail Calcifer keeps for the canvas (a 512 px JPEG of an
// opaque picture). A tall one, fitted narrower than 512 screen pixels, drew that thumbnail. Zoomed past one screen pixel to each of its
// own, its pixels are drawn as they are, as an image editor draws them, not smoothed; fitted, smoothed. A 16,384 px panorama zooms to its
// own pixels (8× its fit was 0.6 of them), its 32 MP file decoding while the page draws on. Its looks, fitted and zoomed, at 1440×900 and
// 1920×1080 on paper and dark, are kept for a person to look at.
const { mkdirSync, readFileSync } = require('fs');
const { join } = require('path');
const { test, expect, canvasVisible, focus, png, themed } = require('./fixtures');
const { ffmpeg, fit, leave } = require('./hands');

// 4 MP RGB pictures, one 4:3 and one 9:16, and a panorama 16,384 px wide.
const WIDE = { name: 'wide', width: 2304, height: 1728 };
const TALL = { name: 'tall', width: 1512, height: 2688 };
const PANORAMA = { name: 'panorama', width: 16384, height: 2048 };
const LOOKS = [[1440, 900], [1920, 1080]];
const soft = expect.configure({ soft: true });

/** A picture with detail at every scale: colour sweeps, a grid of single pixels every 32, fine diagonals. A copy, or its pixels smoothed,
 *  blurs its single pixels. */
const chart = (width, height) => png(0, width, height, false, (x, y) => {
  if (x % 32 === 0 || y % 32 === 0) return [16, 16, 16];
  if ((x + y) % 7 === 0) return [248, 248, 248];
  return [(x * 255 / width) | 0, (y * 255 / height) | 0, (128 + 127 * Math.sin(x / 50) * Math.cos(y / 70)) | 0];
});

/** What Focus draws of its picture: its file or its thumbnail, from where, how many screen pixels to each of its own, and how. */
const drawn = (page) => page.evaluate(() => {
  const host = document.querySelector('.focus-viewer .buffered-image');
  const image = host?.querySelector('img');
  if (!host || !image?.complete || !image.naturalWidth) return null;
  const r = host.getBoundingClientRect();
  return {
    resolution: host.dataset.displayedResolution ?? null,
    width: image.naturalWidth,
    height: image.naturalHeight,
    src: new URL(image.currentSrc || image.src, location.href).pathname,
    across: Math.min(r.width / image.naturalWidth, r.height / image.naturalHeight) * devicePixelRatio,
    rendering: getComputedStyle(image).imageRendering,
  };
});

/** `name` in Focus, double-clicked on the canvas fitted: no box under what floats over it. */
async function focusOn(page, name) {
  const region = page.getByRole('region', { name: 'Focused asset' });
  if (await region.count()) {
    if (await region.getAttribute('data-focused-asset') === name) return region;
    await leave(page);
  }
  await fit(page);
  return focus(page, name);
}

/** Focus's − or + pressed until `done` says what's drawn is far enough, or it can't go further: what's drawn then. */
async function zoomUntil(page, region, button, done) {
  const press = region.getByRole('button', { name: button, exact: true });
  for (let i = 0; i < 40; i++) {
    const now = await drawn(page);
    if (now && done(now)) return now;
    if (await press.isDisabled()) return now;
    await press.click();
  }
  return drawn(page);
}

test('focus-original', async ({ page, project }) => {
  const shots = join(process.env.OUT, 'web', 'focus-original');
  mkdirSync(shots, { recursive: true });
  const panorama = test.info().outputPath('panorama.png');
  ffmpeg('ffmpeg')('-v', 'error', '-y', '-f', 'lavfi', '-i', `testsrc2=size=${PANORAMA.width}x${PANORAMA.height}`, '-frames:v', '1', panorama);
  await project.import(chart(WIDE.width, WIDE.height), { name: WIDE.name, at: { x: 0, y: 0 } });
  await project.import(chart(TALL.width, TALL.height), { name: TALL.name, at: { x: 420, y: 0 } });
  await project.import(readFileSync(panorama), { name: PANORAMA.name, at: { x: 0, y: 420 } });
  for (const { name } of [WIDE, TALL, PANORAMA]) await expect.poll(() => canvasVisible(page, name), { timeout: 60_000 }).toBe(true);
  /** Its file's address, as Calcifer serves it. */
  const fileOf = async (name) => project.at(`/${(await project.asset(name)).path.split('/').map(encodeURIComponent).join('/')}`);
  await page.setViewportSize({ width: 1440, height: 900 });

  await test.step('a 4:3 picture, then a 9:16 one, each its own file in Focus, fitted', async () => {
    for (const one of [WIDE, TALL]) {
      await focusOn(page, one.name);
      const file = await fileOf(one.name);
      await soft.poll(() => drawn(page), { message: `${one.name}: its own file, ${one.width}×${one.height}, from ${file}`, timeout: 30_000 })
        .toMatchObject({ resolution: 'original', width: one.width, height: one.height, src: file });
      test.info().annotations.push({ type: one.name, description: JSON.stringify(await drawn(page)) });
    }
  });

  await test.step('zoomed past a screen pixel to each of its own, its pixels as they are; fitted, smoothed', async () => {
    const region = await focusOn(page, WIDE.name);
    await expect.poll(async () => (await drawn(page))?.width ?? 0, { message: 'its picture drawn', timeout: 30_000 }).toBeGreaterThan(0);
    const fitted = await drawn(page);
    expect.soft(fitted.across, 'fitted, fewer screen pixels than its own').toBeLessThan(1);
    expect.soft(fitted.rendering, 'fitted, smoothed').toBe('auto');
    const close = await zoomUntil(page, region, 'Zoom in image', (now) => now.across > 2);
    expect(close.across, 'zoomed in, more screen pixels than its own').toBeGreaterThan(2);
    await soft.poll(async () => (await drawn(page)).rendering, { message: `at ${close.across.toFixed(2)} screen pixels to each of its own, its pixels as they are` })
      .toBe('pixelated');
    const back = await zoomUntil(page, region, 'Zoom out image', (now) => now.across < 1);
    expect(back.across, 'zoomed out again').toBeLessThan(1);
    await soft.poll(async () => (await drawn(page)).rendering, { message: 'fitted again, smoothed' }).toBe('auto');
  });

  await test.step('a 16,384 px panorama: its 32 MP file drawn as the page draws on, zoomed to its own pixels', async () => {
    // Every frame from here to its file drawn: the longest gap between two.
    await page.evaluate(() => {
      const seen = (window.__frames = { longest: 0, last: performance.now(), id: 0 });
      const tick = (now) => {
        seen.longest = Math.max(seen.longest, now - seen.last);
        seen.last = now;
        seen.id = requestAnimationFrame(tick);
      };
      seen.id = requestAnimationFrame(tick);
    });
    const region = await focusOn(page, PANORAMA.name);
    const file = await fileOf(PANORAMA.name);
    await soft.poll(() => drawn(page), { message: `its own file, from ${file}`, timeout: 60_000 })
      .toMatchObject({ resolution: 'original', width: PANORAMA.width, height: PANORAMA.height, src: file });
    const longest = await page.evaluate(() => {
      cancelAnimationFrame(window.__frames.id);
      return Math.round(window.__frames.longest);
    });
    expect.soft(longest, 'the longest frame while it opened and its file decoded, ms').toBeLessThan(1000);
    const fitted = await drawn(page);
    const most = await zoomUntil(page, region, 'Zoom in image', () => false);
    test.info().annotations.push({ type: 'panorama', description: JSON.stringify({ longest, fitted, most }) });
    expect.soft(most.across, `zoomed all the way in, screen pixels to each of its own (fitted ${fitted.across.toFixed(3)})`).toBeGreaterThanOrEqual(1);
    await soft.poll(async () => (await drawn(page)).rendering, { message: 'and its pixels as they are' }).toBe('pixelated');
  });

  await test.step('its looks, fitted and zoomed, at 1440×900 and 1920×1080 on paper and dark', async () => {
    for (const theme of ['paper', 'dark']) for (const [width, height] of LOOKS) {
      const tag = `${width}x${height}-${theme}`;
      await project.look(theme);
      await page.setViewportSize({ width, height });
      // The canvas's own address, not a reload of Focus's: Focus comes back after the page is up, under the next step's feet.
      await page.goto(project.url());
      await expect(page.locator('.stage')).toBeVisible();
      await themed(page, theme);
      for (const one of [WIDE, TALL]) {
        const region = await focusOn(page, one.name);
        await expect.poll(async () => (await drawn(page))?.width ?? 0, { message: `${tag}: ${one.name} drawn`, timeout: 30_000 }).toBeGreaterThan(0);
        await zoomUntil(page, region, 'Zoom out image', () => false);
        await page.screenshot({ path: join(shots, `${one.name}-fitted-${tag}.png`) });
        if (one !== WIDE) continue;
        await zoomUntil(page, region, 'Zoom in image', (now) => now.across > 2.5);
        await page.waitForFunction(() => document.querySelector('.focus-viewer .buffered-image')?.hasAttribute('data-pixelated'), null, { timeout: 5_000 }).catch(() => {});
        await page.screenshot({ path: join(shots, `${one.name}-zoomed-${tag}.png`) });
      }
    }
  });
});
