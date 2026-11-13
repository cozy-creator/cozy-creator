// layout (cl-973 core cell 18), at 1600×1000 on Chrome's GPU raster path with its scrollbars drawn: a box of every kind in every state, each where
// the API put it, looked at 0.25, 0.5, 1 and 2, then one open in Focus. No box or label lies on another; nothing is cut off, nothing that floats
// over the canvas lies on another, and Focus is under none of it; no scrollbar anywhere. Words read at 10 px or more (a box's at 100%: they're
// drawn at the canvas's scale, cl-890); a prompt over a picture is bold, 4.5:1 or more against its surface (cl-901). A render's Cancel shows at
// 1 and at 0.5, and the Focus frame fits the window. Each finding names its rule, where, and what (layout.js's rules, measured).
const { randomUUID } = require('crypto');
const { test, expect, box, canvasIdle, focus, fixtureTool, MP4, png } = require('./fixtures');

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
  const zoom = new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform).a;
  const name = (el) => el.closest?.('.react-flow__node')?.querySelector('.card')?.dataset.asset;
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
    const onCanvas = Boolean(el.closest('.react-flow__node'));
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
  // At rest, no box on another, with its name over it and what hangs under it.
  const boxes = [...document.querySelectorAll('.react-flow__node')].filter((node) => visible(node) && onScreen(R(node)) && !node.querySelector('.card.here') && !node.classList.contains('dragging'));
  const hung = (node) => [node, ...node.querySelectorAll('.head > *, .ratio-rest, .tile-under, .refused-line')].filter(visible);
  for (let i = 0; i < boxes.length; i++) for (let j = i + 1; j < boxes.length; j++) {
    const [a, b] = [hung(boxes[i]), hung(boxes[j])];
    const on = a.flatMap((ea) => b.filter((eb) => meet(R(ea), R(eb))).map((eb) => [ea, eb]))[0];
    if (on) say('boxes-overlap', boxes[i], `with <${name(boxes[j])}>: ${where(on[0])} ${round(R(on[0]))} on ${where(on[1])} ${round(R(on[1]))}`);
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
  return found;
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
  for (const name of names) await expect(box(page, name)).toBeVisible();

  // A render that fails (the crash fixture), then one being made all through the looking: local renders one at a time. Both ended after.
  const render = (name) => api.call('POST', project.at('/runs'), { names: [name], on: ['local'] }, { headers: { 'Idempotency-Key': randomUUID() } });
  try {
    await render('failed');
    await expect.poll(async () => {
      const held = await project.asset('failed');
      return Boolean(held?.failure || held?.takes?.some((take) => take.failure));
    }, { message: 'failed fails', timeout: 60_000 }).toBe(true);
    await render('making');
    const face = page.getByRole('status', { name: "making's render" });
    await expect(face, 'making is being made').toBeVisible({ timeout: 60_000 });
    const zoomButton = page.getByRole('button', { name: /more zoom options$/ });
    const zoomMenu = async (item) => {
      await zoomButton.click();
      await page.getByRole('menuitem', { name: new RegExp(`^${item}`) }).click();
      await canvasIdle(page);
    };
    /** Looked at once what animates has finished: every finding, said whole. */
    const looked = async (what, focused = false) => {
      await expect.poll(() => page.evaluate(() => document.getAnimations().filter((one) => one.playState === 'running' && one.effect?.getComputedTiming().iterations !== Infinity).length), { message: `${what}: still` }).toBe(0);
      expect.soft(await page.evaluate(audit, { floor: FLOOR, focused }), `${what}: laid out`).toEqual([]);
    };
    const cancelShows = async (zoom) => {
      const cancel = face.getByRole('button', { name: "Cancel making's render" });
      await expect.soft(cancel, `Cancel shows on the rendering face at ${zoom}`).toBeVisible();
      const r = await cancel.boundingBox();
      expect.soft(Boolean(r) && r.x >= 0 && r.y >= 0 && r.x + r.width <= 1600 && r.y + r.height <= 1000, `and on screen at ${zoom}: ${JSON.stringify(r)}`).toBe(true);
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

    await test.step('then open one box in Focus', async () => {
      const region = await focus(page, 'busy');
      await region.getByRole('button', { name: 'Edit image', exact: true }).click();
      const prompt = region.locator('.in-frame .mention-text').first();
      await expect(prompt).toBeVisible();
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
    await api.raw('POST', project.at('/cancel'), { body: { names: ['failed', 'making'] } });
  }
});
