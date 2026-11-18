// focus-fade (cl-1198; Paul, 11-17: "when you focus on a specific node, or unfocus on it, you should fade away the other nodes and unfade
// them (so they appear and disappear in a natural animation as we zoom into the focus-mode, or zoom out back to canvas-free-roam mode)"):
// 120 boxes, at 100% (a runner's software GL draws a few frames a move with all 120 in view), every frame of each move read in the page.
// Into Focus, every other box's opacity goes from 1 to 0 as far along as the camera is in each frame (its zoom says: the move eases the
// canvas's width in world units, 1/zoom), and then it is neither drawn nor hit; the box opened stays at 1. Out, from 0 to 1 the same way,
// and hit only once at 1. Turned on the way in, by Esc or by the browser going back to another box's Focus, each box goes on from the
// opacity it has, never faster than the move's curve. With reduced motion, the move and the fade come at once. Frame times of these moves
// with all 120 in view, and of a plain zoom on the same canvas, go to focus-fade.json; the moves as the browser painted them (start, the
// frames part way, end; paper and dark, 1440×900 and 1920×1080) to focus-fade/. `@solo`: a runner busy with other cells draws no frame
// inside a move (web-3, e2e 44205054246), and its frame times are the cell's own.
const { mkdirSync, writeFileSync } = require('fs');
const { join } = require('path');
const { test, expect, canvasPoint, canvasIdle, png, themed } = require('./fixtures');
const { sameCamera } = require('./geometry');
const { fit } = require('./hands');

const [ACROSS, DOWN, W, H, DX, DY] = [12, 10, 200, 125, 240, 200];
const named = (i) => `b${i}`;
// The box opened, and its neighbours: on its right, in view in its Focus (under Focus's side column), and on its left.
const OPENED = named(4 * ACROSS + 5);
const RIGHT = named(4 * ACROSS + 6);
const LEFT = named(4 * ACROSS + 4);
const WATCHED = [OPENED, RIGHT, LEFT];
/** The canvas's move into and out of Focus (CanvasHost's HERE_MS), and the steepest its curve (easeInOutSine) goes: π/2 over its time. */
const MOVE_MS = 260;
const STEEPEST = Math.PI / 2 / MOVE_MS;
const soft = expect.configure({ soft: true });

/** Picture `i`: its own hue, lighter down and stronger across, so no two share a file. */
const picture = (i) => png(0, W, H, false, (x, y) => {
  const hue = ((i * 37) % 360) / 60, light = 0.35 + 0.4 * (y / H), chroma = (1 - Math.abs(2 * light - 1)) * (0.4 + 0.5 * (x / W));
  const second = chroma * (1 - Math.abs((hue % 2) - 1)), base = light - chroma / 2;
  const rgb = [[chroma, second, 0], [second, chroma, 0], [0, chroma, second], [0, second, chroma], [second, 0, chroma], [chroma, 0, second]][Math.floor(hue) % 6];
  return rgb.map((v) => Math.round((v + base) * 255));
});

/** How far along a move from zoom `from` to `to` a frame at `zoom` is: 0 to 1, as the move eases 1/zoom. */
const along = (zoom, from, to) => Math.min(1, Math.max(0, (1 / zoom - 1 / from) / (1 / to - 1 / from)));

/** In the page, every frame until stopped: the camera's zoom, Focus's box, and each of `names` as the canvas holds it (null: it holds none
 *  of it): its opacity, whether it's drawn, and whether the canvas's own hit test finds it at a point of it on screen (null: none on
 *  screen). `turn` ({ below, action }): once 1/zoom is at or below `below` (the move under way), `action` once: `escape` (Esc) or
 *  `back2` (the browser two steps back). */
function SAMPLE({ names, turn }) {
  const root = document.querySelector('.canvas-root'), inspection = root.canvasInspection, pane = root.getBoundingClientRect();
  const seen = (window.__fade = { on: true, frames: [], turned: null });
  const box = (name) => {
    const card = inspection.card(name), shown = card?.rendered;
    if (!shown) return null;
    const { x, y, width, height } = card.body;
    const l = Math.max(x, pane.left + 1), r = Math.min(x + width, pane.right - 1), t = Math.max(y, pane.top + 1), b = Math.min(y + height, pane.bottom - 1);
    const hit = l < r && t < b ? inspection.hitTest({ x: (l + r) / 2, y: (t + b) / 2 })?.name === name : null;
    return { alpha: shown.transform.alpha, drawn: shown.transform.visible !== false, hit };
  };
  const tick = (time) => {
    if (!seen.on) return;
    const zoom = inspection.camera().zoom, region = document.querySelector('[data-focused-asset]');
    seen.frames.push({ t: time, wall: performance.timeOrigin + time, zoom, focused: region?.dataset.focusedAsset ?? null, boxes: Object.fromEntries(names.map((name) => [name, box(name)])) });
    if (turn && seen.turned === null && 1 / zoom <= turn.below) {
      seen.turned = seen.frames.length - 1;
      if (turn.action === 'escape') dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
      else history.go(-2);
    }
    requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
}

/** The canvas at rest: still, nothing being drawn, every picture in. */
async function rest(page) {
  await canvasIdle(page);
  await expect.poll(() => page.evaluate(() => {
    const inspection = document.querySelector('.canvas-root')?.canvasInspection, stats = inspection?.resources();
    return Boolean(inspection?.idleNow() && stats && stats.queued === 0 && stats.loading === 0);
  }), { message: 'the canvas at rest, every picture in', timeout: 60_000 }).toBe(true);
}

/** Focus open on `name` and drawn in, the canvas at rest. */
async function opened(page, name) {
  const region = page.getByRole('region', { name: 'Focused asset' });
  await expect(region).toHaveAttribute('data-focused-asset', name);
  await expect(region).not.toHaveAttribute('data-entering');
  await rest(page);
}

/** Focus closed, the canvas at rest. */
async function closed(page) {
  await expect(page.getByRole('region', { name: 'Focused asset' })).toHaveCount(0);
  await rest(page);
}

/** Every frame from `act` until `done`, sampled in the page (SAMPLE); with `turn`, `done` waits for the turn first. */
async function watch(page, act, done, turn = null) {
  await page.evaluate(SAMPLE, { names: WATCHED, turn });
  await act();
  if (turn) await expect.poll(() => page.evaluate(() => window.__fade.turned), { message: 'the move under way' }).not.toBeNull();
  await done();
  return page.evaluate(() => { window.__fade.on = false; return { frames: window.__fade.frames, turned: window.__fade.turned }; });
}

const dblclick = (page, name) => async () => {
  const { x, y } = await canvasPoint(page, name);
  await page.mouse.dblclick(x, y);
};

/** Steps of `name`'s opacity faster than the move's curve can go in that frame's time: jumps. */
const jumps = (frames, name) => frames.flatMap((frame, i) => {
  const was = frames[i - 1]?.boxes[name], now = frame.boxes[name];
  if (!was || !now) return [];
  const ms = frame.t - frames[i - 1].t;
  return Math.abs(now.alpha - was.alpha) > STEEPEST * ms + 0.05 ? [{ frame: i, from: +was.alpha.toFixed(3), to: +now.alpha.toFixed(3), ms: +ms.toFixed(1) }] : [];
});

/** `name`'s opacity from its turn on (its lowest from frame `from` with `sign` 1, its highest with -1): steps against `sign`. */
const against = (frames, name, from, sign) => {
  const after = frames.slice(from).map((frame) => frame.boxes[name]?.alpha ?? null);
  const known = after.filter((alpha) => alpha !== null);
  const turn = after.indexOf(sign > 0 ? Math.min(...known) : Math.max(...known));
  return after.slice(turn).flatMap((alpha, i, list) => (i > 0 && alpha !== null && list[i - 1] !== null && sign * (alpha - list[i - 1]) < -0.01 ? [{ frame: from + turn + i, from: list[i - 1], to: alpha }] : []));
};

/** The neighbours against the camera, frame by frame, on a move from zoom `from` to `to`: each one drawn, where the move is part way, at
 *  `want(p)`; how many frames show one part way; how many frames the move took. */
function onTheMove(frames, { from, to, want }) {
  const off = [];
  let partWay = 0, moving = 0;
  for (const [i, frame] of frames.entries()) {
    const p = along(frame.zoom, from, to);
    if (p <= 0.02 || p >= 0.98) continue;
    moving++;
    for (const name of [RIGHT, LEFT]) {
      const seen = frame.boxes[name];
      if (!seen) continue;
      if (seen.alpha > 0.05 && seen.alpha < 0.95) partWay++;
      if (Math.abs(seen.alpha - want(p)) > 0.03) off.push({ frame: i, name, alpha: +seen.alpha.toFixed(3), along: +p.toFixed(3), want: +want(p).toFixed(3) });
    }
  }
  return { off, partWay, moving };
}

const quantiles = (deltas) => {
  const sorted = [...deltas].sort((a, b) => a - b), at = (q) => (sorted.length ? +sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))].toFixed(1) : null);
  return { frames: sorted.length, p50: at(0.5), p95: at(0.95), most: sorted.length ? +sorted.at(-1).toFixed(1) : null, over50: sorted.filter((ms) => ms > 50).length };
};

test('focus-fade', { tag: '@solo' }, async ({ page, project, api }, testInfo) => {
  test.setTimeout(10 * 60_000);
  const shots = join(process.env.OUT, 'web', 'focus-fade');
  mkdirSync(shots, { recursive: true });
  const record = { boxes: ACROSS * DOWN };
  const save = () => writeFileSync(testInfo.outputPath('focus-fade.json'), JSON.stringify(record, null, 2));
  const camera = () => page.evaluate(() => document.querySelector('.canvas-root').canvasInspection.camera());
  const zoomMenu = (item) => async () => {
    await page.getByRole('button', { name: /more zoom options$/ }).click();
    await page.getByRole('menuitem', { name: item }).click();
  };
  /** The project in `theme` at `size`, fitted, then at 100% about its middle: OPENED and its neighbours. */
  const open = async (theme, size) => {
    await project.look(theme);
    await page.setViewportSize(size);
    await page.goto(project.url());
    await expect(page.locator('.stage')).toBeVisible();
    await themed(page, theme);
    await expect.poll(() => page.evaluate(() => document.querySelector('.canvas-root')?.canvasInspection?.names().length ?? 0), { message: 'every box on the canvas' }).toBe(ACROSS * DOWN);
    await fit(page);
    await rest(page);
    await zoomMenu(/^Zoom to 100%/)();
    await rest(page);
  };

  await test.step(`${ACROSS * DOWN} boxes, each its own picture`, async () => {
    const files = [];
    for (let i = 0; i < ACROSS * DOWN; i++) files.push(await project.upload(picture(i)));
    const applied = await api.post(project.at('/apply'), { nodes: Object.fromEntries(files.map((file, i) => [named(i), { kind: 'image', takes: [{ file }] }])) });
    expect(applied.applied, `one apply makes them: ${JSON.stringify(applied.errors ?? []).slice(0, 300)}`).toBe(true);
    await api.patch(project.at('/canvas'), { cards: Object.fromEntries(files.map((_, i) => [named(i), { x: (i % ACROSS) * DX, y: Math.floor(i / ACROSS) * DY, w: W }])) });
    await open('paper', { width: 1440, height: 900 });
  });

  const home = await camera();
  let focusZoom = null;

  await test.step('into Focus: the other boxes go from 1 to 0 with the move, then are neither drawn nor hit; the box opened stays', async () => {
    const { frames } = await watch(page, dblclick(page, OPENED), () => opened(page, OPENED));
    const last = frames.at(-1);
    focusZoom = last.zoom;
    const { off, partWay, moving } = onTheMove(frames, { from: home.zoom, to: focusZoom, want: (p) => 1 - p });
    record.into = { zoom: [home.zoom, focusZoom], frames: frames.length, moving, partWay };
    expect(moving, `frames of the move from zoom ${home.zoom} to ${focusZoom}`).toBeGreaterThanOrEqual(1);
    soft(partWay, 'frames of the move with a neighbour part way out: it fades as the camera moves, not before or after').toBeGreaterThanOrEqual(1);
    soft(off, 'each neighbour as far out as the camera is in, frame by frame').toEqual([]);
    soft(frames.filter((frame) => frame.boxes[OPENED] && frame.boxes[OPENED].alpha !== 1).map((frame) => frame.boxes[OPENED].alpha), `${OPENED}, opened, at 1 throughout`).toEqual([]);
    soft(last.boxes[RIGHT], `${RIGHT} held by the canvas in Focus, beside ${OPENED}: what's judged gone is in view`).not.toBeNull();
    for (const name of [RIGHT, LEFT]) {
      soft(frames[0].boxes[name]?.alpha, `${name} at 1 before the move`).toBe(1);
      const end = last.boxes[name];
      soft(end && { alpha: end.alpha, drawn: end.drawn, hit: end.hit === true }, `${name} in Focus: gone, not drawn, not hit`).toEqual(end ? { alpha: 0, drawn: false, hit: false } : null);
      soft(jumps(frames, name), `${name} never jumps`).toEqual([]);
    }
  });

  await test.step('out of Focus: back from 0 to 1 with the move, hit only once at 1', async () => {
    const { frames } = await watch(page, () => page.keyboard.press('Escape'), () => closed(page));
    const last = frames.at(-1);
    const { off, partWay, moving } = onTheMove(frames, { from: focusZoom, to: home.zoom, want: (p) => p });
    record.out = { frames: frames.length, moving, partWay };
    expect(moving, 'frames of the move back').toBeGreaterThanOrEqual(1);
    soft(partWay, 'frames of the move with a neighbour part way in').toBeGreaterThanOrEqual(1);
    soft(off, 'each neighbour as far in as the camera is out, frame by frame').toEqual([]);
    soft(frames.filter((frame) => frame.boxes[OPENED]?.drawn && frame.boxes[OPENED].alpha !== 1).map((frame) => frame.boxes[OPENED].alpha), `${OPENED} at 1 on its way back`).toEqual([]);
    for (const name of [RIGHT, LEFT]) {
      soft(frames.filter((frame) => frame.boxes[name]?.hit && frame.boxes[name].alpha < 1).map((frame) => +frame.boxes[name].alpha.toFixed(3)), `${name} hit while it fades in`).toEqual([]);
      const end = last.boxes[name];
      soft(end && { alpha: end.alpha, drawn: end.drawn, hit: end.hit }, `${name} back: at 1, drawn, hit`).toEqual({ alpha: 1, drawn: true, hit: true });
      soft(jumps(frames, name), `${name} never jumps`).toEqual([]);
    }
    const now = await camera();
    soft(sameCamera(now, home), `the canvas where it was, ${JSON.stringify(home)}, not ${JSON.stringify(now)}`).toBe(true);
  });

  // Under way: its first frame 3% along, part way however few frames a runner draws.
  const underWay = () => 1 / home.zoom - 0.03 * (1 / home.zoom - 1 / focusZoom);

  await test.step('Esc on the way in: each box turns back from the opacity it has', async () => {
    const { frames, turned } = await watch(page, dblclick(page, OPENED), () => closed(page), { below: underWay(), action: 'escape' });
    const at = frames[turned].boxes[RIGHT];
    record.escOnTheWay = { frames: frames.length, turned, alpha: at?.alpha ?? null, along: along(frames[turned].zoom, home.zoom, focusZoom) };
    soft(at && at.alpha > 0.02 && at.alpha < 0.98, `${RIGHT} part way out as Esc comes: ${JSON.stringify(at)}`).toBe(true);
    for (const name of [RIGHT, LEFT]) {
      soft(against(frames, name, turned, 1), `${name}, from its lowest after Esc, only comes back`).toEqual([]);
      soft(frames.at(-1).boxes[name]?.alpha, `${name} back at 1`).toBe(1);
      soft(jumps(frames, name), `${name} never jumps`).toEqual([]);
    }
    soft(frames.filter((frame) => frame.boxes[OPENED] && frame.boxes[OPENED].alpha !== 1).length, `frames with ${OPENED} under 1`).toBe(0);
    const now = await camera();
    soft(sameCamera(now, home), `the canvas where it was, ${JSON.stringify(home)}, not ${JSON.stringify(now)}`).toBe(true);
  });

  await test.step('another box opened on the way in (the browser back to its Focus): each goes on from the opacity it has', async () => {
    // The browser's history: the canvas, RIGHT's Focus, the canvas, OPENED's Focus; two steps back is RIGHT's.
    await dblclick(page, RIGHT)();
    await opened(page, RIGHT);
    await page.keyboard.press('Escape');
    await closed(page);
    const { frames, turned } = await watch(page, dblclick(page, OPENED), () => opened(page, RIGHT), { below: underWay(), action: 'back2' });
    // The browser's Back lands frames on, where a slow runner's move into OPENED is over: RIGHT turns from its lowest, gone or part way.
    const lowest = Math.min(...frames.slice(turned).map((frame) => frame.boxes[RIGHT]?.alpha ?? 1));
    record.otherOnTheWay = { frames: frames.length, turned, lowest };
    soft(lowest, `${RIGHT} on its way out as it's opened`).toBeLessThan(0.98);
    const last = frames.at(-1).boxes;
    soft(against(frames, RIGHT, turned, 1), `${RIGHT}, opened, from its lowest only comes back`).toEqual([]);
    soft(last[RIGHT]?.alpha, `${RIGHT}, opened, at 1`).toBe(1);
    for (const name of [OPENED, LEFT]) {
      soft(against(frames, name, turned, -1), `${name}, from its highest, only goes`).toEqual([]);
      soft(last[name] ? last[name].alpha : 0, `${name} gone`).toBe(0);
    }
    for (const name of WATCHED) soft(jumps(frames, name), `${name} never jumps`).toEqual([]);
    await page.keyboard.press('Escape');
    await closed(page);
  });

  await test.step('reduced motion: into and out of Focus at once, the other boxes with it', async () => {
    await page.emulateMedia({ reducedMotion: 'reduce' });
    for (const [way, act, done, from, to, end] of [
      ['in', dblclick(page, OPENED), () => opened(page, OPENED), home.zoom, focusZoom, 0],
      ['out', () => page.keyboard.press('Escape'), () => closed(page), focusZoom, home.zoom, 1],
    ]) {
      const { frames } = await watch(page, act, done);
      soft(frames.filter((frame) => { const p = along(frame.zoom, from, to); return p > 0.02 && p < 0.98; }).map((frame) => +frame.zoom.toFixed(3)), `${way}: no zoom part way`).toEqual([]);
      soft(frames.flatMap((frame) => [RIGHT, LEFT].map((name) => frame.boxes[name]?.alpha).filter((alpha) => alpha > 0.02 && alpha < 0.98)), `${way}: no neighbour part way`).toEqual([]);
      soft(frames.at(-1).boxes[RIGHT]?.alpha, `${way}: ${RIGHT} at ${end}`).toBe(end);
    }
    await page.emulateMedia({ reducedMotion: null });
  });

  await test.step(`frame times with all ${ACROSS * DOWN} boxes in view: Focus in and out, and a plain zoom in and out, each three times`, async () => {
    /** Frame times in the page while the camera moves: from its first frame off where it was to its first where it stays. */
    const timed = async (act, done) => {
      await page.evaluate(() => {
        const inspection = document.querySelector('.canvas-root').canvasInspection, seen = (window.__frames = { on: true, frames: [] });
        const tick = (now) => { seen.frames.push([now, inspection.camera().zoom]); if (seen.on) requestAnimationFrame(tick); };
        requestAnimationFrame(tick);
      });
      await act();
      await done();
      const frames = await page.evaluate(() => { window.__frames.on = false; return window.__frames.frames; });
      const first = frames.findIndex(([, zoom]) => zoom !== frames[0][1]), still = frames.at(-1)[1];
      if (first < 0) return [];
      const moving = frames.slice(first - 1, frames.findIndex(([, zoom], i) => i >= first && zoom === still) + 1);
      return moving.slice(1).map(([now], i) => now - moving[i][0]);
    };
    await fit(page);
    await rest(page);
    const runs = { focusIn: [], focusOut: [], zoomIn: [], zoomOut: [] };
    for (let i = 0; i < 3; i++) {
      runs.focusIn.push(...await timed(dblclick(page, OPENED), () => opened(page, OPENED)));
      runs.focusOut.push(...await timed(() => page.keyboard.press('Escape'), () => closed(page)));
      runs.zoomIn.push(...await timed(zoomMenu(/^Zoom to 100%/), () => rest(page)));
      runs.zoomOut.push(...await timed(zoomMenu(/^Zoom to fit/), () => rest(page)));
    }
    record.frameMs = Object.fromEntries(Object.entries(runs).map(([key, deltas]) => [key, quantiles(deltas)]));
    testInfo.annotations.push({ type: 'frame ms', description: JSON.stringify(record.frameMs) });
    save();
  });

  await test.step('as painted: the start, the frames part way and the end of each move, paper and dark, 1440×900 and 1920×1080', async () => {
    const cdp = await page.context().newCDPSession(page);
    const kept = [];
    for (const [width, height] of [[1440, 900], [1920, 1080]]) for (const theme of ['paper', 'dark']) {
      await open(theme, { width, height });
      for (const [way, act, done] of [
        ['in', dblclick(page, OPENED), () => opened(page, OPENED)],
        ['out', () => page.keyboard.press('Escape'), () => closed(page)],
      ]) {
        const tag = `${way}-${width}x${height}-${theme}`;
        await page.screenshot({ path: join(shots, `${tag}-0-start.png`) });
        // Each frame the browser paints on the way, with when it was shown, against the page's samples of the same frames.
        const painted = [];
        const take = ({ data, metadata, sessionId }) => {
          painted.push({ data, wall: metadata?.timestamp ? metadata.timestamp * 1000 : Date.now() });
          cdp.send('Page.screencastFrameAck', { sessionId }).catch(() => {});
        };
        cdp.on('Page.screencastFrame', take);
        await cdp.send('Page.startScreencast', { format: 'jpeg', quality: 92, everyNthFrame: 1 });
        const { frames } = await watch(page, act, done);
        await cdp.send('Page.stopScreencast');
        cdp.off('Page.screencastFrame', take);
        const alphaAt = (wall) => [...frames].reverse().find((frame) => frame.wall <= wall)?.boxes[RIGHT]?.alpha ?? null;
        const partWay = painted.map((frame) => ({ ...frame, alpha: alphaAt(frame.wall) })).filter((frame) => frame.alpha !== null && frame.alpha > 0.02 && frame.alpha < 0.98);
        for (const [i, frame] of partWay.slice(0, 8).entries()) {
          const file = `${tag}-${i + 1}-a${frame.alpha.toFixed(2)}.jpg`;
          writeFileSync(join(shots, file), Buffer.from(frame.data, 'base64'));
          kept.push(file);
        }
        await page.screenshot({ path: join(shots, `${tag}-9-end.png`) });
        record.painted = { ...record.painted, [tag]: { painted: painted.length, partWay: partWay.length } };
      }
    }
    await cdp.detach();
    testInfo.annotations.push({ type: 'painted', description: JSON.stringify(record.painted) });
    save();
  });
});
