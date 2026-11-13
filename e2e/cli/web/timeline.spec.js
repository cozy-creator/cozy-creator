// timeline (cl-973 core cell 20): the video editor (cl-957). A Video Timeline box made from the palette takes three clips: a box let go on
// it from the canvas, and two files let go on its timeline. Then a reorder, a trim, a split at the playhead and a piece deleted, then Ctrl+Z.
// A full-size player sits over the timeline and shows each change at once; each change is joined, free, into a version whose film has the
// cut's frames and its sound (ffprobe); Undo restores the cut, and its `.otio` reads back as it is with OpenTimelineIO's own reader.
const { spawnSync } = require('child_process');
const { readFileSync } = require('fs');
const { join } = require('path');
const { test, expect, box, focus, middle, openPoint, png } = require('./fixtures');
const { carry, carryBox, ffmpeg, fit, toolRow } = require('./hands');

const FPS = 24;
// A cut joined a moment after it stops changing; the first also fetches what joins it.
const JOIN_MS = 120_000;
const RED = [255, 0, 0], BLUE = [0, 0, 255], GREEN = [0, 255, 0];
const looksLike = (pixel, want) => Array.isArray(pixel) && pixel.every((value, i) => Math.abs(value - want[i]) < 48);

/** A clip of one colour under one tone, `seconds` long, as a person's file is. */
function plate(path, color, seconds, hz) {
  ffmpeg('ffmpeg')('-v', 'error', '-y', '-f', 'lavfi', '-i', `color=c=${color}:size=320x180:rate=${FPS}:duration=${seconds}`, '-f', 'lavfi', '-i', `sine=frequency=${hz}:sample_rate=32000:duration=${seconds}`,
    '-c:v', 'libx264', '-preset', 'veryfast', '-pix_fmt', 'yuv420p', '-g', String(FPS), '-c:a', 'aac', '-shortest', path);
  return readFileSync(path);
}

/** A film's frames, each stream's length, and its middle pixel at `frames` (rgb). */
function film(path, frames = []) {
  const probe = (...args) => ffmpeg('ffprobe')('-v', 'error', ...args, path).trim();
  const seconds = (kind) => Number(probe('-select_streams', `${kind}:0`, '-show_entries', 'stream=duration', '-of', 'csv=p=0'));
  const pixel = (frame) => [...ffmpeg('ffmpeg')('-v', 'error', '-i', path, '-vf', `select=eq(n\\,${frame}),crop=2:2:iw/2:ih/2,scale=1:1`, '-frames:v', '1', '-f', 'rawvideo', '-pix_fmt', 'rgb24', '-')];
  return {
    frames: Number(probe('-count_frames', '-select_streams', 'v:0', '-show_entries', 'stream=nb_read_frames', '-of', 'csv=p=0')),
    picture: seconds('v'),
    sound: seconds('a'),
    pixels: frames.map(pixel),
  };
}

// The size the editor was laid out and measured at (web/timeline.js): its player beside its versions, full size over the timeline.
test.use({ viewport: { width: 1900, height: 1000 } });

test('timeline', async ({ page, project }) => {
  const red = plate(test.info().outputPath('red.mp4'), 'red', 2, 440);
  const blue = plate(test.info().outputPath('blue.mp4'), 'blue', 3, 880);
  const title = png(0, 400, 400, false, () => GREEN);
  await project.import(red, { name: 'red', at: { x: 0, y: 0 } });
  await expect(box(page, 'red')).toBeVisible();
  await fit(page);
  const region = page.getByRole('region', { name: 'Focused asset' });
  const clipsOf = () => region.locator('.tl-clip').evaluateAll((all) => all.map((el) => ({ name: el.dataset.name, in: Number(el.dataset.in), out: Number(el.dataset.out), frames: Number(el.dataset.frames) })));
  const order = async () => (await clipsOf()).map((clip) => clip.name).join(' ');
  let edit;
  const versions = async () => (await project.asset(edit))?.prefixes ?? [];

  const COLOURS = { red: RED, blue: BLUE, title: GREEN };
  const facts = new Map();
  let newest = 0;
  /** The version this cut was joined into, once there is one: its film as many frames as the timeline holds, the middle of each clip its
   *  colour, its sound as long as its picture. `fresh`: a version of its own, newer than any before (the same film again is the version
   *  that holds it). */
  const joined = async (what, fresh = true) => {
    const cut = await clipsOf();
    const frames = cut.reduce((sum, clip) => sum + clip.frames, 0);
    const middles = [];
    for (let at = 0, i = 0; i < cut.length; at += cut[i].frames, i++) middles.push(at + Math.floor(cut[i].frames / 2));
    let made;
    await expect.poll(async () => {
      for (const version of [...(await versions())].reverse()) {
        const main = version.main;
        if (!main?.path || main.absent) continue;
        const key = `${main.sha256} ${middles}`;
        if (!facts.has(key)) {
          // A film still being written can't be read yet: read again on the next look.
          try {
            facts.set(key, film(join(project.path, main.path), middles));
          } catch {
            continue;
          }
        }
        const held = facts.get(key);
        if (held.frames === frames && held.pixels.every((pixel, i) => looksLike(pixel, COLOURS[cut[i].name]))) {
          made = { number: version.number, ...held };
          return version.number;
        }
      }
      return null;
    }, { message: `${what}: a version of the cut (${cut.map((clip) => clip.name).join(' ')}, ${frames} frames)`, timeout: JOIN_MS }).not.toBeNull();
    if (fresh) expect.soft(made.number, `${what}: a version of its own`).toBeGreaterThan(newest);
    newest = Math.max(newest, ...(await versions()).map((version) => version.number));
    expect.soft(made.sound, `${what}: its sound`).toBeGreaterThan(0);
    expect.soft(Math.abs(made.sound - made.picture), `${what}: its sound as long as its picture, s`).toBeLessThan(0.1);
    return cut;
  };

  /** Pixels a frame, as the ruler draws them. */
  const scale = async () => (await region.locator('.tl-ruler').boundingBox()).width / (await clipsOf()).reduce((sum, clip) => sum + clip.frames, 0);
  const seek = async (frame) => {
    const rail = await region.locator('.tl-rail').boundingBox();
    await page.mouse.click(rail.x + (frame + 0.5) * (await scale()), rail.y + rail.height / 2);
    await expect(region.locator('.tl-playhead')).toHaveAttribute('data-frame', String(frame));
  };
  /** What the player shows: its clip and its middle pixel. */
  const showing = () => page.evaluate(() => {
    const monitor = document.querySelector('.focus-viewport .tl-monitor');
    const visible = (el) => Boolean(el && getComputedStyle(el).visibility !== 'hidden' && el.getBoundingClientRect().width > 0);
    const sample = (source) => {
      const copy = document.createElement('canvas');
      copy.width = 1;
      copy.height = 1;
      const [w, h] = [source.videoWidth || source.naturalWidth || source.width, source.videoHeight || source.naturalHeight || source.height];
      copy.getContext('2d').drawImage(source, w / 2 - 1, h / 2 - 1, 2, 2, 0, 0, 1, 1);
      return [...copy.getContext('2d').getImageData(0, 0, 1, 1).data].slice(0, 3);
    };
    const video = monitor.querySelector('video');
    const canvas = monitor.querySelector('canvas');
    const image = [...monitor.querySelectorAll('img')].find((img) => img.complete && img.naturalWidth && visible(img));
    const ready = video && visible(video.parentElement) && video.readyState >= 2 && !video.seeking;
    const drawn = ready ? video : visible(canvas) && canvas.width ? canvas : image;
    return { clip: monitor.dataset.clip, pixel: drawn ? sample(drawn) : null };
  });
  /** The player at `frame` shows `name`, its colour. */
  const shows = async (frame, name, colour) => {
    await seek(frame);
    await expect.poll(async () => {
      const now = await showing();
      return now.clip === name && looksLike(now.pixel, colour) ? name : JSON.stringify(now);
    }, { message: `the player at frame ${frame} shows ${name}` }).toBe(name);
  };
  /** Files let go on the timeline at its start or its end. */
  const onTimeline = async (file, where) => {
    const lane = await region.locator('.tl-clips').boundingBox();
    const transfer = await page.evaluateHandle((held) => {
      const data = new DataTransfer();
      data.items.add(new File([Uint8Array.from(atob(held.b64), (c) => c.charCodeAt(0))], held.name, { type: held.type }));
      return data;
    }, { name: file.name, type: file.type, b64: file.bytes.toString('base64') });
    const x = where === 'start' ? lane.x + 2 : lane.x + lane.width - 4;
    for (const type of ['dragenter', 'dragover', 'drop']) await page.dispatchEvent('.focus-viewport .tl-clips', type, { dataTransfer: transfer, clientX: x, clientY: lane.y + lane.height / 2 });
  };

  await test.step('a Video Timeline from the palette', async () => {
    const before = new Set((await project.assets()).map((asset) => asset.name));
    await carry(page, await middle(await toolRow(page, 'Video Timeline', 'cozy/edit')), await openPoint(page));
    await expect.poll(async () => (edit = (await project.assets()).find((asset) => asset.tool === 'cozy/edit' && !before.has(asset.name))?.name), { message: 'its box' }).toBeTruthy();
    await expect(box(page, edit)).toBeVisible();
    await expect(page.locator('.card.landing')).toHaveCount(0);
    // Let go where there was room, it may stand partly out of view: both in view to carry one onto the other.
    await fit(page);
  });

  await test.step('three clips: a box let go on it, and two files on its timeline', async () => {
    await carryBox(page, 'red', await middle(box(page, edit).locator('.media')));
    await expect.poll(async () => JSON.stringify((await project.asset(edit)).fields?.timeline ?? null), { message: 'red on its timeline' }).toContain('<red>');
    await focus(page, edit);
    await expect(region.locator('.tl-clip')).toHaveCount(1);
    await onTimeline({ name: 'title.png', type: 'image/png', bytes: title }, 'start');
    await expect.poll(order).toBe('title red');
    await onTimeline({ name: 'blue.mp4', type: 'video/mp4', bytes: blue }, 'end');
    await expect.poll(order, { message: 'each where it was let go' }).toBe('title red blue');
    await expect.poll(async () => (await clipsOf()).map((clip) => clip.frames), { message: 'each as long as it lasts' }).toEqual([3 * FPS, 2 * FPS, 3 * FPS]);
  });

  await test.step('a full-size player over the timeline, its versions beside them', async () => {
    const viewport = page.viewportSize();
    const [player, timeline, side] = [await region.locator('.tl-player').boundingBox(), await region.locator('.tl').boundingBox(), await region.locator('.here-side').boundingBox()];
    expect.soft(player.width, 'the player, px wide').toBeGreaterThanOrEqual(viewport.width * 0.45);
    expect.soft(player.height, 'the player, px high').toBeGreaterThanOrEqual(viewport.height * 0.35);
    expect.soft(timeline.y, 'the timeline under the player').toBeGreaterThanOrEqual(player.y + player.height - 1);
    expect.soft(timeline.width, 'the timeline as wide as the player').toBeGreaterThanOrEqual(player.width * 0.9);
    expect.soft(side.x, 'its versions beside them').toBeGreaterThanOrEqual(timeline.x + timeline.width - 1);
    await expect.soft(region.getByRole('button', { name: 'Render', exact: true }), 'an Edit is joined, never rendered').toHaveCount(0);
    await shows(10, 'title', GREEN);
    await shows(3 * FPS + 10, 'red', RED);
    await joined('three clips');
  });

  await test.step('a reorder: blue first', async () => {
    const lift = await region.locator('.tl-clip[data-name="blue"]').boundingBox();
    const start = await region.locator('.tl-clips').boundingBox();
    await page.mouse.move(lift.x + lift.width / 2, lift.y + 20);
    await page.mouse.down();
    await page.mouse.move(start.x + 4, lift.y + 20, { steps: 12 });
    await page.mouse.up();
    await expect.poll(order).toBe('blue title red');
    await shows(10, 'blue', BLUE);
    await joined('the reorder');
  });

  await test.step("a trim: red's end pulled in, then a frame more by the arrow", async () => {
    // The clips slid into place: an animation cut short ends all the same.
    await region.evaluate((el) => Promise.allSettled(el.querySelector('.tl').getAnimations({ subtree: true }).map((held) => held.finished)));
    const handle = await region.locator('.tl-clip[data-name="red"] .tl-handle.out').boundingBox();
    const by = -Math.round(16 * (await scale()));
    await page.mouse.move(handle.x + handle.width / 2, handle.y + handle.height / 2);
    await page.mouse.down();
    await page.mouse.move(handle.x + handle.width / 2 + by, handle.y + handle.height / 2, { steps: 6 });
    await expect.soft(region.locator('.tl-bubble'), 'where, as it is pulled').toHaveText(/^Out \d\d:\d\d:\d\d:\d\d, \d+ f$/);
    await page.mouse.up();
    const pulled = (await clipsOf())[2].out;
    expect.soft(pulled, "red's end").toBeLessThan(2 * FPS);
    await page.keyboard.press('ArrowLeft');
    await expect.poll(async () => (await clipsOf())[2].out, { message: 'a frame off its end' }).toBe(pulled - 1);
    const cut = await clipsOf();
    await shows(cut[0].frames + cut[1].frames + 5, 'red', RED);
    await joined('the trim');
  });

  await test.step('a split at the playhead, and a piece deleted', async () => {
    await seek(36);
    await page.keyboard.press('s');
    await expect.poll(order).toBe('blue blue title red');
    const halves = await clipsOf();
    expect.soft([halves[0].in, halves[0].out, halves[1].in], 'blue in two at the playhead').toEqual([0, 36, 36]);
    const second = await region.locator('.tl-clip').nth(1).boundingBox();
    await page.mouse.click(second.x + second.width / 2, second.y + 20);
    await region.getByRole('button', { name: 'Delete clip', exact: true }).click();
    await expect.poll(order).toBe('blue title red');
    await expect.soft(region, 'the Edit stays').toHaveAttribute('data-focused-asset', edit);
    await shows(40, 'title', GREEN);
    await joined('the piece deleted');
  });

  await test.step('Ctrl+Z restores the cut, and its .otio is it', async () => {
    await page.keyboard.press('ControlOrMeta+z');
    await expect.poll(order, { message: 'the piece back' }).toBe('blue blue title red');
    await shows(40, 'blue', BLUE);
    // The cut as it was before the split: its film is a version already.
    const cut = await joined('the undo', false);
    const assets = await project.assets();
    const file = (name) => assets.find((asset) => asset.name === name).path;
    const entry = (clip) => (clip.name === 'title' ? `title:0:${clip.frames}:${file('title')}:still` : `${clip.name}:${clip.in}:${clip.out}:${file(clip.name)}`);
    const otio = spawnSync('uv', ['run', '-q', '--no-project', '--with', 'opentimelineio==0.17.0', 'python', '-I', join(__dirname, '..', 'project', 'otio_check.py'), join(project.path, `${edit}.otio`), String(FPS), ...cut.map(entry)], { encoding: 'utf8' });
    expect.soft(`${otio.stdout}${otio.stderr}`.slice(-1500), 'OpenTimelineIO reads it back as the cut').toMatch(/otio PASS/);
    expect.soft(otio.status, 'otio_check.py').toBe(0);
  });
});
