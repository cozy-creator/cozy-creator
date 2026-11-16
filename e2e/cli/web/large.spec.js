// large (Paul, 11-15: "make sure the new pixijs canvas is robust"; cl-991: a project of thousands of cards stays usable): 300 boxes, then
// 1,000, each its own picture, made by one apply and laid out by one canvas patch. For each: the first open, which makes the thumbnails,
// and the open again at the fit it kept; a 2 s wheel pan at 50% and at the fit; Ctrl+wheel out to the 5% limit and back; 20 cycles of pan
// and zoom. Frame times, the scene's resources and the JS heap go to large.json, with this runner's speed (speed.js), for bars to be set
// from measured runs. Judged now: every box in view is drawn in every frame of a pan; at 100%, off-screen cards' textures are let go down to
// the unpinned allowance the scene documents (docs/canvas-migration.md, 64 MiB); and 20 cycles grow neither the heap nor the textures past
// what cycles 2 and 10 differ by, floored at 3× the heap's own noise at rest. No time bar: opens' bars are Paul's for 50 pictures, and
// nothing says yet what 300 or 1,000 should take.
const { randomBytes } = require('crypto');
const { writeFileSync } = require('fs');
const { join } = require('path');
const { test, expect, canvasNames, png } = require('./fixtures');
const { runnerSpeed } = require('./speed');

const SIZES = [300, 1000];
const [W, DX, DY] = [240, 304, 240];
const VIEWPORT = { width: 1600, height: 1000 };
const CYCLES = 20;
const SAMPLED = [1, 2, 10, 20];
const soft = expect.configure({ soft: true });

/** Picture `i`: 160×100, its own colours, so no two share a file or a texture. */
const picture = (i) => png(0, 160, 100, false, (x, y) => [(i * 53 + x) & 255, (i * 97 + y * 2) & 255, (i * 29 + ((x + y) >> 2)) & 255]);
const named = (i) => `b${i}`;
const quantiles = (deltas) => {
  const sorted = [...deltas].sort((a, b) => a - b), at = (q) => (sorted.length ? +sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))].toFixed(1) : null);
  return { frames: sorted.length, p50: at(0.5), p95: at(0.95), p99: at(0.99), most: sorted.length ? +sorted.at(-1).toFixed(1) : null };
};

/** In the page, from its start: the first frame every box is drawn, and the first every one has its picture (the scene's own counts). */
function OPEN_SAMPLER() {
  const seen = (window.__large = { allDrawn: null, allPictures: null, frames: 0 });
  const tick = () => {
    seen.frames++;
    const stats = document.querySelector('.canvas-root')?.canvasInspection?.resources();
    if (stats?.frames && stats.totalCards) {
      if (seen.allDrawn === null && stats.visibleCards >= stats.totalCards) seen.allDrawn = performance.now();
      if (seen.allPictures === null && stats.loadedCards >= stats.totalCards) seen.allPictures = performance.now();
    }
    if (seen.allPictures === null) requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
}

/** In the page, each frame for as long as `window.__framing` holds: the frame's time, and whether the boxes in view (from the grid the cell
 *  laid out, `grid`) number no more than the scene draws. Counts, so a thousand cards cost nothing a frame. */
const FRAMING = (grid) => {
  const root = document.querySelector('.canvas-root'), inspection = root.canvasInspection, pane = root.getBoundingClientRect();
  const seen = (window.__framing = { on: true, times: [], short: [] });
  const tick = (now) => {
    seen.times.push(now);
    const camera = inspection.camera(), stats = inspection.resources();
    let inView = 0;
    for (let i = 0; i < grid.count; i++) {
      const x = pane.x + camera.x + (i % grid.across) * grid.dx * camera.zoom, y = pane.y + camera.y + Math.floor(i / grid.across) * grid.dy * camera.zoom;
      if (x + grid.w * camera.zoom > pane.left && y + grid.h * camera.zoom > pane.top && x < pane.right && y < pane.bottom) inView++;
    }
    if (stats.visibleCards < inView && seen.short.length < 20) seen.short.push({ at: Math.round(now), inView, drawn: stats.visibleCards, zoom: camera.zoom });
    if (seen.on) requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
};

test('large', { tag: '@solo' }, async ({ context, project, api, signIn, removeAfter }, testInfo) => {
  test.setTimeout(30 * 60_000);
  const speed = runnerSpeed();
  const record = { runner: speed, viewport: VIEWPORT, renderer: process.env.WEB_RENDERER === '2d' ? '2d lane' : 'webgl lane', sizes: {} };
  testInfo.annotations.push({ type: 'runner', description: JSON.stringify(speed) });
  const save = () => writeFileSync(testInfo.outputPath('large.json'), JSON.stringify(record, null, 2));

  for (const count of SIZES) {
    const seen = (record.sizes[count] = {});
    const across = Math.ceil(Math.sqrt(count * 1.6));
    const one = count === SIZES[0] ? project : await (async () => {
      const made = await api.post('/v1/projects', { path: join(process.env.OUT, 'web', 'projects', `web-large-${count}-${randomBytes(3).toString('hex')}`), unencrypted: true });
      removeAfter(made.path);
      // The suite's own project helpers, on this second project.
      return Object.assign(Object.create(Object.getPrototypeOf(project)), { api, id: made.id, code: made.code, path: made.path, name: made.name });
    })();

    await test.step(`${count} boxes, each its own picture, by one apply and one canvas patch`, async () => {
      const began = Date.now();
      const files = [];
      for (let i = 0; i < count; i++) files.push(await one.upload(picture(i)));
      const nodes = Object.fromEntries(files.map((file, i) => [named(i), { kind: 'image', takes: [{ file }] }]));
      const applied = await api.post(one.at('/apply'), { nodes });
      expect(applied.applied, `one apply makes ${count} boxes: ${JSON.stringify(applied.errors ?? []).slice(0, 300)}`).toBe(true);
      await api.patch(one.at('/canvas'), { cards: Object.fromEntries(files.map((_, i) => [named(i), { x: (i % across) * DX, y: Math.floor(i / across) * DY, w: W }])) });
      seen.made_ms = Date.now() - began;
    });

    const page = await context.newPage();
    await page.setViewportSize(VIEWPORT);
    await page.addInitScript(OPEN_SAMPLER);
    const cdp = await page.context().newCDPSession(page);
    await cdp.send('Performance.enable');
    const zoomButton = page.getByRole('button', { name: /more zoom options$/ });
    /** Camera and scene still: three samples alike, no frame asked for. Not canvasIdle: a zoom pushed past 5% leaves `data-moving` set
     *  (cl-1015), which this cell measures around, not through. */
    const still = async (what) => {
      let previous = null, alike = 0;
      await expect.poll(async () => {
        const now = await page.evaluate(() => { const inspection = document.querySelector('.canvas-root')?.canvasInspection; return inspection ? JSON.stringify({ camera: inspection.camera(), frame: inspection.resources()?.scheduledFrame }) : null; });
        alike = now && now === previous && !JSON.parse(now).frame ? alike + 1 : 0;
        previous = now;
        return alike >= 3;
      }, { message: `${what}: the canvas at rest`, intervals: [100], timeout: 30_000 }).toBe(true);
    };
    /** Every picture fetched and drawn, nothing waiting: the scene at rest. */
    const loaded = (what, timeout = 60_000) => soft.poll(() => page.evaluate(() => { const stats = document.querySelector('.canvas-root')?.canvasInspection?.resources(); return stats ? stats.loading + stats.queued : -1; }), { message: `${what}: every picture in view loaded`, timeout }).toBe(0);
    const resources = () => page.evaluate(() => {
      const stats = document.querySelector('.canvas-root').canvasInspection.resources();
      const keep = ['totalCards', 'visibleCards', 'loadedCards', 'textures', 'pinnedTextures', 'textureBytes', 'pinnedBytes', 'unusedBytes', 'unusedTargetBytes', 'targetBytes', 'overflowBytes', 'loading', 'queued', 'failed', 'labels', 'estimatedLabelBytes', 'frames'];
      return Object.fromEntries(keep.map((key) => [key, stats[key]]));
    });
    /** The JS heap in use after a full collection, and the page's DOM nodes and listeners, from Chrome. */
    const heap = async () => {
      await cdp.send('HeapProfiler.collectGarbage');
      const { metrics } = await cdp.send('Performance.getMetrics');
      const metric = (name) => metrics.find((one) => one.name === name)?.value ?? null;
      return { heap: metric('JSHeapUsedSize'), nodes: metric('Nodes'), listeners: metric('JSEventListeners') };
    };
    const camera = () => page.evaluate(() => document.querySelector('.canvas-root').canvasInspection.camera());
    const grid = async () => {
      const card = await page.evaluate((name) => { const inspection = document.querySelector('.canvas-root').canvasInspection; return { bounds: inspection.card(name)?.bounds, zoom: inspection.camera().zoom }; }, named(0));
      return { count, across, dx: DX, dy: DY, w: W, h: card.bounds.height / card.zoom };
    };
    /** Wheels in turn, each awaited a frame, the frames timed in the page from the first to the last. */
    const framed = async (label, wheels) => {
      await page.evaluate(FRAMING, await grid());
      for (const [dx, dy, ctrl] of wheels) {
        if (ctrl) await page.keyboard.down('Control');
        await page.mouse.wheel(dx, dy);
        if (ctrl) await page.keyboard.up('Control');
        await page.evaluate(() => new Promise((done) => requestAnimationFrame(done)));
      }
      const { times, short } = await page.evaluate(() => { window.__framing.on = false; return window.__framing; });
      const result = { ...quantiles(times.slice(1).map((at, i) => at - times[i])), wheels: wheels.length, short };
      seen[label] = result;
      return result;
    };
    const middle = async () => {
      const at = await page.evaluate(() => { const r = document.querySelector('.canvas-root').getBoundingClientRect(), tools = document.querySelector('.tools')?.getBoundingClientRect(); return { x: ((tools?.right ?? r.left) + r.right) / 2, y: r.top + r.height / 2 }; });
      await page.mouse.move(at.x, at.y);
    };

    await test.step('opened the first time: every box drawn, then every picture (its thumbnails made as asked)', async () => {
      const began = Date.now();
      await signIn(page, one.url());
      await expect.poll(() => canvasNames(page), { timeout: 60_000 }).toHaveLength(count);
      await zoomButton.click();
      await page.getByRole('menuitem', { name: /^Zoom to fit/ }).click();
      await still('fitted');
      await loaded('the first open', 300_000);
      seen.first_open = { wall_ms: Date.now() - began, ...(await page.evaluate(() => window.__large)) };
    });

    await test.step('opened again at the fit it kept: every box drawn, then every picture', async () => {
      // The page's own view saved, so the reload opens at the fit.
      const fit = await camera();
      await expect.poll(async () => { const { viewport } = await api.get(one.at('/view')); return Boolean(viewport) && Math.abs(viewport.zoom - fit.zoom) < 0.0005; }, { message: 'the fit kept as its view' }).toBe(true);
      await page.reload();
      await expect.poll(() => page.evaluate(() => window.__large?.allPictures ?? null), { message: 'every picture drawn after the reload', timeout: 120_000 }).not.toBeNull();
      const opened = await page.evaluate((id) => {
        const path = (entry) => new URL(entry.name).pathname;
        const resources = performance.getEntriesByType('resource');
        const state = resources.find((entry) => path(entry) === `/v1/projects/${encodeURIComponent(id)}` && !new URL(entry.name).search);
        const canvas = resources.find((entry) => path(entry) === `/v1/projects/${encodeURIComponent(id)}/canvas`);
        const from = state && canvas ? Math.max(state.responseEnd, canvas.responseEnd) : null;
        const { allDrawn, allPictures, frames } = window.__large;
        const after = (at) => (at === null || from === null ? null : Math.round(at - from));
        return { state_ms: from === null ? null : Math.round(from), all_drawn_ms: Math.round(allDrawn), all_pictures_ms: Math.round(allPictures), drawn_after_state_ms: after(allDrawn), pictures_after_state_ms: after(allPictures), frames };
      }, one.id);
      seen.open = { ...opened, zoom: fit.zoom, resources: await resources(), memory: await heap() };
      save();
    });

    await test.step('a 2 s wheel pan at the fit, then at 50%: every box in view drawn in every frame', async () => {
      await still('the open');
      await middle();
      const pan = (n) => Array.from({ length: n }, (_, i) => [i < n / 2 ? 24 : -24, 0, false]);
      const overview = await framed('pan_fit', pan(60));
      soft(overview.short, 'at the fit, every box in view drawn in every frame of the pan').toEqual([]);
      await page.keyboard.press('Shift+Digit0');
      await still('100%');
      await page.getByRole('button', { name: 'Zoom out', exact: true }).click();
      await page.getByRole('button', { name: 'Zoom out', exact: true }).click();
      await still('50%');
      await loaded('50%');
      seen.working_zoom = (await camera()).zoom;
      const working = await framed('pan_working', pan(60));
      soft(working.short, 'at 50%, every box in view drawn in every frame of the pan').toEqual([]);
    });

    await test.step('Ctrl+wheel out to the 5% limit and back', async () => {
      await still('the pan');
      await middle();
      const from = (await camera()).zoom;
      const wheels = [...Array.from({ length: 24 }, () => [0, 60, true]), ...Array.from({ length: 24 }, () => [0, -60, true])];
      const zoomed = await framed('zoom', wheels);
      zoomed.from = from;
      zoomed.to = (await camera()).zoom;
      soft(zoomed.short, 'every box in view drawn in every frame of the zoom').toEqual([]);
      seen.moving_after_zoom = await page.evaluate(() => Boolean(document.querySelector('.stage[data-moving]')));
    });

    await test.step('at 100%, off-screen cards let go of their pictures, down to the scene\'s unpinned allowance', async () => {
      await zoomButton.click();
      await page.getByRole('menuitem', { name: /^Zoom to fit/ }).click();
      await still('fitted again');
      await loaded('fitted again', 120_000);
      const all = await resources();
      await page.keyboard.press('Shift+Digit0');
      await still('100%');
      await loaded('100%');
      await expect.poll(async () => (await resources()).unusedBytes <= all.unusedTargetBytes, { message: 'unpinned textures within the allowance', timeout: 10_000 }).toBe(true).catch(() => {});
      const near = await resources();
      seen.let_go = { fit: all, hundred: near };
      soft(near.pinnedBytes, `fewer pinned bytes in view at 100% (${near.visibleCards} cards) than all ${count} at the fit`).toBeLessThan(all.pinnedBytes);
      soft(near.unusedBytes, 'what is no longer in view is kept only up to the unpinned allowance').toBeLessThanOrEqual(near.unusedTargetBytes);
      soft(near.textureBytes, 'every texture held is one in view or within the allowance').toBeLessThanOrEqual(near.pinnedBytes + near.unusedTargetBytes);
    });

    await test.step(`${CYCLES} cycles of a pan and a zoom at 50%: no growth`, async () => {
      await page.getByRole('button', { name: 'Zoom out', exact: true }).click();
      await page.getByRole('button', { name: 'Zoom out', exact: true }).click();
      await still('50%');
      await middle();
      const home = await camera();
      const samples = {};
      const cycleFrames = [];
      for (let cycle = 1; cycle <= CYCLES; cycle++) {
        const wheels = [...Array.from({ length: 20 }, () => [30, 0, false]), ...Array.from({ length: 20 }, () => [-30, 0, false]), ...Array.from({ length: 8 }, () => [0, 60, true]), ...Array.from({ length: 8 }, () => [0, -60, true])];
        const result = await framed(`cycle_${cycle}`, wheels);
        delete seen[`cycle_${cycle}`];
        cycleFrames.push({ cycle, p50: result.p50, p95: result.p95, p99: result.p99, short: result.short.length });
        soft(result.short, `cycle ${cycle}: every box in view drawn in every frame`).toEqual([]);
        if (!SAMPLED.includes(cycle)) continue;
        await still(`cycle ${cycle}`);
        await loaded(`cycle ${cycle}`);
        const at = await camera();
        samples[cycle] = { camera: at, resources: await resources(), memory: await heap() };
        // The heap's own noise at rest, at cycle 2: two more collections.
        if (cycle === 2) samples[cycle].noise = [samples[cycle].memory.heap, (await heap()).heap, (await heap()).heap];
      }
      seen.cycles = { home, frames: cycleFrames, samples };
      save();
      const [two, ten, twenty] = [samples[2], samples[10], samples[20]];
      const noise = Math.max(...two.noise) - Math.min(...two.noise);
      const allowed = (key, of) => Math.max(Math.abs(of(ten) - of(two)), key === 'heap' ? 3 * noise : 0);
      const growth = {
        heap: { two: two.memory.heap, ten: ten.memory.heap, twenty: twenty.memory.heap, noise, allowed: allowed('heap', (s) => s.memory.heap) },
        textureBytes: { two: two.resources.textureBytes, ten: ten.resources.textureBytes, twenty: twenty.resources.textureBytes, allowed: allowed('textureBytes', (s) => s.resources.textureBytes) },
        listeners: { two: two.memory.listeners, ten: ten.memory.listeners, twenty: twenty.memory.listeners, allowed: allowed('listeners', (s) => s.memory.listeners) },
        nodes: { two: two.memory.nodes, ten: ten.memory.nodes, twenty: twenty.memory.nodes, allowed: allowed('nodes', (s) => s.memory.nodes) },
      };
      seen.growth = growth;
      save();
      for (const [key, row] of Object.entries(growth)) soft(row.twenty - row.two, `${key} after ${CYCLES} cycles against cycle 2 (allowed: cycles 2–10's difference${key === 'heap' ? ', at least 3× the noise at rest' : ''}): ${JSON.stringify(row)}`).toBeLessThanOrEqual(row.allowed);
      soft(twenty.resources.unusedBytes, 'unpinned textures within the allowance after the cycles').toBeLessThanOrEqual(twenty.resources.unusedTargetBytes);
    });
    await page.close();
  }
  save();
  await testInfo.attach('large', { path: testInfo.outputPath('large.json'), contentType: 'application/json' });
});
