// layout (cl-973 core cell 18), at 1600×1000 on Chrome's GPU raster path with its scrollbars drawn: a box of every kind in every state, each where
// the API put it, looked at 0.25, 0.5, 1 and 2, then one open in Focus. No box or label lies on another; nothing is cut off, nothing that floats
// over the canvas lies on another, and Focus is under none of it; no scrollbar anywhere. Words read at 10 px or more (a box's at 100%: they're
// drawn at the canvas's scale, cl-890); a prompt over a picture is bold, 4.5:1 or more against its surface (cl-901). A render's Cancel shows at
// 1 and at 0.5, and the Focus frame fits the window. A name cut at rest says itself whole under the pointer (cl-1028 row 18). Each finding
// names its rule, where, and what (layout.js's rules, measured).
const { randomUUID } = require('crypto');
const { test, expect, canvasCard, canvasNames, canvasIdle, hoverBox, focus, fixtureTool, middle, nameUnderPointer, newCanvas, observeStill, openPoint, screenshotPixels, MP4, png } = require('./fixtures');

test.use({
  launchOptions: {
    executablePath: process.env.CHROME,
    // Scrollbars drawn, as a person's are; rastered on the GPU path, in software here.
    ignoreDefaultArgs: ['--hide-scrollbars'],
    args: ['--use-angle=swiftshader', '--enable-unsafe-swiftshader', '--enable-gpu-rasterization', '--ignore-gpu-blocklist'],
  },
});

const LONG = 'a_box_whose_name_goes_on_and_on_past_any_width';
const COLOURS = [[0, 0, 0], [255, 255, 255], [255, 210, 0], [30, 90, 255], [240, 40, 40], [250, 245, 235]];
/** The least a word may be drawn, in CSS pixels: on screen, or a box's at 100%. */
const FLOOR = 10;

/** What's wrong with how the page is laid out, measured in it: each finding a rule, where, and what (from layout.js's audit). */
function audit({ floor, focused }) {
  const found = [];
  const R = (el) => el.getBoundingClientRect();
  const root = document.querySelector('.canvas-root'), inspection = root?.canvasInspection;
  const zoom = inspection?.camera().zoom ?? 1;
  const name = (el) => el.closest?.('.canvas-node')?.querySelector('.card')?.dataset.asset ?? el.closest?.('[data-focused-asset]')?.dataset.focusedAsset;
  const where = (el) => {
    const path = [];
    for (let e = el; e && e !== document.body && path.length < 3; e = e.parentElement) {
      const label = e.getAttribute('aria-label');
      path.unshift(`${e.tagName.toLowerCase()}${[...e.classList].slice(0, 2).map((c) => `.${c}`).join('')}${label ? `[${label.slice(0, 24)}]` : ''}`);
    }
    const held = name(el);
    return `${held ? `<${held}> ` : ''}${path.join(' > ')}`;
  };
  const say = (rule, el, what) => found.push(`${rule}: ${where(el)}: ${what}`);
  const round = (r) => [Math.round(r.left), Math.round(r.top), Math.round(r.width), Math.round(r.height)];
  const visible = (el) => el.checkVisibility?.({ opacityProperty: true, visibilityProperty: true }) && R(el).width >= 1 && R(el).height >= 1;
  const onScreen = (r) => r.right > 0 && r.bottom > 0 && r.left < innerWidth && r.top < innerHeight;
  const meet = (a, b, by = 1) => Math.min(a.right, b.right) - Math.max(a.left, b.left) > by && Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top) > by;
  const within = (a, b, by = 1) => a.left >= b.left - by && a.top >= b.top - by && a.right <= b.right + by && a.bottom <= b.bottom + by;
  const scaleOf = (el) => (el.offsetWidth ? R(el).width / el.offsetWidth : el.offsetHeight ? R(el).height / el.offsetHeight : 1);
  const ownText = (el) => [...el.childNodes].some((n) => n.nodeType === 3 && n.textContent.trim());
  const scrolled = (el, panel) => {
    for (let e = el.parentElement; e && e !== panel; e = e.parentElement) if (/auto|scroll/.test(getComputedStyle(e).overflowY + getComputedStyle(e).overflowX)) return true;
    return false;
  };
  const PANELS = '.here-under, .here-side, .drawer, .top, .tools, .zoom, .zoom-menu, .main-menu nav, .asset-menu, .name-menu, .activity, .notes .note, .ptabs';
  const CONTROLS = 'button, input, select, textarea, summary, a.tb, .chip, .add-on, [role=slider], [contenteditable=true]';
  const OVER_PICTURE = '.media, .tl-film, .take, .slot-media';

  // Words: legible, and not cut off unless an ellipsis says so; no line cut through.
  for (const el of [...document.querySelectorAll('body *')].filter((one) => ownText(one) && visible(one) && onScreen(R(one)) && !one.closest('.ptabs-ghost, .tool-ghost, [aria-hidden=true]'))) {
    const style = getComputedStyle(el);
    const onCanvas = Boolean(el.closest('.canvas-node'));
    const px = (parseFloat(style.fontSize) * scaleOf(el)) / (onCanvas ? zoom : 1);
    if (px < floor) say('illegible', el, `${px.toFixed(1)} px ${onCanvas ? 'at 100%' : 'on screen'}: "${el.textContent.trim().slice(0, 30)}"`);
    const ellipsis = style.textOverflow === 'ellipsis' && !/flex|grid/.test(style.display);
    if (/hidden|clip/.test(style.overflowX) && el.scrollWidth > el.clientWidth + 1 && !ellipsis) say('clipped', el, `its text is ${el.scrollWidth} wide in ${el.clientWidth}, cut with no ellipsis`);
    const line = parseFloat(style.lineHeight);
    if (/hidden|clip/.test(style.overflowY) && line && el.scrollHeight > el.clientHeight + 1) {
      const cut = (el.clientHeight - parseFloat(style.paddingTop)) % line;
      if (cut > 1 && cut < line - 1) say('line-cut', el, `its last line shows ${Math.round((cut / line) * 100)}% of itself`);
    }
  }
  // Each panel: what's in it inside it unless it scrolls, and no two of its controls on each other.
  for (const panel of document.querySelectorAll(PANELS)) {
    if (!visible(panel)) continue;
    const ps = getComputedStyle(panel);
    const scrolls = /auto|scroll/.test(ps.overflowY) || /auto|scroll/.test(ps.overflowX);
    const controls = [...panel.querySelectorAll(CONTROLS)].filter((el) => visible(el) && el.closest(PANELS) === panel);
    for (const el of controls) if (!within(R(el), R(panel), 1) && !scrolls && !scrolled(el, panel)) say('outside', el, `${round(R(el))} past its panel ${round(R(panel))}`);
    for (let i = 0; i < controls.length; i++) for (let j = i + 1; j < controls.length; j++) {
      const [a, b] = [controls[i], controls[j]];
      if (a.contains(b) || b.contains(a)) continue;
      if ((a.matches('.take-off') && b.closest('.take-at') === a.parentElement) || (b.matches('.take-off') && a.closest('.take-at') === b.parentElement)) continue;
      if (a.closest(OVER_PICTURE) && a.closest(OVER_PICTURE) === b.closest(OVER_PICTURE)) continue;
      if (meet(R(a), R(b))) say('overlap', a, `on ${where(b)}: ${round(R(a))} and ${round(R(b))}`);
    }
  }
  // Actual retained labels have no DOM boxes. Judge their measured glyph bounds, declared font and drawn text.
  const rect = r => ({ ...r, left: r.x, top: r.y, right: r.x + r.width, bottom: r.y + r.height });
  const norm = text => text.replace(/\s+/g, ' ').trim();
  const pixels = [];
  const boxes = [];
  // What floats over the canvas covers a label whether or not it takes the pointer (a note is pointer-events: none).
  const covers = [...document.querySelectorAll('.notes .note, .tools, .zoom, .zoom-menu, .top, .drawer, .activity')].filter(visible).map(R);
  const inspectFailure = (rule, asset, part, what) => found.push(`${rule}: <${asset}> ${part}: ${what}`);
  for (const asset of inspection?.names() ?? []) {
    const card = inspection.card(asset);
    const native = document.querySelector(`.canvas-node:has(.card[data-asset=${JSON.stringify(asset)}])`);
    if (native && visible(native) && !native.querySelector('.card.here') && !native.classList.contains('dragging')) {
      const hung = [native, ...native.querySelectorAll('.head > *, .ratio-rest, .tile-under, .refused-line')].filter(visible)
        .map(el => ({ part: where(el), rect: R(el) }));
      if (onScreen(R(native))) boxes.push({ name: asset, hung });
      continue;
    }
    const painted = card?.rendered;
    if (!painted && !focused && card?.bounds && onScreen(rect(card.bounds))) inspectFailure('not-presented', asset, 'card', 'its visible bounds have no scene or native card');
    if (!painted || painted.dragging) continue;
    const bounds = rect(card.bounds), body = rect(painted.body);
    const labels = Object.entries(painted.labels).filter(([, label]) => label.text && label.width >= 1 && label.height >= 1);
    const hung = [{ part: 'card', rect: bounds }, ...labels.map(([key, label]) => ({ part: key, rect: rect(label) })),
      ...painted.controls.map(control => ({ part: control.part, rect: rect(control) }))];
    if (painted.stateDot) hung.push({ part: 'state dot', rect: rect(painted.stateDot) });
    if (onScreen(bounds)) boxes.push({ name: asset, hung });
    for (const [key, label] of labels) {
      const at = rect(label);
      if (!onScreen(at)) continue;
      if (label.fontSize < floor) inspectFailure('illegible', asset, key, `${label.fontSize.toFixed(1)} px at 100%: "${label.text.slice(0, 30)}"`);
      if (norm(label.text) !== norm(label.fullText) && !label.text.trimEnd().endsWith('…')) inspectFailure('clipped', asset, key, `cut with no ellipsis: "${label.text}"`);
      if (at.left < bounds.left - 1 || at.right > bounds.right + 1) inspectFailure('clipped', asset, key, `${round(at)} past the box's width ${round(bounds)}`);
      // Labels intentionally above/below the body stay outside it. Text that starts inside may not have a partial last line outside it.
      if (at.top >= body.top && at.top < body.bottom && at.bottom > body.bottom + 1) inspectFailure('line-cut', asset, key, `${round(at)} past the picture/text body's bottom ${Math.round(body.bottom)}`);
      if ((key === 'title' || key === 'cancel' || key.startsWith('words')) && label.fontSize * zoom >= 8
        && within(at, { left: 0, top: 0, right: innerWidth, bottom: innerHeight }, 0) && !covers.some((over) => meet(at, over, 0))
        && root.contains(document.elementFromPoint(at.left + at.width / 2, at.top + at.height / 2))) pixels.push({ name: asset, key, ...label });
    }
    for (let i = 0; i < painted.controls.length; i++) for (let j = i + 1; j < painted.controls.length; j++) {
      const [a, b] = [painted.controls[i], painted.controls[j]];
      if (meet(rect(a), rect(b))) inspectFailure('overlap', asset, a.part, `on ${b.part}: ${round(rect(a))} and ${round(rect(b))}`);
    }
    // A box's own Delete and Cancel: within its width (Cancel hangs under it, with the render's line), and a press on one is that control of
    // that box. Where its hit area ends is the scene's choice.
    for (const control of painted.controls) {
      const at = rect(control), middle = { x: at.left + at.width / 2, y: at.top + at.height / 2 };
      if (middle.x < bounds.left || middle.x > bounds.right) inspectFailure('outside', asset, control.part, `its middle ${Math.round(middle.x)},${Math.round(middle.y)} past the box's width ${round(bounds)}`);
      else if (within(at, { left: 0, top: 0, right: innerWidth, bottom: innerHeight }, 0) && !covers.some((over) => meet(at, over, 0))) {
        const hit = inspection.hitTest(middle);
        if (hit?.name !== asset || hit?.part !== control.part) inspectFailure('unreachable', asset, control.part, `a press on its middle is ${JSON.stringify(hit)}`);
      }
    }
  }
  // At rest, no box on another, including its title, metadata, errors and render controls hanging above/below it.
  for (let i = 0; i < boxes.length; i++) for (let j = i + 1; j < boxes.length; j++) {
    const [a, b] = [boxes[i], boxes[j]];
    const overlap = a.hung.flatMap(ea => b.hung.filter(eb => meet(ea.rect, eb.rect)).map(eb => [ea, eb]))[0];
    if (overlap) inspectFailure('boxes-overlap', a.name, overlap[0].part, `with <${b.name}>: ${round(overlap[0].rect)} on ${overlap[1].part} ${round(overlap[1].rect)}`);
  }
  // What floats over the canvas, on nothing else that floats; in Focus, its frame and heading under none of it.
  const floats = ['.top .history', '.top .ptabs', '.top .mtabs', '.top .canvas-controls', '.top .main-menu > summary', '.top .pill', '.tools', '.zoom', '.notes'].flatMap((s) => [...document.querySelectorAll(s)]).filter(visible);
  for (let i = 0; i < floats.length; i++) for (let j = i + 1; j < floats.length; j++) if (meet(R(floats[i]), R(floats[j]))) say('chrome-overlap', floats[i], `on ${where(floats[j])}`);
  if (focused) {
    for (const el of document.querySelectorAll('.focus-image-transform > .media, .focus-heading, .here-under, .here-side')) {
      if (!visible(el)) continue;
      for (const over of document.querySelectorAll('.top .history, .top .ptabs, .top .mtabs, .top .main-menu > summary, .tools, .zoom')) if (visible(over) && meet(R(el), R(over), 2)) say('under-chrome', el, `under ${where(over)}`);
    }
    const frame = document.querySelector('.focus-viewer');
    if (!frame || !within(R(frame), { left: 0, top: 0, right: innerWidth, bottom: innerHeight }, 0.5)) say('frame-outside', frame ?? document.body, `Focus's frame ${frame ? round(R(frame)) : 'missing'} past the window ${innerWidth}×${innerHeight}`);
  }
  // No bar on what scrolls, and the page itself doesn't scroll.
  for (const el of document.querySelectorAll('body *')) {
    const s = getComputedStyle(el);
    if (!/auto|scroll/.test(s.overflowX + s.overflowY) || !visible(el)) continue;
    const bar = Math.max(el.offsetWidth - el.clientWidth - parseFloat(s.borderLeftWidth) - parseFloat(s.borderRightWidth), el.offsetHeight - el.clientHeight - parseFloat(s.borderTopWidth) - parseFloat(s.borderBottomWidth));
    if (bar > 0.5) say('scrollbar', el, `a ${Math.round(bar)} px bar`);
  }
  const page = document.scrollingElement;
  if (page.scrollWidth > innerWidth + 0.5 || page.scrollHeight > innerHeight + 0.5) say('page-scrolls', page, `${page.scrollWidth}×${page.scrollHeight} in ${innerWidth}×${innerHeight}`);
  const probe = document.createElement('i'); probe.style.color = 'var(--ink)'; document.body.append(probe);
  const css = getComputedStyle(probe).color, values = css.match(/[\d.]+/g).map(Number).slice(0, 3);
  probe.remove();
  return { found, pixels, ink: css.startsWith('color(') ? values.map(value => value * 255) : values, dpr: devicePixelRatio };
}

/** How a prompt's words are drawn: weight, and their contrast with the surface under them, each colour as the page computes it, in sRGB. */
function legibility(el) {
  const canvas = new OffscreenCanvas(1, 1);
  const context = canvas.getContext('2d', { willReadFrequently: true });
  const rgba = (colour) => {
    context.clearRect(0, 0, 1, 1);
    context.fillStyle = colour;
    context.fillRect(0, 0, 1, 1);
    return [...context.getImageData(0, 0, 1, 1).data];
  };
  let surface = el;
  while (surface && rgba(getComputedStyle(surface).backgroundColor)[3] === 0) surface = surface.parentElement;
  const [text, under] = [rgba(getComputedStyle(el).color), rgba(getComputedStyle(surface ?? document.body).backgroundColor)];
  const luminance = ([r, g, b]) => {
    const [R, G, B] = [r, g, b].map((v) => (v / 255 <= 0.04045 ? v / 255 / 12.92 : ((v / 255 + 0.055) / 1.055) ** 2.4));
    return 0.2126 * R + 0.7152 * G + 0.0722 * B;
  };
  const [a, b] = [luminance(text), luminance(under)];
  return { weight: Number(getComputedStyle(el).fontWeight), text, under, surface: surface?.className ?? null, contrast: Number(((Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05)).toFixed(2)) };
}

/** Read real foreground pixels within the reported label bounds. Layout metadata alone cannot prove a label is painted. */
function textPixels(read, labels, ink, dpr) {
  return labels.filter(label => {
    for (let y = Math.floor(label.y * dpr); y < Math.ceil((label.y + label.height) * dpr); y++) {
      for (let x = Math.floor(label.x * dpr); x < Math.ceil((label.x + label.width) * dpr); x++) {
        if (read(x, y).every((value, i) => Math.abs(value - ink[i]) <= 40)) return false;
      }
    }
    return true;
  }).map(label => `${label.name ?? 'making'}:${label.key ?? 'cancel'}`);
}

async function passive(page) {
  // On open canvas, so no box is under the pointer at any zoom and none mounts its card; in Focus the canvas takes no hover.
  const { x, y } = (await page.locator('[data-focused-asset]').count()) ? { x: 2, y: 500 } : await openPoint(page);
  await page.mouse.move(x, y);
  await expect(page.locator('.canvas-node')).toHaveCount(0);
  // Runs/loading may animate forever. Only finite arrival/movement must settle before measuring geometry.
  await expect.poll(() => page.evaluate(() => {
    const inspection = document.querySelector('.canvas-root')?.canvasInspection;
    return inspection?.names().filter(name => { const card = inspection.card(name)?.rendered; return card?.landing?.active || card?.glide?.active; }) ?? ['surface unavailable'];
  }), { message: 'passive cards have finished arriving and moving' }).toEqual([]);
  await page.evaluate(() => new Promise(done => requestAnimationFrame(() => requestAnimationFrame(done))));
}

test('layout', async ({ page, project, api }) => {
  const { tools } = await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}`);
  const qwen = tools.find((tool) => tool.package === 'qwen-image-2' && tool.makes === 'image' && tool.fields.some((field) => field.type === 'prompt'));
  const shotTool = tools.find((tool) => tool.id === 'minimax-h3/generate') ?? tools.find((tool) => tool.package === 'minimax-h3' && tool.makes === 'video');
  expect(qwen && shotTool, 'the Hub lists Qwen Image 2 and H3').toBeTruthy();
  const at = (x, y) => ({ x, y });
  // Three rows 440 apart, columns 420 apart, and a stack of two at the right.
  await project.add({ name: 'blank', kind: 'image', fields: { node_kind: 'image' }, at: at(0, 0) });
  await project.import(png(90), { name: LONG, at: at(420, 0) });
  await project.add({ name: 'failed', tool: fixtureTool('shots.py/crash'), kind: 'video', fields: { crash: "the fixture's own failure, said in full so it wraps" }, at: at(840, 0) });
  await project.import(png(130), { name: 'stacked_1', at: at(1260, 0) });
  await project.import(png(170), { name: 'stacked_2', at: at(1260, 260) });
  await project.import(MP4, { name: 'clip', at: at(0, 440) });
  await project.add({ name: 'making', tool: fixtureTool('tool.py/slow'), kind: 'image', fields: { instructions: 'slowly', seconds: 120 }, at: at(420, 440) });
  await project.add({ name: 'shot', tool: shotTool.id, kind: 'video', fields: { summary: 'a lighthouse at dusk', beats: [{ description: 'Waves on black rocks.', duration: 5 }] }, at: at(840, 440) });
  await api.post(project.at('/assets/shot/versions'), { source: await project.upload(MP4) });
  await project.add({ name: 'cut', tool: 'cozy/edit', kind: 'video', fields: { node_kind: 'video' }, at: at(0, 880) });
  await project.add({ name: 'words', kind: 'text', fields: { text: 'a quiet harbour at dawn, boats tied along the wall, gulls over the lamp' }, at: at(420, 880) });
  await project.import(png(0, 640, 400, false, (x, y) => COLOURS[(((x >> 3) ^ (y >> 4)) + (x >> 5)) % COLOURS.length]), { name: 'busy', at: at(840, 880) });
  await api.patch(project.at('/assets/busy'), { tool: qwen.id });
  const names = ['blank', LONG, 'failed', 'stacked_1', 'stacked_2', 'clip', 'making', 'shot', 'cut', 'words', 'busy'];
  await expect.poll(() => canvasNames(page)).toEqual(expect.arrayContaining(names));

  // A render that fails (the crash fixture), then one being made all through the looking: local renders one at a time. Both ended after.
  // Only where this computer runs models, as Calcifer says (Linux's runners; macOS and Windows runners don't): elsewhere both stay unrendered.
  const { local_usable: renders } = await api.get('/v1/status');
  const render = (name) => api.call('POST', project.at('/runs'), { names: [name], on: ['local'] }, { headers: { 'Idempotency-Key': randomUUID() } });
  const face = page.getByRole('status', { name: "making's render" });
  try {
    if (renders) {
      await render('failed');
      await expect.poll(async () => {
        const held = await project.asset('failed');
        return Boolean(held?.failure || held?.takes?.some((take) => take.failure));
      }, { message: 'failed fails', timeout: 60_000 }).toBe(true);
      await render('making');
      await expect.poll(async () => Boolean((await canvasCard(page, 'making'))?.rendered?.progress), { message: 'making is being made on the canvas', timeout: 60_000 }).toBe(true);
    }
    const zoomButton = page.getByRole('button', { name: /more zoom options$/ });
    const zoomMenu = async (item) => {
      await zoomButton.click();
      await page.getByRole('menuitem', { name: new RegExp(`^${item}`) }).click();
      await canvasIdle(page);
    };
    /** Looked at once what animates has finished: every finding, said whole. */
    const looked = async (what, focused = false) => {
      await expect.poll(() => page.evaluate(() => document.getAnimations().filter((one) => one.playState === 'running' && one.timeline === document.timeline && one.effect?.getComputedTiming().iterations !== Infinity).length), { message: `${what}: still` }).toBe(0);
      await passive(page);
      const look = await page.evaluate(audit, { floor: FLOOR, focused });
      expect.soft(look.found, `${what}: laid out`).toEqual([]);
      if (!focused) expect.soft(look.pixels.length, `${what}: passive labels are available to inspect in the screenshot`).toBeGreaterThan(0);
      const read = screenshotPixels(await page.screenshot());
      expect.soft(textPixels(read, look.pixels, look.ink, look.dpr), `${what}: readable passive labels have text pixels`).toEqual([]);
    };
    const cancelShows = async (zoom) => {
      if (!renders) return;
      await passive(page);
      let painted;
      await expect.poll(async () => {
        painted = (await canvasCard(page, 'making'))?.rendered;
        return Boolean(painted?.controls.some(control => control.part === 'cancel') && painted.labels.cancel?.text === 'Cancel');
      }, { message: `Cancel is actually in the passive scene at ${zoom}` }).toBe(true);
      const r = painted.controls.find(control => control.part === 'cancel');
      expect.soft(Boolean(r) && r.x >= 0 && r.y >= 0 && r.x + r.width <= 1600 && r.y + r.height <= 1000, `and on screen at ${zoom}: ${JSON.stringify(r)}`).toBe(true);
      const look = await page.evaluate(() => {
        const root = document.querySelector('.canvas-root'), inspection = root.canvasInspection;
        const control = inspection.card('making').rendered.controls.find(control => control.part === 'cancel');
        const hit = inspection.hitTest({ x: control.x + control.width / 2, y: control.y + control.height / 2 });
        const probe = document.createElement('i'); probe.style.color = 'var(--ink)'; document.body.append(probe);
        const css = getComputedStyle(probe).color, values = css.match(/[\d.]+/g).map(Number).slice(0, 3);
        probe.remove();
        return { hit, ink: css.startsWith('color(') ? values.map(value => value * 255) : values, dpr: devicePixelRatio };
      });
      expect.soft(look.hit, `Cancel is reachable at ${zoom}`).toEqual({ name: 'making', part: 'cancel' });
      expect.soft(textPixels(screenshotPixels(await page.screenshot()), [painted.labels.cancel], look.ink, look.dpr), `Cancel has actual text pixels at ${zoom}`).toEqual([]);
      // The sparse native control still has its accessible role when the person physically hovers the box (the old canvas's hover card;
      // the new canvas draws Cancel, judged above by hitTest and its pixels, and names no DOM button for it).
      await hoverBox(page, 'making');
      if (await newCanvas(page)) test.info().annotations.push({ type: 'no DOM control', description: `the new canvas draws making's Cancel at ${zoom}: its role needs an accessible action on its .canvas-cards option` });
      else {
        const cancel = face.getByRole('button', { name: "Cancel making's render" });
        await expect.soft(cancel, `Cancel shows on the rendering face at ${zoom}`).toBeVisible();
        const native = await cancel.boundingBox();
        expect.soft(Boolean(native) && native.x >= 0 && native.y >= 0 && native.x + native.width <= 1600 && native.y + native.height <= 1000, `the native Cancel is on screen at ${zoom}: ${JSON.stringify(native)}`).toBe(true);
      }
      await passive(page);
    };

    await test.step('view at 0.25, 0.5, 1 and 2', async () => {
      await zoomMenu('Zoom to fit');
      await zoomMenu('Zoom to 50%');
      for (let i = 0; i < 2; i++) await page.getByRole('button', { name: 'Zoom out', exact: true }).click();
      await canvasIdle(page);
      await expect(zoomButton).toHaveText('25%');
      await page.mouse.move(2, 500);
      await looked('at 0.25');
      await zoomMenu('Zoom to 50%');
      await looked('at 0.5');
      await cancelShows(0.5);
      await zoomMenu('Zoom to 100%');
      await looked('at 1');
      await cancelShows(1);
      await zoomMenu('Zoom to 200%');
      await looked('at 2');
      await zoomMenu('Zoom to fit');
    });

    await test.step('a name cut at rest says itself whole under the pointer, and nothing else moves', async () => {
      // cl-1028 acceptance row 18 (Paul's): the long name, cut with an ellipsis where its box ends at rest; hovered, the whole of it.
      await zoomMenu('Zoom to fit');
      await passive(page);
      const label = (await canvasCard(page, LONG))?.rendered?.labels?.title;
      expect.soft(Boolean(label) && label.text !== label.fullText && label.text.trimEnd().endsWith('…') && label.fullText === LONG,
        `cut at rest, with an ellipsis: ${JSON.stringify(label && { text: label.text, fullText: label.fullText })}`).toBe(true);
      const where = () => page.evaluate(() => {
        const inspection = document.querySelector('.canvas-root').canvasInspection;
        return { camera: inspection.camera(), at: Object.fromEntries(inspection.names().map((name) => [name, inspection.card(name)?.position ?? null])) };
      });
      const before = await where();
      const said = await nameUnderPointer(page, LONG);
      expect.soft((said.title === LONG || (said.whole && said.text === LONG)) && said.under_pointer, `whole under the pointer: ${JSON.stringify(said)}`).toBe(true);
      expect.soft(await where(), 'nothing else moves as it is hovered').toEqual(before);
      await passive(page);
    });

    await test.step('then open one box in Focus', async () => {
      const region = await focus(page, 'busy');
      await region.getByRole('button', { name: 'Edit image', exact: true }).click();
      const prompt = region.locator('.in-frame .mention-text').first();
      await expect(prompt).toBeVisible();
      // Edit fits the frame again as it opens its words: looked at once they're at rest.
      await expect.poll(() => observeStill(page, 300, '.focus-viewport .in-frame .mention-text'), { message: 'the words at rest' }).toEqual([]);
      await page.mouse.move(2, 500);
      await looked('busy in Focus, written over its picture', true);
      const drawn = await prompt.evaluate(legibility);
      expect.soft(drawn.weight, `the prompt over the picture is bold: ${JSON.stringify(drawn)}`).toBeGreaterThanOrEqual(700);
      expect.soft(drawn.contrast, `and 4.5:1 or more against its surface: ${JSON.stringify(drawn)}`).toBeGreaterThanOrEqual(4.5);
      await page.keyboard.press('Escape');
      await expect(region).toHaveCount(0);
    });
  } finally {
    // Ended once looked at: by then either may have ended itself.
    if (renders) await api.raw('POST', project.at('/cancel'), { body: { names: ['failed', 'making'] } });
  }
});
