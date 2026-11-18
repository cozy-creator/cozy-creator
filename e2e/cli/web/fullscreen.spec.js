// fullscreen (cl-1080). Paul, 11-16: "the video player should have a full-screen option, like youtube-style"; "also never add blur over the
// video (when paused or anywhere else)"; and "when I click on a video, focus on it, the video frame displayed to me is still the old thumbnail
// … which is very tiny". In Focus a video shows its own frame at once, at its own size, nothing drawn larger than it is and nothing blurred
// over it. Full screen by its button, by F and by a double-click: the same <video> going on from where it was, letterboxed at its own aspect
// (a 9:16 one too), its track and transport over its bottom and working there, gone while the pointer rests as it plays and back as it moves.
// Esc leaves full screen and not Focus; F typed in a field is a letter. An orbit turns by drag in full screen. Pictures to look at, in and out
// of full screen at 1440×900 and 1920×1080, paper and dark: $OUT/web/fullscreen/<browser>. Chrome in lane 2, Firefox in the engines lane.
const { mkdirSync, readFileSync } = require('fs');
const { join } = require('path');
const { test, expect, themed, focus, canvasVisible, canvasIdle, fixtureTool } = require('./fixtures');
const { ffmpeg, fit, leave } = require('./hands');

const FPS = 24;
// H3's stand-in (project/segments.py): a take in segments, each beat's length, its seams in its result.
const SEGMENTS = fixtureTool('segments.py/segments');
const BEAT_S = 2;
// The transport goes once the pointer has rested this long (FocusFrame.tsx).
const REST_MS = 2500;
const need = (name) => process.env[name] || (() => { throw new Error(`$${name} is not set: web.sh's web_suite sets it`); })();

/** `seconds` of a test pattern `width`×`height` under a tone, as a person's clip is; `keys`: every frame a keyframe, as an orbit's are. */
function clip(path, width, height, seconds, keys = false) {
  ffmpeg('ffmpeg')('-v', 'error', '-y', '-f', 'lavfi', '-i', `testsrc2=size=${width}x${height}:rate=${FPS}:duration=${seconds}`, '-f', 'lavfi', '-i', `sine=frequency=330:duration=${seconds}`,
    '-c:v', 'libx264', '-preset', 'veryfast', '-pix_fmt', 'yuv420p', ...(keys ? ['-g', '1'] : []), '-c:a', 'aac', '-shortest', path);
  return readFileSync(path);
}

/** The player as a person sees it: the <video> at the picture's middle, drawn and on top, its own size; any picture in it drawn larger than
 *  its own pixels; any blur on or over the picture; and what's full screen. */
const seen = (page) => page.evaluate(() => {
  const media = document.querySelector('[data-focused-asset] .focus-image-transform > .media');
  if (!media) return { media: false };
  const video = media.querySelector('video.live');
  const visible = (el) => el.checkVisibility({ opacityProperty: true, visibilityProperty: true });
  const box = (video && visible(video) ? video : media).getBoundingClientRect();
  const stretched = [...media.querySelectorAll('img')].filter((img) => img.complete && img.naturalWidth && visible(img)).flatMap((img) => {
    const r = img.getBoundingClientRect(), fit = getComputedStyle(img).objectFit, x = r.width / img.naturalWidth, y = r.height / img.naturalHeight;
    const scale = fit === 'scale-down' ? Math.min(1, x, y) : fit === 'contain' ? Math.min(x, y) : fit === 'none' ? 1 : Math.max(x, y);
    return scale > 1.01 ? [`img.${img.className.trim().replace(/\s+/g, '.')} ${img.naturalWidth}x${img.naturalHeight} drawn ×${scale.toFixed(2)}`] : [];
  });
  const blurs = [];
  for (const el of document.querySelectorAll('*')) {
    const r = el.getBoundingClientRect();
    if (!(r.width && r.height && r.right > box.left && r.left < box.right && r.bottom > box.top && r.top < box.bottom)) continue;
    for (const pseudo of [null, '::before', '::after']) {
      const style = getComputedStyle(el, pseudo);
      const said = `${style.filter} ${style.backdropFilter ?? ''} ${style.webkitBackdropFilter ?? ''}`;
      if (/blur/.test(said)) blurs.push(`${el.tagName.toLowerCase()}.${[...el.classList].join('.')}${pseudo ?? ''}: ${said.trim()}`);
    }
  }
  const middle = document.elementFromPoint(box.x + box.width / 2, box.y + box.height / 2);
  return {
    media: true,
    video: video ? { drawn: video.readyState >= 2 && visible(video) && Boolean(video.currentSrc), on_top: middle === video, natural: [video.videoWidth, video.videoHeight], at: video.currentTime, paused: video.paused } : null,
    poster: [...media.querySelectorAll('img')].map((img) => [img.naturalWidth, img.naturalHeight]),
    stretched,
    blurs,
    full: document.fullscreenElement?.matches('.focus-viewer[data-full]') ?? false,
  };
});

/** What's full screen now: the player's frame, or nothing. */
const fullNow = (page) => page.evaluate(() => (document.fullscreenElement ? `${document.fullscreenElement.className}${document.fullscreenElement.hasAttribute('data-full') ? '[data-full]' : ''}` : null));

/** The picture and what's over its bottom, full screen, and the screen: {media, under, screen}. */
const laid = (page) => page.evaluate(() => {
  const rect = (el) => { if (!el) return null; const r = el.getBoundingClientRect(); return { x: r.x, y: r.y, width: r.width, height: r.height }; };
  const full = document.querySelector('.focus-viewer[data-full]');
  return { media: rect(full?.querySelector('.focus-image-transform > .media')), under: rect(full?.querySelector('.focus-under')), screen: { width: innerWidth, height: innerHeight } };
});

/** Letterboxed at `aspect`: as wide or as tall as the screen, in its middle. */
function letterboxed(at, aspect) {
  const { media, screen } = at;
  return {
    aspect: Math.abs(media.width / media.height - aspect) < 0.01,
    fills: Math.abs(media.width - screen.width) < 2 || Math.abs(media.height - screen.height) < 2,
    inside: media.width <= screen.width + 1 && media.height <= screen.height + 1,
    middle: Math.abs(media.x + media.width / 2 - screen.width / 2) < 2 && Math.abs(media.y + media.height / 2 - screen.height / 2) < 2,
  };
}

/** Full screen, once the frame has laid itself out on the screen: the picture letterboxed at `aspect`. */
async function letterboxedFull(page, aspect, what) {
  let at;
  await expect.configure({ soft: true }).poll(async () => {
    const ok = letterboxed((at = await laid(page)), aspect);
    return Object.values(ok).every(Boolean) ? 'letterboxed' : JSON.stringify({ ok, at });
  }, { message: `${what}: letterboxed` }).toBe('letterboxed');
  return at;
}

test('fullscreen', { tag: '@engines' }, async ({ page, project, api, cli }) => {
  // Its local tool's reading alone took up to 25 s in web-2 (e2e 44134428394), beside its own ~2.8 min.
  test.setTimeout(360_000);
  await cli.json('machine', 'use', 'local');
  const shots = join(need('OUT'), 'web', 'fullscreen', test.info().project.name);
  mkdirSync(shots, { recursive: true });
  await project.import(clip(test.info().outputPath('wide.mp4'), 1280, 720, 10), { name: 'wide', at: { x: 0, y: 0 } });
  await project.import(clip(test.info().outputPath('tall.mp4'), 720, 1280, 6), { name: 'tall', at: { x: 450, y: 0 } });
  await project.import(clip(test.info().outputPath('spin.mp4'), 640, 360, 2, true), { name: 'spin', at: { x: 900, y: 0 } });
  // An orbit: its file's metadata says so (design/file-metadata.md), set here on its version as a function that makes one declares it.
  await expect.poll(async () => (await project.asset('spin'))?.duration_ms ?? 0, { message: "spin's file read" }).toBeGreaterThan(0);
  await api.patch(project.at('/assets/spin/versions/0'), { metadata: { orbit: { degrees: 360, direction: 'cw', loops: true } } });
  await project.add({ name: 'shot', tool: SEGMENTS, kind: 'video', fields: { summary: 'Along the coast.', beats: [{ description: 'The cliffs.', duration: BEAT_S }, { description: 'The harbour.', duration: BEAT_S }], wait: 0 }, at: { x: 1350, y: 0 } });
  await expect.poll(async () => (await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}`)).tools.some((tool) => tool.local && tool.id === SEGMENTS), { message: 'the project lists its local tool', timeout: 180_000 }).toBe(true);
  for (const name of ['wide', 'tall', 'spin', 'shot']) await expect.poll(() => canvasVisible(page, name)).toBe(true);
  await fit(page);
  const region = page.getByRole('region', { name: 'Focused asset' });
  const screen = page.locator('.focus-viewer[data-full]');
  const transport = region.locator('.tl-transport');

  /** `name` in Focus shows its own frame, `size`, at once: on top, paused, nothing drawn larger than it is, nothing blurred over it. */
  const ownFrame = async (name, size) => {
    const began = Date.now();
    await expect.poll(async () => {
      const now = await seen(page);
      return now.video?.drawn && now.video.on_top ? now.video.natural : JSON.stringify(now);
    }, { message: `${name}: its own ${size.join('×')} frame at once, not its poster` }).toEqual(size);
    const ms = Date.now() - began;
    test.info().annotations.push({ type: `${name}-own-frame-ms`, description: String(ms) });
    const now = await seen(page);
    expect.soft(now.video.paused, `${name}: opened to show its frame, not to play`).toBe(true);
    expect.soft(now.stretched, `${name}: nothing in the frame drawn larger than its own pixels`).toEqual([]);
    expect.soft(now.blurs, `${name}: nothing blurs the picture`).toEqual([]);
    return now;
  };
  const playing = (video) => video.evaluate((el) => ({ at: el.currentTime, paused: el.paused, connected: el.isConnected, full: Boolean(document.fullscreenElement?.contains(el)) }));

  let held;
  await test.step('in Focus a video shows its own frame at once, at its own size, nothing blurred over it', async () => {
    await focus(page, 'wide');
    const now = await ownFrame('wide', [1280, 720]);
    expect.soft(now.poster, 'no poster under its own frame in Focus (cl-1214)').toEqual([]);
    held = await region.locator('video.live').elementHandle();
  });

  await test.step('full screen by its button: the same <video> plays on from where it was, letterboxed, its transport over it', async () => {
    await transport.getByRole('button', { name: 'Play', exact: true }).click();
    await expect.poll(async () => (await playing(held)).at, { message: 'it plays' }).toBeGreaterThan(1.5);
    const before = await playing(held);
    await transport.getByRole('button', { name: 'Full screen', exact: true }).click();
    await expect.poll(() => fullNow(page), { message: 'its frame is full screen' }).toBe('focus-viewer[data-full]');
    const after = await playing(held);
    expect.soft(after, 'the same <video>, in full screen, playing on').toMatchObject({ connected: true, full: true, paused: false });
    expect.soft(after.at, 'from where it was, s').toBeGreaterThanOrEqual(before.at);
    expect.soft(await region.locator('video.live').count(), 'one <video>').toBe(1);
    const at = await letterboxedFull(page, 16 / 9, '16:9');
    expect.soft(at.under.y + at.under.height, 'its transport at the bottom of the screen, px').toBeCloseTo(at.screen.height, 0);
    expect.soft(at.under.y, 'over the picture').toBeLessThan(at.media.y + at.media.height);
    await expect.soft(screen.locator('.tl-transport').getByRole('button', { name: 'Exit full screen', exact: true })).toBeVisible();
  });

  await test.step('in full screen its transport works, nothing blurs the paused picture, and it goes as the pointer rests and comes back', async () => {
    const controls = screen.locator('.tl-transport');
    await controls.getByRole('button', { name: 'Pause', exact: true }).click();
    await expect.poll(async () => (await playing(held)).paused, { message: 'Pause pauses it' }).toBe(true);
    const paused = await seen(page);
    expect.soft(paused.blurs, 'paused full screen: nothing blurs the picture').toEqual([]);
    expect.soft(paused.video.on_top, 'paused full screen: its own frame on top').toBe(true);
    const scrub = controls.getByRole('slider', { name: 'Scrub', exact: true });
    await scrub.focus();
    await page.keyboard.press('End');
    await expect.poll(async () => (await playing(held)).at, { message: 'the scrubber seeks to the end' }).toBeGreaterThan(9.5);
    await page.keyboard.press('Home');
    await expect.poll(async () => (await playing(held)).at, { message: 'and to the start' }).toBeLessThan(0.1);
    await controls.getByRole('button', { name: 'Play', exact: true }).click();
    await expect.poll(async () => (await playing(held)).at, { message: 'Play plays it' }).toBeGreaterThan(0.5);
    const picture = await laid(page);
    const middle = { x: picture.media.x + picture.media.width / 2, y: picture.media.y + picture.media.height / 3 };
    await page.mouse.move(middle.x, middle.y);
    const under = screen.locator('.focus-under');
    const opacity = () => under.evaluate((el) => getComputedStyle(el).opacity);
    await expect.poll(opacity, { message: 'gone once the pointer rests while it plays', timeout: REST_MS + 5_000 }).toBe('0');
    await page.mouse.move(middle.x + 40, middle.y + 10, { steps: 2 });
    await expect.poll(opacity, { message: 'back as it moves' }).toBe('1');
  });

  await test.step('Esc leaves full screen, and not Focus: the same <video> playing on', async () => {
    const before = await playing(held);
    await page.keyboard.press('Escape');
    await expect.poll(() => fullNow(page), { message: 'out of full screen' }).toBeNull();
    await expect(region, 'still in Focus').toHaveAttribute('data-focused-asset', 'wide');
    const after = await playing(held);
    expect.soft(after, 'the same <video>, playing on').toMatchObject({ connected: true, full: false, paused: false });
    expect.soft(after.at, 'from where it was, s').toBeGreaterThanOrEqual(before.at);
  });

  await test.step('F toggles full screen, paused where it is; in a field F is a letter', async () => {
    await transport.getByRole('button', { name: 'Pause', exact: true }).click();
    await expect.poll(async () => (await playing(held)).paused).toBe(true);
    const at = (await playing(held)).at;
    await page.keyboard.press('f');
    await expect.poll(() => fullNow(page), { message: 'F: full screen' }).toBe('focus-viewer[data-full]');
    expect.soft(await playing(held), 'F: the same <video>, still paused where it was').toMatchObject({ connected: true, full: true, paused: true, at });
    await page.keyboard.press('f');
    await expect.poll(() => fullNow(page), { message: 'F again: out' }).toBeNull();
    await region.getByRole('button', { name: 'Rename box', exact: true }).click();
    const field = region.getByRole('textbox', { name: 'Name', exact: true });
    await field.click();
    await page.keyboard.press('End');
    await page.keyboard.press('f');
    // A declared window: half a second for a toggle that shouldn't come.
    expect.soft(await page.evaluate(() => new Promise((done) => setTimeout(() => done(document.fullscreenElement), 500))), 'F typed in a field: no full screen').toBeNull();
    await expect.soft(field, 'F typed in a field: a letter').toHaveValue(/f$/);
    await page.keyboard.press('Escape');
    await expect(field).toHaveCount(0);
    await expect(region, 'still in Focus').toHaveAttribute('data-focused-asset', 'wide');
  });

  await test.step('a double-click on the picture toggles full screen, its play state kept', async () => {
    const picture = await region.locator('.focus-image-transform > .media').boundingBox();
    await page.mouse.dblclick(picture.x + picture.width / 2, picture.y + picture.height / 2);
    await expect.poll(() => fullNow(page), { message: 'a double-click: full screen' }).toBe('focus-viewer[data-full]');
    const at = await letterboxedFull(page, 16 / 9, '16:9');
    await page.mouse.dblclick(at.media.x + at.media.width / 2, at.media.y + at.media.height / 2);
    await expect.poll(() => fullNow(page), { message: 'another: out' }).toBeNull();
    expect.soft(await playing(held), 'paused as it was').toMatchObject({ connected: true, paused: true });
  });

  await test.step('a 9:16 video: its own frame in Focus, and full screen as tall as the screen, its transport across it', async () => {
    await held.dispose();
    await focus(page, 'tall');
    await ownFrame('tall', [720, 1280]);
    await page.keyboard.press('f');
    await expect.poll(() => fullNow(page), { message: 'F: full screen' }).toBe('focus-viewer[data-full]');
    const at = await letterboxedFull(page, 9 / 16, '9:16');
    expect.soft(Math.abs(at.media.height - at.screen.height), 'as tall as the screen, px').toBeLessThan(2);
    expect.soft([Math.round(at.under.x), Math.round(at.under.width)], 'its transport across the screen').toEqual([0, at.screen.width]);
    expect.soft((await seen(page)).blurs, 'nothing blurs it').toEqual([]);
    await screen.getByRole('button', { name: 'Exit full screen', exact: true }).click();
    await expect.poll(() => fullNow(page), { message: 'Exit full screen: out' }).toBeNull();
  });

  await test.step("a take's track goes full screen with it: its segments over the picture, and a segment seeks", async () => {
    await focus(page, 'shot');
    await region.getByRole('button', { name: 'Render', exact: true }).click();
    await expect.poll(async () => (await project.asset('shot'))?.prefixes?.length ?? 0, { message: 'its take', timeout: 240_000 }).toBe(1);
    await region.locator('.here-side .take[data-take="1"]').click();
    await ownFrame('shot', [160, 90]);
    await expect(region.locator('.tl-take .segment'), 'two segments under the frame').toHaveCount(2);
    await page.keyboard.press('f');
    await expect.poll(() => fullNow(page), { message: 'F: full screen' }).toBe('focus-viewer[data-full]');
    const blocks = screen.locator('.tl-take .segment');
    await expect.soft(blocks, 'its two segments in full screen').toHaveCount(2);
    await expect.soft(page.locator('.tl-take'), 'one track: none left under the frame').toHaveCount(1);
    const at = await letterboxedFull(page, 16 / 9, '16:9');
    const track = await screen.locator('.tl-take').boundingBox();
    expect.soft(track.y, 'its track over the picture').toBeGreaterThan(at.media.y);
    expect.soft(track.y + track.height, 'above its transport').toBeLessThanOrEqual((await screen.locator('.tl-transport').boundingBox()).y + 1);
    const video = region.locator('video.live');
    await blocks.nth(1).click();
    await expect.poll(() => video.evaluate((el) => el.currentTime), { message: 'segment 2 seeks to its start' }).toBeCloseTo(BEAT_S, 1);
    await page.screenshot({ path: join(shots, 'shot-1600x1000-paper-full.png') });
    await page.keyboard.press('Escape');
    await expect.poll(() => fullNow(page), { message: 'Esc: out' }).toBeNull();
    await expect(region.locator('.tl-take .segment'), 'its track back under the frame').toHaveCount(2);
  });

  await test.step('an orbit turns by drag in full screen as in the box', async () => {
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await focus(page, 'spin');
    const orbit = region.getByRole('slider', { name: 'Turn', exact: true });
    await expect(region.locator('.orbit.ready')).toBeVisible();
    await region.locator('.orbit-bar').getByRole('button', { name: 'Full screen', exact: true }).click();
    await expect.poll(() => fullNow(page), { message: "the orbit's Full screen" }).toBe('focus-viewer[data-full]');
    const at = await letterboxedFull(page, 16 / 9, '16:9');
    const turn = await orbit.boundingBox();
    const y = turn.y + turn.height / 2;
    await page.mouse.move(turn.x + turn.width * 0.25, y);
    await page.mouse.down();
    await page.mouse.move(turn.x + turn.width * 0.75, y, { steps: 12 });
    await page.mouse.up();
    await expect.poll(async () => Number(await orbit.getAttribute('data-angle')), { message: 'half its width turns it half round' }).toBeCloseTo(180, -1);
    await page.screenshot({ path: join(shots, 'orbit-1600x1000-paper-full.png') });
    await page.keyboard.press('Escape');
    await expect.poll(() => fullNow(page), { message: 'Esc: out' }).toBeNull();
    await expect(region).toHaveAttribute('data-focused-asset', 'spin');
    await page.emulateMedia({ reducedMotion: null });
  });

  await test.step('to look at: in and out of full screen at 1440×900 and 1920×1080, paper and dark, 16:9 and 9:16', async () => {
    for (const theme of ['paper', 'dark']) {
      if (await region.count()) await leave(page);
      await project.look(theme);
      await page.reload();
      await expect(page.locator('.stage')).toBeVisible();
      await themed(page, theme);
      for (const [width, height] of [[1440, 900], [1920, 1080]]) {
        if (await region.count()) await leave(page);
        await page.setViewportSize({ width, height });
        await canvasIdle(page);
        await fit(page);
        for (const [name, size] of [['wide', [1280, 720]], ['tall', [720, 1280]]]) {
          await focus(page, name);
          await ownFrame(name, size);
          await page.screenshot({ path: join(shots, `${name}-${width}x${height}-${theme}.png`) });
          await transport.getByRole('button', { name: 'Full screen', exact: true }).click();
          await expect.poll(() => fullNow(page)).toBe('focus-viewer[data-full]');
          await expect(screen.locator('.tl-transport')).toBeVisible();
          await page.screenshot({ path: join(shots, `${name}-${width}x${height}-${theme}-full.png`) });
          await page.keyboard.press('Escape');
          await expect.poll(() => fullNow(page)).toBeNull();
        }
      }
    }
    await leave(page);
    await page.setViewportSize({ width: 1600, height: 1000 });
  });
});
