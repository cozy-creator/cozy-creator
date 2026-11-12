// What the canvas is drawn on, and its dots (cl-895; Paul, 11-12: "when you zoom in and out of the canvas ... you are zooming into the
// background more ... a small subtle grid of dots throughout the canvas ... in front of the background but beneath the nodes / images").
// On the landscape, the paper and dark, opened at 1 and zoomed by the control to 0.5, 0.1 and 3, in Chrome rastering on the GPU path as a
// person's does (page.js GPU): each layer of the landscape scaled by the zoom to its `--rate`, the sheet by the zoom, each laid out at a
// rung and drawn at 0.75 to 1 of it (Sheet.tsx), so Chrome neither enlarges a raster nor leaves part of it blank; the dots found in the
// picture over the background and never over a box, a step apart, small and faint; and in a 4K window, no part of the bare background left
// blank; paper's sky, whose wash comes and goes every 2048 px, faded out far out. A picture of each is kept. Then a pan's and a zoom's frames at 1 and 0.1 on each, held to rc/0.25.0's (BEFORE).
//   node canvas.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT, $CHROME, $COZY, $SHOT as see.js. Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { mkdtempSync, rmSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { GPU, STEP_MS, calcifer, importFile, importYaml, lastLook, launch, lookAt, noStall, png, projectUrl, removeGeneratedProject, settled, signIn, summary, until } = require('./page');
const { runnerSpeed } = require('./speed');

const failures = [];
const seen = { looks: 0 };
noStall(seen, died);
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  if (!ok) failures.push(name);
}

const THEMES = { landscape: ['paper', 'shown'], paper: ['paper', 'hidden'], dark: ['dark', 'shown'] };
/** Each zoom as the control reaches it from the one before: out by its −, back to 100% by Shift+0, in by its +. */
const ZOOMS = [1, 0.5, 0.1, 3];
const WINDOW = { viewport: { width: 1600, height: 1000 }, deviceScaleFactor: 1 };
const FOUR_K = { viewport: { width: 3840, height: 2160 }, deviceScaleFactor: 1 };
/** The boxes, four pictures 240 wide round the point the view is on, so at 3 a corner of each is in it, and at 0.1 all four. */
const SPOT = { x: 320, y: 275 };
const near = (a, b, by) => Math.abs(a - b) <= by;
// The page's own (ZoomControl.tsx's ZOOMS).
const STEPS = [0.05, 0.1, 0.15, 0.2, 0.25, 0.33, 0.5, 0.67, 0.75, 0.9, 1, 1.25, 1.5, 2, 2.5, 3, 4];

/**
 * rc/0.25.0's frames (e2e 43141967400: its page at 72f3b6a29 under this cell), on a runner whose speed.js read `speed_ms`: a frame now may take
 * no longer than its median and 95th, times a slower runner's speed against that one, with room for a frame's jitter.
 */
const BEFORE = {
  run: 43141967400,
  speed_ms: 25.72,
  frames: {
    'landscape_1_pan': { median_ms: 16.7, p95_ms: 16.8 },
    'landscape_1_zoom': { median_ms: 50, p95_ms: 83.4 },
    'landscape_0.1_pan': { median_ms: 16.7, p95_ms: 16.8 },
    'landscape_0.1_zoom': { median_ms: 16.7, p95_ms: 16.7 },
    'paper_1_pan': { median_ms: 16.7, p95_ms: 16.7 },
    'paper_1_zoom': { median_ms: 33.3, p95_ms: 66.6 },
    'paper_0.1_pan': { median_ms: 16.7, p95_ms: 16.7 },
    'paper_0.1_zoom': { median_ms: 16.7, p95_ms: 16.7 },
    'dark_1_pan': { median_ms: 16.7, p95_ms: 16.7 },
    'dark_1_zoom': { median_ms: 33.3, p95_ms: 50 },
    'dark_0.1_pan': { median_ms: 16.7, p95_ms: 16.7 },
    'dark_0.1_zoom': { median_ms: 16.7, p95_ms: 16.8 },
  },
};

/** The view as the page has it. */
const VIEW = () => {
  const [, x, y, zoom] = document.querySelector('.react-flow__viewport').style.transform.match(/translate\(([-\d.]+)px, ([-\d.]+)px\) scale\(([\d.]+)\)/).map(Number);
  return { x, y, zoom };
};

/** Each layer of what the canvas is drawn on, and the dots: what scales each by, as the page set it. */
const geometryOf = (page) => page.evaluate(`(() => {
  const view = (${VIEW})();
  const scaleOf = (element) => new DOMMatrixReadOnly(getComputedStyle(element).transform === 'none' ? undefined : getComputedStyle(element).transform).a;
  const number = (element, name) => parseFloat(getComputedStyle(element).getPropertyValue(name));
  const land = document.querySelector('.paper-landscape');
  const base = land && number(land, '--landscape-scale');
  const layers = land ? [...land.children].map((element) => ({
    name: element.className.replace('paper-landscape-', ''),
    rate: number(element, '--rate'),
    drawn: scaleOf(element),
    rung: element.matches('.paper-landscape-sky') ? 1 : number(element, '--landscape-scale') / base,
  })) : [];
  const sheet = document.querySelector('.paper-world');
  const sky = document.querySelector('.paper-world:not(.paper-world-plain)');
  const world = sheet && { drawn: scaleOf(sheet), rung: number(sheet, '--rung') || 1, sky: sky && Number(getComputedStyle(sky).opacity) };
  const dots = document.querySelector('.canvas-dots');
  const style = dots && getComputedStyle(dots);
  return {
    view, base, layers, world,
    dots: dots && (() => {
      const by = scaleOf(dots);
      return { shown: style.display !== 'none' && style.visibility === 'visible' && Number(style.opacity) > 0, step: parseFloat(dots.querySelector('pattern').getAttribute('width')) * by, size: 2 * parseFloat(dots.querySelector('circle').getAttribute('r')) * by, fill: style.fill };
    })(),
  };
})()`);

/**
 * Where the dots are on screen, from the layer and its pattern as drawn (not as this cell would have put them), sampled at most 48 across and
 * 32 down: each whose point the pane is topmost at (`open`, nothing over the dots there but the clear pane), or a box's picture (`box`);
 * and half a step from each open one, where no dot is.
 */
const samplesOf = (page) => page.evaluate(() => {
  const dots = document.querySelector('.canvas-dots');
  const by = new DOMMatrixReadOnly(getComputedStyle(dots).transform === 'none' ? undefined : getComputedStyle(dots).transform).a;
  const gap = parseFloat(dots.querySelector('pattern').getAttribute('width')) * by;
  const circle = dots.querySelector('circle');
  const at = dots.getBoundingClientRect();
  const [cx, cy] = [at.left + parseFloat(circle.getAttribute('cx')) * by, at.top + parseFloat(circle.getAttribute('cy')) * by];
  const kind = (x, y) => {
    const element = document.elementFromPoint(x, y);
    if (!element) return null;
    if (element.closest('.react-flow__node .media')) return 'box';
    return element.classList.contains('react-flow__pane') ? 'open' : null;
  };
  const out = { gap, open: [], boxes: [], between: [] };
  const across = Math.max(1, Math.ceil(innerWidth / gap / 48));
  const down = Math.max(1, Math.ceil(innerHeight / gap / 32));
  const first = (from, step) => from - Math.floor(from / step) * step;
  for (let y = first(cy, gap * down); y < innerHeight; y += gap * down) {
    for (let x = first(cx, gap * across); x < innerWidth; x += gap * across) {
      if (kind(x, y) !== 'open') continue;
      out.open.push([x, y]);
      if (kind(x + gap / 2, y + gap / 2) === 'open') out.between.push([x + gap / 2, y + gap / 2]);
    }
  }
  // Each box's picture, 2 px in from its edge (past what a dot beside it draws), where it's on screen and topmost.
  for (const media of document.querySelectorAll('.react-flow__node .media')) {
    const box = media.getBoundingClientRect();
    const [left, top, right, bottom] = [Math.max(0, box.left + 2), Math.max(0, box.top + 2), Math.min(innerWidth, box.right - 2), Math.min(innerHeight, box.bottom - 2)];
    if (right > left && bottom > top && kind((left + right) / 2, (top + bottom) / 2) === 'box') out.boxes.push([left, top, right, bottom]);
  }
  return out;
});

/** Hides (or shows again) what matches, as a person's page never would: for a picture of what's under it. */
const hide = (page, selector, hidden) => page.evaluate(([selector, hidden]) => {
  for (const element of document.querySelectorAll(selector)) {
    if (hidden) element.style.setProperty('visibility', 'hidden', 'important');
    else element.style.removeProperty('visibility');
  }
}, [selector, hidden]);

/** Every animation held where it is (paper's clouds drift), or let go: two pictures of one moment differ only by what's hidden between them. */
const still = (page, held) => page.evaluate((held) => { for (const animation of document.getAnimations()) held ? animation.pause() : animation.play(); }, held);

/** Two frames, then a moment for the raster: a picture after a zoom shows what a person sees once it's still. */
const drawn = async (page) => {
  await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
  await page.waitForTimeout(400);
};

/** In the decoder's page: how far each dot differs from the picture without the dots, round each point (3×3 device pixels). */
async function dotsIn({ shown, bare, samples, k }) {
  const read = async (base64) => {
    const bitmap = await createImageBitmap(await (await fetch(`data:image/png;base64,${base64}`)).blob());
    const context = new OffscreenCanvas(bitmap.width, bitmap.height).getContext('2d');
    context.drawImage(bitmap, 0, 0);
    try { return context.getImageData(0, 0, bitmap.width, bitmap.height); } finally { bitmap.close(); }
  };
  const [a, b] = await Promise.all([read(shown), read(bare)]);
  const luma = (d, i) => 0.2126 * d[i] + 0.7152 * d[i + 1] + 0.0722 * d[i + 2];
  const most = ([x, y]) => {
    let [held, light] = [0, 0];
    for (let dy = -1; dy <= 1; dy++) for (let dx = -1; dx <= 1; dx++) {
      const [px, py] = [Math.round(x * k - 0.5) + dx, Math.round(y * k - 0.5) + dy];
      if (px < 0 || py < 0 || px >= a.width || py >= a.height) continue;
      const i = (py * a.width + px) * 4;
      const by = Math.max(Math.abs(a.data[i] - b.data[i]), Math.abs(a.data[i + 1] - b.data[i + 1]), Math.abs(a.data[i + 2] - b.data[i + 2]));
      if (by > held) [held, light] = [by, Math.abs(luma(a.data, i) - luma(b.data, i))];
    }
    return [held, light];
  };
  const of = (points) => points.map(most);
  const middle = (values) => (values.length ? [...values].sort((x, y) => x - y)[values.length >> 1] : null);
  const [open, between] = [of(samples.open), of(samples.between)];
  // Under a box: every pixel of its picture the same with the dots and without.
  let [box, over] = [0, 0];
  for (const [left, top, right, bottom] of samples.boxes) {
    for (let py = Math.ceil(top * k); py < Math.floor(bottom * k); py++) {
      for (let px = Math.ceil(left * k); px < Math.floor(right * k); px++) {
        const i = (py * a.width + px) * 4;
        box++;
        if (Math.max(Math.abs(a.data[i] - b.data[i]), Math.abs(a.data[i + 1] - b.data[i + 1]), Math.abs(a.data[i + 2] - b.data[i + 2])) > 2) over++;
      }
    }
  }
  return {
    open: open.length,
    found: open.filter(([by]) => by >= 6).length,
    median_difference: middle(open.map(([by]) => by)),
    median_light: middle(open.map(([, light]) => +light.toFixed(1))),
    boxes: samples.boxes.length,
    box_pixels: box,
    over_a_box: over,
    between: between.length,
    between_drawn: between.filter(([by]) => by > 2).length,
  };
}

/** In the decoder's page: blocks of 16×16 CSS pixels of one flat colour in a picture of the background alone, as Chrome leaves what it didn't raster. */
async function blanksIn({ picture, k }) {
  const bitmap = await createImageBitmap(await (await fetch(`data:image/png;base64,${picture}`)).blob());
  const context = new OffscreenCanvas(bitmap.width, bitmap.height).getContext('2d');
  context.drawImage(bitmap, 0, 0);
  const { data, width, height } = context.getImageData(0, 0, bitmap.width, bitmap.height);
  bitmap.close();
  const side = Math.round(16 * k);
  const flat = [];
  let blocks = 0;
  for (let y = 0; y + side <= height; y += side) {
    for (let x = 0; x + side <= width; x += side) {
      blocks++;
      const first = (y * width + x) * 4;
      let same = true;
      for (let yy = y; yy < y + side && same; yy++) {
        for (let xx = x, i = (yy * width + x) * 4; xx < x + side; xx++, i += 4) {
          if (data[i] !== data[first] || data[i + 1] !== data[first + 1] || data[i + 2] !== data[first + 2]) { same = false; break; }
        }
      }
      if (same) flat.push([Math.round(x / k), Math.round(y / k)]);
    }
  }
  return { blocks, flat: flat.length, first: flat.slice(0, 8) };
}

/** A pan or a zoom by the wheel, a step each frame for `ms`, there and back: each frame's time, and how many views it went through. */
const moving = (page, kind, ms) => page.evaluate(([kind, ms]) => new Promise((done) => {
  const pane = document.querySelector('.react-flow__pane');
  const box = pane.getBoundingClientRect();
  const viewport = document.querySelector('.react-flow__viewport');
  const [frames, views] = [[], new Set()];
  let [last, sign, count] = [performance.now(), 1, 0];
  const end = last + ms;
  const step = (now) => {
    frames.push(now - last);
    last = now;
    views.add(viewport.style.transform);
    // A zoom: 20 frames out by 0.973 each (to 0.57 of where it was), 20 back. A pan: 40 frames 12 px each way.
    if (++count % (kind === 'zoom' ? 20 : 40) === 0) sign = -sign;
    const by = kind === 'zoom' ? { deltaY: sign * 20, ctrlKey: true } : { deltaX: sign * 12 };
    pane.dispatchEvent(new WheelEvent('wheel', { ...by, clientX: box.left + box.width / 2, clientY: box.top + box.height / 2, bubbles: true, cancelable: true }));
    if (now < end) requestAnimationFrame(step);
    else done({ frames: frames.slice(1), views: views.size });
  };
  requestAnimationFrame(step);
}), [kind, ms]);

(async () => {
  const base = calcifer();
  const speed = runnerSpeed();
  seen.runner = speed;
  const browser = await launch(playwright, GPU);
  const software = await launch(playwright);
  const decoder = await (await browser.newContext()).newPage();
  const errors = [];
  const watch = (tab) => {
    tab.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
    tab.on('console', (m) => { if (m.type() === 'error' && !/net::ERR_/.test(m.text())) errors.push(`console: ${m.text()}`); });
    tab.setDefaultTimeout(STEP_MS);
  };
  const first = await browser.newContext(WINDOW);
  const page = await first.newPage();
  watch(page);
  lookAt(page);
  await signIn(page);
  await page.waitForSelector('.top');
  const { token } = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session')));
  const api = async (path, init = {}) => (await fetch(base + path, { ...init, headers: { Authorization: `Bearer ${token}`, ...init.headers } })).json();
  const scratch = mkdtempSync(join(tmpdir(), 'web-canvas-'));
  const made = await api('/v1/projects', { method: 'POST', headers: { 'Content-Type': 'application/json', 'Idempotency-Key': `canvas-${Date.now()}` }, body: JSON.stringify({ name: `web-canvas-${Date.now()}`, unencrypted: true }) });
  const contexts = [first];
  try {
    writeFileSync(join(scratch, 'a.png'), png(90));
    importFile(made.path, join(scratch, 'a.png'), 'a_picture');
    const held = await api(`/v1/projects/${made.id}`);
    const hash = held.assets.find((row) => row.name === 'a_picture').sha256.replace('sha256:', '').slice(0, 12);
    let text = 'version: 4\nassets:\n';
    const cards = {};
    for (const [i, [x, y]] of [[0, 0], [400, 0], [0, 400], [400, 400]].entries()) {
      text += `  n${i}:\n    approved: "${hash}"\n    tool: see-no-such-package/draw\n    instructions: box ${i}\n`;
      cards[`n${i}`] = { x, y, w: 240 };
    }
    importYaml(made.path, text);
    await api(`/v1/projects/${made.id}/canvas`, { method: 'PATCH', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ cards }) });
    // The import is the project's whole list: the picture stays a file, each box approved on it.
    await until('four boxes read', async () => (await api(`/v1/projects/${made.id}`)).assets?.length, (n) => n === 4);
    const url = await projectUrl(made.id);

    /** A tab on `theme` in `context`, the view on SPOT at `zoom`, once its pictures are in. */
    const open = async (context, theme, zoom) => {
      const tab = await context.newPage();
      watch(tab);
      lookAt(tab);
      const { width, height } = tab.viewportSize();
      const view = { x: width / 2 - SPOT.x * zoom, y: height / 2 - SPOT.y * zoom, zoom };
      await tab.addInitScript(([id, view, [look, land]]) => {
        localStorage.setItem(`cozy.view:${id}`, JSON.stringify(view));
        localStorage.setItem('cozy.theme', look);
        localStorage.setItem('cozy.landscape', land);
      }, [made.id, view, THEMES[theme]]);
      await tab.goto(url);
      await tab.waitForSelector('.react-flow__node .media img');
      await until('every picture drawn', () => tab.evaluate(() => [...document.querySelectorAll('.react-flow__node .media img')].every((image) => image.complete)), Boolean);
      await settled(tab);
      await drawn(tab);
      return tab;
    };
    /** The control from where the view is to `zoom`, a step at a time, each let settle. */
    const zoomTo = async (tab, zoom) => {
      for (let i = 0; i < 20; i++) {
        const now = (await tab.evaluate(VIEW)).zoom;
        if (near(now, zoom, 0.001)) break;
        if (zoom >= 1 && now < 1) await tab.keyboard.press('Shift+Digit0');
        else await tab.click(`.zoom button[aria-label="${zoom < now ? 'Zoom out' : 'Zoom in'}"]`);
        // A slow frame holds the view still part way: it's there once it has left where it was and is on one of the control's steps.
        await until('the zoom moved', async () => (await tab.evaluate(VIEW)).zoom, (at) => !near(at, now, 0.0005));
        await until('the zoom on a step', async () => (await tab.evaluate(VIEW)).zoom, (at) => STEPS.some((step) => near(at, step, 0.0005)));
        await settled(tab);
      }
      await drawn(tab);
      return (await tab.evaluate(VIEW)).zoom;
    };
    /** What the background's layers are scaled by against what the zoom asks of each, and each drawn at 0.75 to 1 of its rung. */
    const scaledRight = (geometry, theme) => {
      const zoom = geometry.view.zoom;
      const each = theme === 'landscape'
        ? geometry.layers.map(({ name, rate, drawn, rung }) => ({ name, want: zoom ** rate, got: drawn * rung, drawn, sky: name === 'sky' }))
        : geometry.world ? [{ name: 'sheet', want: zoom, got: geometry.world.drawn * geometry.world.rung, drawn: geometry.world.drawn }] : [];
      const wrong = each.filter(({ want, got, drawn, sky }) => !(near(got / want, 1, 0.01) && (sky ? drawn > 0.9 && drawn < 1.05 : drawn > 0.749 && drawn <= 1.001)));
      return { ok: each.length > 0 && wrong.length === 0, each: each.map(({ name, want, got, drawn }) => ({ name, want: +want.toFixed(3), got: +got.toFixed(3), drawn: +drawn.toFixed(3) })) };
    };
    const pictureOf = async (tab) => (await tab.screenshot()).toString('base64');
    /** A picture of the background alone: everything beside it on its way up to the page, at no opacity (what sets its own visibility shows past a hidden parent). */
    const bareOf = async (tab) => {
      await tab.evaluate(() => {
        const kept = [...document.querySelectorAll('.paper-landscape, .paper-world')];
        const path = new Set(kept.flatMap((element) => { const up = []; for (let at = element.parentElement; at; at = at.parentElement) up.push(at); return up; }));
        window.bare = [...path].flatMap((at) => [...at.children].filter((child) => !path.has(child) && !kept.includes(child)));
        for (const element of window.bare) element.style.setProperty('opacity', '0', 'important');
      });
      await drawn(tab);
      const picture = await tab.screenshot();
      await tab.evaluate(() => { for (const element of window.bare) element.style.removeProperty('opacity'); });
      return picture;
    };

    seen.zooms = {};
    for (const theme of Object.keys(THEMES)) {
      const context = theme === 'landscape' ? first : await browser.newContext(WINDOW);
      if (context !== first) contexts.push(context);
      if (context !== first) await signIn(await context.newPage());
      const tab = await open(context, theme, 1);
      for (const zoom of ZOOMS) {
        const at = await zoomTo(tab, zoom);
        const name = `${theme}_${zoom}`;
        await still(tab, true);
        const geometry = await geometryOf(tab);
        const scaled = scaledRight(geometry, theme);
        check(`${name}_background_scaled`, near(at, zoom, 0.001) && scaled.ok, { at, ...scaled });
        // Paper's sky comes and goes down the sheet every 2048 px: in close it's on, far out it's faded to the plain sheet under it.
        if (theme === 'paper') check(`${name}_sky_faded_far_out`, geometry.world?.sky === (at >= 0.6 ? 1 : at <= 0.3 ? 0 : geometry.world?.sky), geometry.world);
        let gap = 24 * at;
        while (gap < 12) gap *= 2;
        check(`${name}_dots_shown`, geometry.dots?.shown && near(geometry.dots.step, gap, 0.01) && geometry.dots.size > 1.8 && geometry.dots.size <= 2.001, geometry.dots);
        const samples = await samplesOf(tab);
        const shown = await tab.screenshot();
        if (process.env.SHOT) writeFileSync(`${process.env.SHOT}-${theme}-${zoom}.png`, shown);
        await hide(tab, '.canvas-dots', true);
        await drawn(tab);
        const bare = await tab.screenshot();
        await hide(tab, '.canvas-dots', false);
        const dots = await decoder.evaluate(dotsIn, { shown: shown.toString('base64'), bare: bare.toString('base64'), samples, k: WINDOW.deviceScaleFactor });
        seen.looks++;
        // Over the background: nearly every open point shows a dot, faint (a channel 6 to 60 off what's under it); never over a box's
        // picture; and half a step from each, nothing.
        check(`${name}_dots_over_the_background`, dots.open >= 20 && dots.found >= 0.9 * dots.open && dots.median_difference <= 60, dots);
        check(`${name}_dots_under_the_boxes`, dots.boxes >= 1 && dots.box_pixels > 0 && dots.over_a_box === 0, dots);
        check(`${name}_dots_are_dots`, dots.between >= 10 && dots.between_drawn <= 0.05 * dots.between, dots);
        if (theme !== 'dark') {
          // Dark's sheet is its stage's colour, its grain a level or two far out: a block left blank can't be told from it there.
          const blanks = await decoder.evaluate(blanksIn, { picture: (await bareOf(tab)).toString('base64'), k: WINDOW.deviceScaleFactor });
          check(`${name}_no_blank`, blanks.flat === 0, blanks);
        }
        await still(tab, false);
        seen.zooms[name] = { at, geometry: scaled.each, sky: geometry.world?.sky, dots };
      }
      await tab.close();

      // The same in a 4K window, zoomed there the same way: nothing of the background left blank; a picture kept of each.
      const large = await browser.newContext(FOUR_K);
      contexts.push(large);
      await signIn(await large.newPage());
      const big = await open(large, theme, 1);
      for (const zoom of ZOOMS) {
        const at = await zoomTo(big, zoom);
        const name = `${theme}_${zoom}_4k`;
        const scaled = scaledRight(await geometryOf(big), theme);
        check(`${name}_background_scaled`, near(at, zoom, 0.001) && scaled.ok, { at, ...scaled });
        if (process.env.SHOT) await big.screenshot({ path: `${process.env.SHOT}-${theme}-${zoom}-4k.jpg`, type: 'jpeg', quality: 85 });
        if (theme !== 'dark') {
          const blanks = await decoder.evaluate(blanksIn, { picture: (await bareOf(big)).toString('base64'), k: FOUR_K.deviceScaleFactor });
          check(`${name}_no_blank`, blanks.flat === 0, blanks);
        }
        seen.looks++;
      }
      await large.close();

      // A pan's and a zoom's frames, at 1 and at 0.1, in Chrome compositing as headless does by itself (as see.js and zoom.js time a pan):
      // SwiftShader draws each layer in software, a fifth of a second a frame here, and so few frames tell nothing.
      seen.frames ??= {};
      const timing = await software.newContext(WINDOW);
      contexts.push(timing);
      await signIn(await timing.newPage());
      for (const zoom of [1, 0.1]) {
        const timed = await open(timing, theme, zoom);
        for (const kind of ['pan', 'zoom']) {
          const { frames, views } = await moving(timed, kind, 3000);
          const name = `${theme}_${zoom}_${kind}`;
          seen.frames[name] = { views, ...summary(frames) };
          check(`${name}_moved`, views > 10, views);
          await settled(timed);
        }
        await timed.close();
      }
      await timing.close();
    }

    // No slower than before: each run of frames against rc/0.25.0's on its runner, by this runner's speed against that one's.
    if (BEFORE) {
      // A frame is a whole number of the display's: a faster runner than that one's holds to the same bars.
      const by = Math.max(1, speed.median_ms / BEFORE.speed_ms);
      const slower = Object.entries(seen.frames).filter(([name, now]) => {
        const then = BEFORE.frames[name];
        return then && (now.median_ms > then.median_ms * by * 1.1 + 2 || now.p95_ms > then.p95_ms * by * 1.25 + 4);
      });
      seen.frames_against = { run: BEFORE.run, by: +by.toFixed(2) };
      check('frames_no_slower', slower.length === 0, Object.fromEntries(slower.map(([name, now]) => [name, { now, before: BEFORE.frames[name] }])));
    } else seen.frames_against = 'no baseline in canvas.js (BEFORE)';
  } catch (error) {
    await lastLook();
    throw error;
  } finally {
    for (const context of contexts) await context.close().catch(() => {});
    await removeGeneratedProject(made.path, 'web-canvas-');
    rmSync(scratch, { recursive: true, force: true });
  }

  check('no_page_errors', errors.length === 0, errors);
  console.log(JSON.stringify({ failures, seen }, null, 2));
  await browser.close();
  await software.close();
  process.exit(failures.length ? 1 : 0);
})().catch(died);

async function died(error) {
  console.error(error);
  await lastLook();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(2);
}
