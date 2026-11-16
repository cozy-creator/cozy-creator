// steady (Paul, 11-15: video boxes jumped about on the canvas; "positions move only when the user moves them"): three video boxes and one
// rendering a video (project/segments.py). Each box's picture stays exactly where it is on the canvas, its size too, as each is hovered and
// left, as the render's take lands, and as the canvas is panned: in canvas units, to half a pixel, in every frame the camera is still.
const { randomUUID } = require('crypto');
const { readFileSync } = require('fs');
const { test, expect, canvasIdle, canvasVisible, fixtureTool, hoverBox, openPoint } = require('./fixtures');
const { camera } = require('./geometry');
const { ffmpeg, fit } = require('./hands');

const SEGMENTS = fixtureTool('segments.py/segments');
const CLIPS = ['clip_a', 'clip_b', 'clip_c'];
const ALL = [...CLIPS, 'shot'];
const RUN_MS = 120_000;

/** `seconds` of a test pattern, `hue` turning it, as a person's clip is. */
function clip(path, hue) {
  ffmpeg('ffmpeg')('-v', 'error', '-y', '-f', 'lavfi', '-i', `testsrc2=size=320x180:rate=24:duration=3`, '-vf', `hue=h=${hue}`, '-c:v', 'libx264', '-pix_fmt', 'yuv420p', path);
  return readFileSync(path);
}

/** Each box's picture as drawn, in canvas units: the mounted card's, else the scene's. Run in the page. */
function bodies(names) {
  const root = document.querySelector('.canvas-root'), inspection = root?.canvasInspection;
  if (!inspection) return null;
  const cam = inspection.camera(), pane = root.getBoundingClientRect(), r = (v) => Math.round(v * 100) / 100;
  return Object.fromEntries(names.map((name) => {
    const native = document.querySelector(`.canvas-node:has(.card[data-asset=${JSON.stringify(name)}])`);
    const media = native?.checkVisibility() ? native.querySelector('.media')?.getBoundingClientRect() : null;
    const card = inspection.card(name), b = media ?? card?.rendered?.body;
    return [name, b ? { x: r((b.x - pane.left - cam.x) / cam.zoom), y: r((b.y - pane.top - cam.y) / cam.zoom), w: r(b.width / cam.zoom), h: r(b.height / cam.zoom), native: Boolean(media) } : null];
  }));
}

/** What moved of `now` from `base`, by more than half a canvas pixel; `free` names a box whose height may change (a first take's shape). */
function moved(base, now, free = []) {
  return Object.keys(base).flatMap((name) => {
    const [a, b] = [base[name], now?.[name]];
    if (!a || !b) return [`${name}: ${b ? 'was not drawn' : 'not drawn'}`];
    const keys = free.includes(name) ? ['x', 'y', 'w'] : ['x', 'y', 'w', 'h'];
    const off = keys.filter((key) => Math.abs(a[key] - b[key]) > 0.5);
    return off.length ? [`${name}${b.native ? ' (its card)' : ''}: ${off.map((key) => `${key} ${a[key]}→${b[key]}`).join(', ')}`] : [];
  });
}

/** In the page, every frame the camera is still: the most each box's picture moved from where it was first, past half a canvas pixel. */
function sample(names, read) {
  const seen = (window.__steady = { base: read(names), off: {}, frames: 0, id: 0 });
  const look = () => {
    if (!document.querySelector('.stage[data-moving]')) {
      const now = read(names);
      seen.frames++;
      for (const name of names) {
        const [a, b] = [seen.base[name], now?.[name]];
        if (!a || !b) continue;
        const off = Math.max(Math.abs(a.x - b.x), Math.abs(a.y - b.y), Math.abs(a.w - b.w), name === 'shot' ? 0 : Math.abs(a.h - b.h));
        if (off > 0.5 && off > (seen.off[name]?.by ?? 0)) seen.off[name] = { by: off, was: a, now: b };
      }
    }
    seen.id = requestAnimationFrame(look);
  };
  seen.id = requestAnimationFrame(look);
}

test('steady', async ({ page, project, api }) => {
  for (const [i, name] of CLIPS.entries()) await project.import(clip(test.info().outputPath(`${name}.mp4`), i * 120), { name, at: { x: i * 380, y: 0 } });
  await project.add({ name: 'shot', tool: SEGMENTS, kind: 'video', fields: { summary: 'Along the coast.', beats: [{ description: 'The cliffs.', duration: 4 }], wait: 0 }, at: { x: 0, y: 320 } });
  await expect.poll(async () => (await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}`)).tools.some((tool) => tool.local && tool.id === SEGMENTS), { message: 'the project lists its local tool' }).toBe(true);
  for (const name of ALL) await expect.poll(() => canvasVisible(page, name)).toBe(true);
  await fit(page);
  const away = await openPoint(page);
  await page.mouse.move(away.x, away.y);
  await expect.poll(() => page.evaluate(bodies, ALL).then((all) => all && ALL.every((name) => all[name])), { message: 'every box drawn' }).toBe(true);
  await canvasIdle(page);
  const base = await page.evaluate(bodies, ALL);

  // Every frame from here with the camera still: what moved, by box. (Sent as source: the page's CSP allows no eval of its own.)
  await page.evaluate(`(${sample})(${JSON.stringify(ALL)}, ${bodies})`);

  await test.step('each hovered and left: its picture where it was', async () => {
    for (const name of ALL) {
      await hoverBox(page, name);
      await page.waitForTimeout(300);
      expect.soft(moved(base, await page.evaluate(bodies, ALL)), `${name} hovered`).toEqual([]);
      await page.mouse.move(away.x, away.y);
      await expect(page.locator(`.canvas-node:has(.card[data-asset="${name}"])`)).toHaveCount(0);
      await page.waitForTimeout(300);
      expect.soft(moved(base, await page.evaluate(bodies, ALL)), `${name} left`).toEqual([]);
    }
  });

  await test.step("shot's take lands: nothing moves (shot may take its take's height)", async () => {
    await api.call('POST', project.at('/runs'), { names: ['shot'], on: ['local'] }, { headers: { 'Idempotency-Key': randomUUID() } });
    await expect.poll(async () => (await project.asset('shot'))?.versions ?? 0, { message: 'its take', timeout: RUN_MS }).toBeGreaterThanOrEqual(1);
    await page.waitForTimeout(2000);
    expect.soft(moved(base, await page.evaluate(bodies, ALL), ['shot']), 'after it landed').toEqual([]);
  });

  await test.step('panned: the screen moves, the boxes on the canvas do not', async () => {
    const was = await camera(page);
    for (let i = 0; i < 3; i++) await page.mouse.wheel(150, 90);
    await canvasIdle(page);
    const now = await camera(page);
    expect.soft(now.x !== was.x || now.y !== was.y, 'the canvas panned').toBe(true);
    expect.soft(moved(base, await page.evaluate(bodies, ALL), ['shot']), 'after the pan').toEqual([]);
  });

  await test.step('in every still frame, nothing moved', async () => {
    const seen = await page.evaluate(() => { cancelAnimationFrame(window.__steady.id); return window.__steady; });
    expect.soft(seen.frames, 'frames looked at').toBeGreaterThan(30);
    expect.soft(seen.off, 'the most each box moved in a still frame, past half a canvas pixel').toEqual({});
  });
});
