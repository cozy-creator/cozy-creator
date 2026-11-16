// previews (Paul, 11-15: "make sure the new pixijs canvas is robust"): media on the Pixi canvas under load. A video's hover preview stops
// when the canvas pans under it, its poster drawn and nothing left playing, and plays again once the pointer rests on it. Twelve videos at
// once each draw a poster, not a black frame, and none decodes at rest. An 8000×6000 picture is drawn at its shape, the page answering a
// click within a second, and close up its picture is the screen's size, not the file's. A picture whose file is gone pulses while it's
// asked for, then says it's unavailable: no page error, and no asking in a loop (MediaCache: six tries, 2, 4, 8, 16, 32 s apart). The
// SVG with a script is web/svg.spec.js's.
const { readdirSync, readFileSync, rmSync } = require('fs');
const { join } = require('path');
const { test, expect, box, canvasCard, canvasIdle, canvasPoint, canvasVisible, hoverBox, newCanvas, openPoint, png, previewState, previewsPlaying, selectedNames } = require('./fixtures');
const { ffmpeg, fit, posterSpread, rest } = require('./hands');

const CLIPS = 12;
/** THUMB_TRIES (api/thumbs.ts) and the first 10 s of thumbWait's schedule: the asks at 0, 2 and 6 s. */
const TRIES = 6;
const FIRST_TEN_S = 3;

/** A short test pattern, its length `seconds`, so each clip is a file of its own. */
function clip(path, seconds) {
  ffmpeg('ffmpeg')('-v', 'error', '-y', '-f', 'lavfi', '-i', `testsrc2=size=160x90:rate=24:duration=${seconds}`, '-c:v', 'libx264', '-pix_fmt', 'yuv420p', path);
  return readFileSync(path);
}
/** Videos in the page decoding now: playing, a frame at hand. */
const decodingVideos = (page) => page.evaluate(() => [...document.querySelectorAll('video')].filter((video) => !video.paused && !video.ended && video.readyState >= 2).length);
/** Videos playing on the canvas now: the old canvas's decoding `<video>`s; the new canvas's previews (textures, no `<video>` in the page). */
const decoding = async (page) => ((await newCanvas(page)) ? previewsPlaying(page) : decodingVideos(page));
const playing = async (page, name) => (await previewState(page, name)).playing;

test('previews', async ({ page, project }) => {
  const names = Array.from({ length: CLIPS }, (_, i) => `clip_${String(i + 1).padStart(2, '0')}`);
  for (const [i, name] of names.entries()) {
    await project.import(clip(test.info().outputPath(`${name}.mp4`), 2 + i * 0.25), { name, at: { x: (i % 4) * 360, y: Math.floor(i / 4) * 300 } });
  }
  for (const name of names) await expect.poll(() => canvasVisible(page, name)).toBe(true);
  await fit(page);

  await test.step('twelve videos at once: a poster each, none decoding at rest', async () => {
    const rested = await openPoint(page);
    await page.mouse.move(rested.x, rested.y);
    await expect.poll(async () => {
      const cards = await Promise.all(names.map((name) => canvasCard(page, name)));
      return names.filter((name, i) => !cards[i]?.rendered?.media.ready);
    }, { message: 'every poster drawn', timeout: 90_000 }).toEqual([]);
    const black = [];
    for (const name of names) if ((await posterSpread(page, name)) < 12) black.push(name);
    expect.soft(black, 'a poster, not a black frame').toEqual([]);
    expect.soft(await decoding(page), 'no video decoding at rest').toBe(0);
  });

  await test.step('a hover preview stops as the canvas pans under it, its poster drawn; it plays again once the pointer rests', async () => {
    const name = names[5];
    await hoverBox(page, name);
    await rest(page, (await newCanvas(page)) ? (await canvasCard(page, name)).body : box(page, name).locator('.media'));
    await expect.poll(() => playing(page, name), { message: 'its preview plays', timeout: 45_000 }).toBe(true);
    expect.soft(await decoding(page), 'one video decoding, the one under the pointer').toBe(1);
    // The wheel pans while the pointer stays where it was: the box slides from under it (CanvasSurface's wheel stops the hover preview).
    for (let i = 0; i < 6; i++) await page.mouse.wheel(0, 24);
    await canvasIdle(page);
    await expect.poll(() => decoding(page), { message: 'nothing left playing once it has panned' }).toBe(0);
    expect.soft(await previewsPlaying(page), 'no preview left mounted').toBe(0);
    expect.soft((await canvasCard(page, name))?.rendered?.media.ready, 'its poster drawn').toBe(true);
    const back = await canvasPoint(page, name);
    await page.mouse.move(back.x - 12, back.y, { steps: 2 });
    await page.mouse.move(back.x, back.y, { steps: 4 });
    await expect.poll(() => playing(page, name), { message: 'it plays again at rest', timeout: 45_000 }).toBe(true);
    const away = await openPoint(page);
    await page.mouse.move(away.x, away.y);
    await expect.poll(() => decoding(page), { message: 'and stops once the pointer leaves' }).toBe(0);
  });

  await test.step('an 8000×6000 picture: drawn at its shape, the page answering at once; close up, the screen\'s size', async () => {
    const path = test.info().outputPath('huge.png');
    ffmpeg('ffmpeg')('-v', 'error', '-y', '-f', 'lavfi', '-i', 'color=c=0x3366cc:s=8000x6000', '-frames:v', '1', path);
    await project.import(readFileSync(path), { name: 'huge', at: { x: 0, y: 1000 } });
    await project.import(png(90), { name: 'small', at: { x: 420, y: 1000 } });
    for (const name of ['huge', 'small']) await expect.poll(() => canvasVisible(page, name), { timeout: 60_000 }).toBe(true);
    await fit(page);
    await expect.poll(async () => (await canvasCard(page, 'huge'))?.rendered?.media.ready ?? false, { message: 'its picture drawn', timeout: 60_000 }).toBe(true);
    const drawn = (await canvasCard(page, 'huge')).rendered;
    expect.soft(Math.abs(drawn.media.width / drawn.media.height / (4 / 3) - 1), `its picture 4:3: ${drawn.media.width}×${drawn.media.height}`).toBeLessThan(0.02);
    expect.soft(Math.abs(drawn.body.width / drawn.body.height / (4 / 3) - 1), 'its box 4:3').toBeLessThan(0.03);
    const small = await canvasPoint(page, 'small');
    await page.mouse.click(small.x, small.y);
    await expect.poll(() => selectedNames(page), { message: 'a click selects within a second', timeout: 1_000 }).toEqual(['small']);
    const near = await canvasPoint(page, 'huge');
    await page.mouse.click(near.x, near.y);
    await page.keyboard.press('Shift+Digit2');
    await canvasIdle(page);
    await page.mouse.move(4, 600);
    await expect.poll(async () => (await canvasCard(page, 'huge'))?.rendered?.media ?? null, { message: 'close up, the file itself drawn', timeout: 60_000 })
      .toMatchObject({ ready: true, resolution: 'original' });
    const close = (await canvasCard(page, 'huge')).rendered.media;
    expect.soft(close.width, `its picture no wider than asked (${close.requestedWidth} px), not the file's 8000`).toBeLessThanOrEqual(Math.min(4096, close.requestedWidth));
    const clear = await openPoint(page);
    await page.mouse.click(clear.x, clear.y);
    await expect.poll(() => selectedNames(page), { message: 'and still answers a click within a second', timeout: 1_000 }).toEqual([]);
  });
});

test('previews: a picture whose file is gone', async ({ page, project, guards }) => {
  // Made while the page is elsewhere, so no tab ever held its thumbnail (a plain project's are cached as immutable).
  await page.goto('about:blank');
  await project.import(png(70), { name: 'lost', at: { x: 0, y: 0 } });
  await project.import(png(130), { name: 'kept', at: { x: 420, y: 0 } });
  const hex = (await project.asset('lost')).sha256.replace(/^sha256:/, '').toLowerCase();
  const file = readdirSync(join(project.path, 'files')).find((one) => one.startsWith(hex));
  expect(file, 'its file on disk').toBeTruthy();
  rmSync(join(project.path, 'files', file));
  // Its thumbnail asked for and refused on purpose, from here on.
  // Its picture is asked for as a thumbnail, or, drawn larger than one, as its file (Scene.ts's request).
  guards.allow(new RegExp(`status of 4\\d\\d\\b.*\\/(thumbs|files)\\/[^ ]*${hex}`, 'i'));
  const asks = [];
  page.on('request', (request) => { if (/\/(thumbs|files)\//.test(request.url()) && request.url().toLowerCase().includes(hex)) asks.push(Date.now()); });

  await test.step('opened: the other drawn, it pulses while asked for, then says it is unavailable, never asked in a loop', async () => {
    await page.goto(project.url());
    await page.mouse.move(4, 600);
    for (const name of ['lost', 'kept']) await expect.poll(() => canvasVisible(page, name)).toBe(true);
    await expect.poll(async () => (await canvasCard(page, 'kept'))?.rendered?.media.ready ?? false, { message: 'the other picture drawn', timeout: 30_000 }).toBe(true);
    const placeholder = async () => {
      const shown = (await canvasCard(page, 'lost'))?.rendered;
      return Boolean(shown?.loadingPulse?.active || shown?.media.failed || shown?.labels.unavailable || shown?.labels.note);
    };
    // Drawn from the thumbnail Calcifer made when it came in (thumbs outlive their file), or a placeholder while asked for and then said
    // unavailable: either way settled, never asked for in a loop.
    const drawn = async () => Boolean((await canvasCard(page, 'lost'))?.rendered?.media.ready);
    // Calcifer says the file isn't here: the box says where it is kept and the page asks for nothing (the canvas's own rule, cl-1048).
    const kept = async () => /^Kept on /.test((await canvasCard(page, 'lost'))?.rendered?.labels.note?.text ?? '');
    await expect.poll(async () => (await drawn()) || (await placeholder()), { message: 'its picture, or a placeholder while it is asked for' }).toBe(true);
    if (await kept()) expect.soft(asks.length, 'said kept elsewhere, and never asked for').toBe(0);
    else if (!(await drawn())) {
      await expect.poll(() => asks.length, { message: 'asked for' }).toBeGreaterThan(0);
      const first = asks[0];
      await page.waitForTimeout(Math.max(0, first + 10_000 - Date.now()));
      expect.soft(asks.filter((when) => when - first < 10_000).length, `asked at most ${FIRST_TEN_S} times in its first 10 s`).toBeLessThanOrEqual(FIRST_TEN_S);
      await expect.poll(async () => {
        const shown = (await canvasCard(page, 'lost'))?.rendered;
        return Boolean(shown?.media.ready || (shown?.media.failed && shown.labels.unavailable?.text));
      }, { message: 'then it is drawn or says it is unavailable', timeout: 90_000 }).toBe(true);
    }
    const settled = asks.length;
    await page.waitForTimeout(5_000);
    expect.soft(asks.length, 'not asked again once it said so').toBe(settled);
    expect.soft(settled, `asked at most ${TRIES} times in all`).toBeLessThanOrEqual(TRIES);
  });
});
