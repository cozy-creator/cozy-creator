// upload (cl-973 core cell 13): files from the computer. A PNG, an MP4 and a WAV dropped together are each a box of its kind at the drop,
// with its thumbnail, poster or waveform; Add files adds one; a .txt dropped with a PNG is refused in one line and the PNG lands; the same
// PNG twice is two boxes and one file. Twenty pictures dropped at once (mixed shapes, two with alpha, camera JPEGs turned by their EXIF):
// each is first drawn where and as it stays, its picture decoded, and nothing moves, resizes or swaps, for 5 s or after a reload, a JPEG
// standing upright. Esc while a dropped file is being named leaves nothing. No hash shows on the page.
const { createHash } = require('crypto');
const { readdirSync } = require('fs');
const { join } = require('path');
const { test, expect, canvasIdle, MP4, photo, png, wav, canvasCard, canvasNames, canvasVisible, screenshotPixels } = require('./fixtures');
const { dropFiles } = require('./hands');

const hex = (bytes) => createHash('sha256').update(bytes).digest('hex');
const plain = (sha256) => (sha256 ?? '').replace(/^sha256:/, '');

/** A camera's JPEG with an EXIF Orientation of `turn`, as a phone writes it: photo()'s 96×64, stored as shot. */
function turned(turn) {
  const tiff = Buffer.from([0x4d, 0x4d, 0x00, 0x2a, 0x00, 0x00, 0x00, 0x08, 0x00, 0x01, 0x01, 0x12, 0x00, 0x03, 0x00, 0x00, 0x00, 0x01, 0x00, turn, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
  const app1 = Buffer.concat([Buffer.from([0xff, 0xe1, 0x00, 2 + 6 + tiff.length]), Buffer.from('Exif\0\0', 'binary'), tiff]);
  const bytes = photo();
  return Buffer.concat([bytes.subarray(0, 2), app1, bytes.subarray(2)]);
}

// Twenty, each with the shape it's drawn at: PNGs from 1:2.5 to 3:1, two with alpha, and JPEGs turned 1, 3, 6, 8 and 5.
const TWENTY = [
  ...[[320, 200], [200, 320], [640, 480], [300, 300], [900, 300], [300, 750], [800, 450], [256, 512], [1024, 576], [480, 640], [500, 200], [210, 297], [333, 222, true], [128, 128, true], [1200, 900]]
    .map(([w, h, alpha], i) => ({ name: `p${String(i + 1).padStart(2, '0')}`, file: `p${String(i + 1).padStart(2, '0')}.png`, type: 'image/png', bytes: png(20 + i * 12, w, h, Boolean(alpha)), aspect: w / h })),
  ...[1, 3, 6, 8, 5].map((turn, i) => ({ name: `p${16 + i}`, file: `p${16 + i}.jpg`, type: 'image/jpeg', bytes: turned(turn), aspect: turn >= 5 ? 64 / 96 : 96 / 64, turn })),
];

/** Every box of `names` as it's drawn, by name: its card's rect and its picture's. */
const drawn = (page, names) => page.evaluate((names) => {
  // Until a reload's canvas is mounted there is nothing drawn yet, not an error: the poll asks again.
  const scene = document.querySelector('.canvas-root')?.canvasInspection;
  if (!scene) return {};
  return Object.fromEntries(names.flatMap(name => {
    const card = scene.card(name), shown = card?.rendered;
    if (!card) return [];
    const native = document.querySelector(`.canvas-node:has(.card[data-asset=${JSON.stringify(name)}])`);
    if (native?.checkVisibility()) {
      const at = native.getBoundingClientRect(), media = native.querySelector('.media')?.getBoundingClientRect(), img = native.querySelector('.media-pixels img');
      return [[name, { x: at.x, y: at.y, w: at.width, h: at.height, media: media ? media.width / media.height : null, picture: img?.complete && img.naturalWidth ? img.naturalWidth / img.naturalHeight : null }]];
    }
    if (!shown) return [];
    const view = scene.camera(), pane = document.querySelector('.canvas-root').getBoundingClientRect();
    const at = { x: pane.x + view.x + shown.transform.x * view.zoom, y: pane.y + view.y + shown.transform.y * view.zoom, width: card.bounds.width * shown.transform.scaleX, height: card.bounds.height * shown.transform.scaleY }, media = shown.body;
    return [[name, { x: at.x, y: at.y, w: at.width, h: at.height, media: media.width / media.height, picture: shown.media.ready ? shown.media.width / shown.media.height : null }]];
  }));
}, names);

/** A point on open canvas with a `w`×`h` room under it and to its right, clear of every box and of what floats over the canvas. */
const room = (page, w, h) => page.evaluate(({ w, h }) => {
  const open = (x, y) => document.elementFromPoint(x, y)?.classList.contains('canvas-pane') && !document.querySelector('.canvas-root')?.canvasInspection?.hitTest({ x, y });
  for (let y = 120; y + h < innerHeight - 20; y += 40) {
    for (let x = 340; x + w < innerWidth - 20; x += 40) {
      let clear = true;
      for (let dy = -40; clear && dy <= h; dy += 40) for (let dx = -40; clear && dx <= w; dx += 40) clear = open(x + dx, y + dy);
      if (clear) return { x, y };
    }
  }
  throw new Error(`no open room of ${w}x${h} on the canvas`);
}, { w, h });

/** Every frame from now, each box of `names`: where and how big it's drawn, its picture and its head, from the frame it's first drawn. */
const watch = (page, names) => page.evaluate((names) => {
  const seen = (window.dropWatch = { first: {}, changes: [], bad: [], stop: false });
  const sample = (name) => {
    const scene = document.querySelector('.canvas-root')?.canvasInspection, card = scene?.card(name), shown = card?.rendered;
    if (!card) return null;
    const zoom = scene.camera().zoom;
    const native = document.querySelector(`.canvas-node:has(.card[data-asset=${JSON.stringify(name)}])`);
    if (native?.checkVisibility()) {
      const root = document.querySelector('.canvas-root').getBoundingClientRect(), view = scene.camera(), r = native.getBoundingClientRect();
      const media = native.querySelector('.media'), body = media?.getBoundingClientRect(), img = media?.querySelector('.media-pixels img');
      const host = img?.closest('.buffered-image'), src = img?.getAttribute('src') ?? '', sha = host?.dataset.displayedSha?.replace(/^sha256:/, '');
      return { pos: [(r.x - root.x - view.x) / zoom, (r.y - root.y - view.y) / zoom], w: r.width / zoom, h: r.height / zoom, mw: body?.width / zoom, mh: body?.height / zoom, srcs: src.startsWith('blob:') ? 'local' : 'remote', local: src.startsWith('blob:'), sha, decoded: Boolean(img?.complete && img.naturalWidth > 0), head: native.querySelector('.head .nm')?.textContent, wrong: [native.classList.contains('glide') && 'glide', native.querySelector('.card.landing') && 'landing', native.querySelector('.prog') && 'progress', native.querySelector('.state-dot.queued, .state-dot.running') && 'state dot'].filter(Boolean) };
    }
    if (!shown) return null;
    return {
      pos: [shown.transform.x, shown.transform.y],
      w: card.bounds.width / zoom * shown.transform.scaleX, h: card.bounds.height / zoom * shown.transform.scaleY,
      mw: shown.body.width / zoom, mh: shown.body.height / zoom,
      srcs: shown.media.local ? 'local' : 'remote', local: shown.media.local, sha: shown.media.sha,
      decoded: shown.media.ready, head: shown.labels.title?.fullText,
      wrong: [shown.glide?.active && 'glide', shown.landing?.active && 'landing', shown.progress && 'progress', ['queued', 'running'].includes(shown.state) && 'state dot'].filter(Boolean),
    };
  };
  const same = (a, b) => a.pos.every((value, i) => Math.abs(value - b.pos[i]) < 0.01)
    && ['w', 'h', 'mw', 'mh'].every(side => Math.abs(a[side] - b[side]) <= 0.5) && a.srcs === b.srcs && (!a.sha || !b.sha || a.sha === b.sha) && a.head === b.head;
  const tick = () => {
    if (seen.stop) return;
    const when = Math.round(performance.now());
    for (const name of names) {
      const now = sample(name);
      if (!now) continue;
      const first = seen.first[name];
      if (!first) seen.first[name] = { ...now, at: when };
      else if (!same(first, now) && seen.changes.length < 40) seen.changes.push({ name, at: when, first, now });
      if (now.wrong.length && seen.bad.length < 40) seen.bad.push({ name, at: when, wrong: now.wrong });
    }
    requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
}, names);

test.use({ viewport: { width: 1920, height: 1200 } });

test('upload', { tag: '@engines' }, async ({ page, project, guards }) => {
  // The .txt's import, refused on purpose.
  guards.allow(/status of 400\b.*\/v1\/projects\/[^/ ]+\/imports$/);
  const assets = async () => Object.fromEntries((await project.assets()).map((asset) => [asset.name, asset]));
  const coast = png(60);
  /** Where each box stands on the canvas, by name: its node's place in canvas units, and its card's size. */
  const places = () => page.evaluate(() => {
    const root = document.querySelector('.canvas-root'), scene = root?.canvasInspection;
    if (!scene) return {};
    const view = scene.camera(), pane = root.getBoundingClientRect();
    const fixed = value => Math.round(value * 1000) / 1000;
    return Object.fromEntries(scene.names().flatMap(name => {
      const card = scene.card(name), shown = card?.rendered, native = document.querySelector(`.canvas-node:has(.card[data-asset=${JSON.stringify(name)}])`);
      const r = native?.checkVisibility() ? native.getBoundingClientRect() : null;
      if (!shown && !r) return [];
      const at = r ? [(r.x - pane.x - view.x) / view.zoom, (r.y - pane.y - view.y) / view.zoom, r.width / view.zoom, r.height / view.zoom] : [shown.transform.x, shown.transform.y, card.bounds.width / view.zoom * shown.transform.scaleX, card.bounds.height / view.zoom * shown.transform.scaleY];
      return [[name, at.map(fixed)]];
    }));
  });

  await test.step('a PNG, an MP4 and a WAV dropped together, Add files, a .txt beside a PNG, and the same PNG twice', async () => {
    // Two across and two down, as the page lays out a drop of three.
    const spot = await room(page, 600, 560);
    const at = { x: spot.x + 124, y: spot.y + 20 };
    await dropFiles(page, [{ name: 'Coast.png', type: 'image/png', bytes: coast }, { name: 'Take.mp4', type: 'video/mp4', bytes: MP4 }, { name: 'Sea.wav', type: 'audio/wav', bytes: wav() }], at);
    for (const name of ['coast', 'take', 'sea']) await expect.poll(() => canvasVisible(page, name)).toBe(true);
    await expect.soft(page.locator('.nm-field'), 'several dropped: none asks its name').toHaveCount(0);
    const first = (await canvasCard(page, 'coast')).bounds;
    expect.soft(first.x <= at.x && at.x <= first.x + first.width && first.y <= at.y && at.y <= first.y + first.height, `the first where it was let go: ${JSON.stringify(first)}`).toBe(true);
    await expect.poll(async () => {
      const held = await assets();
      return ['coast', 'take', 'sea'].map((name) => held[name]?.kind);
    }, { message: 'each a box of its kind' }).toEqual(['image', 'video', 'audio']);
    for (const name of ['coast', 'take', 'sea']) {
      await expect.poll(async () => (await canvasCard(page, name))?.rendered?.media.ready ?? false, { message: `${name}'s picture, poster or waveform`, timeout: 60_000 }).toBe(true);
    }

    const chooser = page.waitForEvent('filechooser');
    await page.getByRole('button', { name: 'Add files', exact: true }).click();
    const chosen = await chooser;
    expect.soft(await chosen.element().getAttribute('accept'), 'it takes media').toBe('image/*,video/*,audio/*');
    await chosen.setFiles({ name: 'Hero.png', mimeType: 'image/png', buffer: png(120) });
    const name = page.getByRole('textbox', { name: 'Name', exact: true });
    await expect(name, 'one file asks its name, its stem chosen').toBeFocused();
    await expect.soft(name).toHaveValue('hero');
    await name.press('Enter');
    await expect.poll(async () => (await assets()).hero?.kind, { message: 'Add files adds one' }).toBe('image');

    const filesBefore = readdirSync(join(project.path, 'files')).length;
    const clear = await room(page, 600, 300);
    const beside = { x: clear.x + 124, y: clear.y + 20 };
    await dropFiles(page, [{ name: 'notes.txt', type: 'text/plain', bytes: Buffer.from('these are not pictures\n'.repeat(4)) }, { name: 'Coast.png', type: 'image/png', bytes: coast }], beside);
    await expect.poll(async () => Object.values(await assets()).filter((asset) => plain(asset.sha256) === hex(coast)).map((asset) => asset.name).sort(), { message: 'the same PNG twice: two boxes' }).toHaveLength(2);
    await expect.poll(() => page.locator('.notes .note .line').allTextContents(), { message: 'the .txt refused in one line' }).toEqual(expect.arrayContaining([expect.stringContaining('notes.txt')]));
    expect.soft((await page.locator('.notes .note .line').allTextContents()).filter((line) => line.includes('notes.txt')), 'one line').toHaveLength(1);
    await expect.configure({ soft: true }).poll(async () => (await canvasNames(page)).includes('notes'), { message: 'nothing of it on the canvas' }).toBe(false);
    expect.soft(Object.keys(await assets()), 'nor in the project').not.toContain('notes');
    expect.soft(readdirSync(join(project.path, 'files')).length, 'the same PNG twice: one file').toBe(filesBefore);
  });

  const pictures = TWENTY.map((one) => one.name);
  let placed;
  await test.step('twenty pictures at once, each drawn where and as it stays', async () => {
    await page.getByRole('button', { name: /more zoom options$/ }).click();
    await page.getByRole('menuitem', { name: /^Zoom to 50%/ }).click();
    await canvasIdle(page);
    await page.getByRole('button', { name: 'Zoom out', exact: true }).click();
    await canvasIdle(page);
    await expect(page.getByRole('button', { name: /more zoom options$/ })).toHaveText('33%');
    // Five across, four down, as the page lays a drop of twenty out: room for them all, clear of the boxes there.
    const zoom = 0.33;
    const spot = await room(page, Math.ceil(5 * 272 * zoom) + 40, Math.ceil(2300 * zoom) + 40);
    const at = { x: spot.x + Math.ceil(120 * zoom) + 4, y: spot.y + Math.ceil(16 * zoom) + 4 };
    await watch(page, pictures);
    await dropFiles(page, TWENTY.map(({ file, type, bytes }) => ({ name: file, type, bytes })), at);
    await expect.poll(async () => {
      const held = await assets();
      return pictures.filter((name) => !held[name]);
    }, { message: 'all twenty in the project', timeout: 60_000 }).toEqual([]);
    // Calcifer measures each after its import: that read comes back to the page too, and changes nothing on it.
    const dims = (row) => row.prefixes?.find((version) => version.number === 1)?.main ?? row.prefixes?.[0]?.main ?? row;
    await expect.poll(async () => {
      const held = await assets();
      return pictures.filter((name) => !(dims(held[name] ?? {}).width && dims(held[name] ?? {}).height));
    }, { message: 'all twenty measured', timeout: 60_000 }).toEqual([]);
    const measured = await assets();
    // The one declared window: 5 s more, every frame, then what the watch saw.
    const seen = await page.evaluate((ms) => new Promise((done) => setTimeout(() => {
      window.dropWatch.stop = true;
      done(window.dropWatch);
    }, ms)), 5000);
    const firsts = Object.entries(seen.first);
    expect.soft(firsts.map(([name]) => name).sort(), 'each drawn').toEqual([...pictures].sort());
    expect.soft(firsts.filter(([, one]) => !one.decoded).map(([name]) => name), 'first drawn without its picture decoded').toEqual([]);
    expect.soft(firsts.filter(([name, one]) => !one.local || (one.sha && one.sha !== hex(TWENTY.find(file => file.name === name).bytes))).map(([name, one]) => `${name}: ${one.srcs}`), 'first drawn with other than its own file').toEqual([]);
    expect.soft(seen.changes.slice(0, 6), 'moved, resized or swapped after it was drawn').toEqual([]);
    expect.soft(seen.bad.slice(0, 6), 'a progress bar, a pulsing dot, a landing or a glide').toEqual([]);
    const shapes = TWENTY.map(({ name, aspect }) => ({ name, aspect, drawn: seen.first[name] && seen.first[name].mw / seen.first[name].mh }));
    expect.soft(shapes.filter(({ aspect, drawn: shown }) => !(shown && Math.abs(shown / aspect - 1) < 0.02)), "drawn at other than its picture's shape").toEqual([]);
    const turnedRows = TWENTY.filter(({ turn }) => turn).map(({ name, aspect }) => ({ name, aspect, measured: dims(measured[name]).width / dims(measured[name]).height }));
    expect.soft(turnedRows.filter(({ aspect, measured: ratio }) => !(Math.abs(ratio / aspect - 1) < 0.02)), 'a turned JPEG measured other than as it stands').toEqual([]);
    const shot = screenshotPixels(await page.screenshot({ scale: 'css' }));
    for (const [index, file] of TWENTY.entries()) {
      const body = (await canvasCard(page, file.name)).body;
      const pixels = [[0.65, 0.25], [0.85, 0.25], [0.65, 0.75], [0.85, 0.75]].map(([u, v]) => ({ u, v, rgb: shot(body.x + body.width * u, body.y + body.height * v) }));
      if (file.type === 'image/png') for (const { u, v, rgb } of pixels) {
        const expected = [20 + index * 12, 255 * u, 255 * v];
        expect.soft(Math.max(...rgb.map((value, i) => Math.abs(value - expected[i]))), `${file.name} source pixels ${u},${v}`).toBeLessThanOrEqual(32);
      } else {
        expect.soft(new Set(pixels.map(({ rgb }) => rgb.map(v => Math.round(v / 8)).join())).size, `${file.name} JPEG paints a nonuniform picture`).toBeGreaterThan(1);
      }
    }
    placed = await drawn(page, pictures);
  });

  await test.step('Esc while a dropped file is named leaves nothing; a reload keeps every box as it was', async () => {
    const before = Object.keys(await assets()).sort();
    const spot = await room(page, 120, 120);
    await dropFiles(page, [{ name: 'Never.png', type: 'image/png', bytes: png(200) }], { x: spot.x + 60, y: spot.y + 20 });
    const name = page.getByRole('textbox', { name: 'Name', exact: true });
    await expect(name).toBeFocused();
    await page.keyboard.press('Escape');
    await expect.poll(async () => (await canvasNames(page)).includes('never'), { message: 'gone from the canvas' }).toBe(false);
    await expect.poll(async () => Object.keys(await assets()).sort(), { message: 'nothing in the project' }).toEqual(before);
    const stood = await places();
    expect.soft(Object.keys(stood).sort(), 'every box drawn').toEqual(before);
    await page.reload();
    await expect.poll(async () => Object.keys(await places()).sort(), { message: 'every box back' }).toEqual(before);
    await expect.poll(async () => Object.values(await drawn(page, pictures)).filter((one) => one.picture).length, { message: 'each picture back', timeout: 60_000 }).toBe(pictures.length);
    expect.soft(await places(), 'every box where it was, its size').toEqual(stood);
    const after = await drawn(page, pictures);
    expect.soft(pictures.filter((one) => !(after[one] && Math.abs(after[one].w - placed[one].w) <= 1 && Math.abs(after[one].h - placed[one].h) <= 1)), 'another size after the reload').toEqual([]);
    expect.soft(pictures.filter((one) => !(after[one] && Math.abs(after[one].picture / after[one].media - 1) < 0.03)), 'a picture not upright in its box').toEqual([]);
  });

  await test.step('no hash shows on the page', async () => {
    const digests = Object.values(await assets()).map((asset) => plain(asset.sha256)).filter(Boolean);
    const shown = await page.evaluate(() => document.body.innerText + [...document.querySelectorAll('[title],[aria-label]')].map((el) => `${el.title} ${el.ariaLabel}`).join(' ') + (() => { const scene = document.querySelector('.canvas-root').canvasInspection; return scene.names().flatMap(name => Object.values(scene.card(name)?.rendered?.labels ?? {}).map(label => label.text)).join(' '); })());
    expect.soft(digests.length, 'files held').toBeGreaterThan(20);
    expect.soft(digests.filter((digest) => shown.includes(digest)), 'a hash on the page').toEqual([]);
  });
});
