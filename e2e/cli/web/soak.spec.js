// soak (Paul, 11-15: "make sure the new pixijs canvas is robust"): a long session on one tab, as an hour of steady work does it. Each of 20
// cycles switches to another project and back by their tabs, writes in a box in Focus and leaves, moves a box, adds one (Ctrl+D) and
// deletes it, while a slow render keeps its progress coming and lands its takes: 40 switches, 20 edits, 20 moves, 20 boxes made and
// deleted, a take every few cycles, about what an hour of editing does, in ~4 minutes. From before the page's first script, the listeners
// it holds (on the window, the document, what's in the page and any other target), its intervals and its animation frames are counted.
// At rest after cycle 2, 10 and 20 (render canceled, notes gone, the canvas still), judged: no page or console error (the guards); no
// listener type or interval more than after cycle 2; no animation frame asked for in 2 s (no ticker left running); the JS heap and the
// scene's textures and labels no more than cycle 2's by what cycles 2 and 10 differ by, the heap floored at 3× its noise at rest.
// Every sample goes to soak.json.
const { randomBytes, randomUUID } = require('crypto');
const { writeFileSync } = require('fs');
const { join } = require('path');
const { test, expect, canvasIdle, canvasNames, canvasPoint, canvasVisible, fixtureTool, focus, openPoint, png, selectedNames } = require('./fixtures');
const { carry, rows } = require('./geometry');

const CYCLES = 20;
const SAMPLED = [2, 10, 20];
const SLOW = fixtureTool('tool.py/slow');
const soft = expect.configure({ soft: true });

/** Before the page's own scripts: every listener added and not yet removed, by target and type; intervals still set; frames asked for. */
function LEDGER() {
  const add = EventTarget.prototype.addEventListener, remove = EventTarget.prototype.removeEventListener;
  const held = new WeakMap(), targets = [];
  const keyOf = (type, options) => `${type}|${typeof options === 'boolean' ? options : Boolean(options?.capture)}`;
  EventTarget.prototype.addEventListener = function (type, listener, options) {
    // A `once` listener lets itself go when it fires; the rest are held until removed or their signal aborts.
    if (listener && !(typeof options === 'object' && options?.once)) {
      let map = held.get(this);
      if (!map) { map = new Map(); held.set(this, map); targets.push(new WeakRef(this)); }
      const key = keyOf(type, options), set = map.get(key) ?? new Set();
      map.set(key, set);
      set.add(listener);
      if (typeof options === 'object' && options?.signal) add.call(options.signal, 'abort', () => set.delete(listener), { once: true });
    }
    return add.call(this, type, listener, options);
  };
  EventTarget.prototype.removeEventListener = function (type, listener, options) {
    held.get(this)?.get(keyOf(type, options))?.delete(listener);
    return remove.call(this, type, listener, options);
  };
  const ledger = (window.__ledger = { frames: 0, intervals: new Set() });
  const frame = window.requestAnimationFrame.bind(window);
  window.requestAnimationFrame = (callback) => { ledger.frames++; return frame(callback); };
  const every = window.setInterval.bind(window), stop = window.clearInterval.bind(window);
  window.setInterval = (...args) => { const id = every(...args); ledger.intervals.add(id); return id; };
  window.clearInterval = (id) => { ledger.intervals.delete(id); return stop(id); };
  // What's held on what can still fire: the window, the document, nodes in the page, and every other target. A node no longer in the page
  // goes with its listeners; one that's kept anyway shows in the heap.
  ledger.listeners = () => {
    const counts = {};
    for (const ref of targets) {
      const target = ref.deref();
      if (!target || (target instanceof Node && target !== document && !target.isConnected)) continue;
      const kind = target === window ? 'window' : target === document ? 'document' : target instanceof Node ? 'node' : target.constructor?.name ?? 'other';
      for (const [key, set] of held.get(target) ?? []) if (set.size) counts[`${kind}:${key.split('|')[0]}`] = (counts[`${kind}:${key.split('|')[0]}`] ?? 0) + set.size;
    }
    return counts;
  };
}

test('soak', { tag: '@solo' }, async ({ context, project, api, signIn, removeAfter }, testInfo) => {
  test.setTimeout(15 * 60_000);
  const record = { cycles: CYCLES, renderer: process.env.WEB_RENDERER === '2d' ? '2d lane' : 'webgl lane', samples: {}, timeline: [] };
  const save = () => writeFileSync(testInfo.outputPath('soak.json'), JSON.stringify(record, null, 2));
  const b = await api.post('/v1/projects', { path: join(process.env.OUT, 'web', 'projects', `web-soak-b-${randomBytes(3).toString('hex')}`), unencrypted: true });
  removeAfter(b.path);
  await api.post(`/v1/projects/${encodeURIComponent(b.id)}/assets`, { name: 'only_b', tool: '', exact: true, fields: { text: 'project b' }, at: { x: 0, y: 0 } });
  await project.add({ name: 'notes', kind: 'text', fields: { text: 'a soak' }, at: { x: 0, y: 0 } });
  await project.import(png(90), { name: 'spare', at: { x: 420, y: 0 } });
  for (let i = 0; i < 6; i++) await project.import(png(20 + i * 35), { name: `pic_${i}`, at: { x: (i % 3) * 420, y: 440 + Math.floor(i / 3) * 400 } });
  const { local_usable: renders } = await api.get('/v1/status');
  if (renders) await project.add({ name: 'making', tool: SLOW, kind: 'image', fields: { instructions: 'slowly', seconds: 40 }, at: { x: 840, y: 0 } });
  record.renders = renders;

  await context.addInitScript(LEDGER);
  const page = await context.newPage();
  await page.setViewportSize({ width: 1600, height: 1000 });
  const cdp = await page.context().newCDPSession(page);
  await cdp.send('Performance.enable');
  await signIn(page, project.url());
  for (const name of ['notes', 'spare', 'pic_5']) await expect.poll(() => canvasVisible(page, name), { timeout: 30_000 }).toBe(true);
  const tab = (row) => page.locator(`nav.ptabs a.ptab[href="/p/${row.code}"]`);

  /** The slow render going, started again whenever its last take has landed. */
  const keepRendering = async () => {
    if (!renders) return;
    if ((await project.asset('making'))?.standing === 'making') return;
    await api.call('POST', project.at('/runs'), { names: ['making'], on: ['local'] }, { headers: { 'Idempotency-Key': randomUUID() } });
  };
  /** Everything stopped that's meant to stop: the render canceled, Focus closed, the pointer on open canvas, notes gone, nothing loading. */
  const atRest = async (label) => {
    if (renders) {
      await api.raw('POST', project.at('/cancel'), { body: { names: ['making'] } });
      await expect.poll(async () => (await project.asset('making'))?.standing !== 'making', { message: `${label}: its render canceled`, timeout: 60_000 }).toBe(true);
    }
    await expect(page.getByRole('region', { name: 'Focused asset' })).toHaveCount(0);
    const open = await openPoint(page);
    await page.mouse.move(open.x, open.y);
    await soft.poll(() => page.locator('.notes .note').count(), { message: `${label}: notes gone`, timeout: 30_000 }).toBe(0);
    await canvasIdle(page);
    await expect.poll(() => page.evaluate(() => { const stats = document.querySelector('.canvas-root')?.canvasInspection?.resources(); return stats && !stats.scheduledFrame && !stats.loading && !stats.queued && stats.activeLatents === 0; }), { message: `${label}: the scene at rest`, timeout: 30_000 }).toBe(true);
  };
  /** At rest: the ledger, frames asked for over 2 s, the JS heap after a full collection, and the scene's resources. */
  const sample = async (label) => {
    await atRest(label);
    const framesBefore = await page.evaluate(() => window.__ledger.frames);
    await page.waitForTimeout(2000);
    const ledger = await page.evaluate(() => ({ frames: window.__ledger.frames, intervals: window.__ledger.intervals.size, listeners: window.__ledger.listeners() }));
    const heaps = [];
    for (let i = 0; i < 3; i++) {
      await cdp.send('HeapProfiler.collectGarbage');
      const { metrics } = await cdp.send('Performance.getMetrics');
      heaps.push(Object.fromEntries(metrics.filter((one) => ['JSHeapUsedSize', 'Nodes', 'JSEventListeners', 'Documents'].includes(one.name)).map((one) => [one.name, one.value])));
    }
    const stats = await page.evaluate(() => {
      const all = document.querySelector('.canvas-root').canvasInspection.resources();
      return Object.fromEntries(['totalCards', 'visibleCards', 'textures', 'pinnedTextures', 'textureBytes', 'pinnedBytes', 'unusedBytes', 'unusedTargetBytes', 'labels', 'estimatedLabelBytes'].map((key) => [key, all[key]]));
    });
    const said = { framesAtRest: ledger.frames - framesBefore, intervals: ledger.intervals, listeners: ledger.listeners, heap: heaps.map((one) => one.JSHeapUsedSize), nodes: heaps[2].Nodes, jsListeners: heaps[2].JSEventListeners, documents: heaps[2].Documents, resources: stats };
    record.samples[label] = said;
    save();
    return said;
  };

  for (let cycle = 1; cycle <= CYCLES; cycle++) {
    await test.step(`cycle ${cycle}`, async () => {
      const began = Date.now();
      await keepRendering();
      await tab(b).click();
      await expect.poll(() => canvasVisible(page, 'only_b'), { message: 'project b on its tab' }).toBe(true);
      await tab(project).click();
      await expect.poll(() => canvasVisible(page, 'notes'), { message: 'and back' }).toBe(true);

      const region = await focus(page, 'notes');
      const text = region.getByRole('textbox', { name: 'Text' });
      // Its words replaced by as many as before, so what the canvas holds stays the same size from cycle to cycle.
      const words = `Cycle ${String(cycle).padStart(2, '0')} of the soak.`;
      await text.click();
      await page.keyboard.press('ControlOrMeta+a');
      await page.keyboard.type(words);
      await page.keyboard.press('Escape');
      await expect(region).toHaveCount(0);
      await soft.poll(async () => (await project.asset('notes'))?.fields?.text, { message: `cycle ${cycle}: the words saved` }).toBe(words);

      await canvasIdle(page);
      const was = (await rows(project)).spare;
      const from = await canvasPoint(page, 'spare', 'title');
      await carry(page, from, { x: from.x + (cycle % 2 ? 80 : -80), y: from.y });
      await soft.poll(async () => (await rows(project)).spare?.x !== was?.x, { message: `cycle ${cycle}: the move saved` }).toBe(true);

      await canvasIdle(page);
      const before = await canvasNames(page);
      const body = await canvasPoint(page, 'spare');
      await page.mouse.click(body.x, body.y);
      await expect.poll(() => selectedNames(page)).toEqual(['spare']);
      await page.keyboard.press('ControlOrMeta+d');
      let copy;
      await expect.poll(async () => { copy = (await canvasNames(page)).find((name) => !before.includes(name)); return Boolean(copy); }, { message: `cycle ${cycle}: Ctrl+D makes a box` }).toBe(true);
      await canvasIdle(page);
      const its = await canvasPoint(page, copy);
      await page.mouse.click(its.x, its.y);
      await expect.poll(() => selectedNames(page)).toEqual([copy]);
      await page.keyboard.press('Delete');
      await expect.poll(async () => (await canvasNames(page)).includes(copy), { message: `cycle ${cycle}: ${copy} deleted` }).toBe(false);
      record.timeline.push({ cycle, ms: Date.now() - began });
      if (SAMPLED.includes(cycle)) await sample(String(cycle));
    });
  }

  await test.step('after 20 cycles: no listener, interval, frame, heap or texture left growing', async () => {
    const [two, ten, last] = SAMPLED.map((cycle) => record.samples[String(cycle)]);
    const grown = Object.entries(last.listeners).filter(([type, count]) => count > (two.listeners[type] ?? 0)).map(([type, count]) => `${type}: ${two.listeners[type] ?? 0} → ${ten.listeners[type] ?? 0} → ${count}`);
    soft(grown, 'no kind of listener held more than after cycle 2').toEqual([]);
    soft(last.intervals, `intervals: ${two.intervals} → ${ten.intervals} → ${last.intervals}`).toBeLessThanOrEqual(two.intervals);
    for (const cycle of SAMPLED) soft(record.samples[String(cycle)].framesAtRest, `no animation frame asked for in 2 s at rest, after cycle ${cycle}`).toBe(0);
    const noise = Math.max(...two.heap) - Math.min(...two.heap);
    const heapAllowed = Math.max(Math.abs(ten.heap[0] - two.heap[0]), 3 * noise);
    soft(last.heap[0] - two.heap[0], `the JS heap after cycle 20 against cycle 2 (allowed: cycles 2–10's difference, at least 3× the noise ${noise}): ${two.heap[0]} → ${ten.heap[0]} → ${last.heap[0]}`).toBeLessThanOrEqual(heapAllowed);
    // Pinned: what's drawn holds. Unpinned textures are the scene's LRU, which may fill to its stated allowance (checked below).
    for (const key of ['pinnedBytes', 'pinnedTextures', 'labels', 'estimatedLabelBytes']) {
      const allowed = Math.abs(ten.resources[key] - two.resources[key]);
      soft(last.resources[key] - two.resources[key], `the scene's ${key} after cycle 20 against cycle 2 (allowed: cycles 2–10's difference): ${two.resources[key]} → ${ten.resources[key]} → ${last.resources[key]}`).toBeLessThanOrEqual(allowed);
    }
    soft(last.nodes - two.nodes, `DOM nodes after cycle 20 against cycle 2: ${two.nodes} → ${ten.nodes} → ${last.nodes}`).toBeLessThanOrEqual(Math.abs(ten.nodes - two.nodes));
    soft(last.resources.unusedBytes, 'unpinned textures within the scene\'s allowance').toBeLessThanOrEqual(last.resources.unusedTargetBytes);
    save();
    await testInfo.attach('soak', { path: testInfo.outputPath('soak.json'), contentType: 'application/json' });
  });
});
