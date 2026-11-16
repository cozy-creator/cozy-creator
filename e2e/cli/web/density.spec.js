// density (Paul, 11-15: "make sure the new pixijs canvas is robust"; cl-890's 10 px floor; cl-991's resize and DPR handling): the canvas
// at the sizes and densities people's screens have, 1440×900 at 1× and 2×, 1280×800 at 3×, 1920×1080 zoomed to 150%, 4K and a narrow
// window, and through a live resize. At each: the WebGL picture is drawn at the screen's own density (its backing store the CSS size × DPR,
// so nothing is scaled up blurry); every picture in view is there in its own colour at its body's middle; a press lands where the box is
// drawn, its body on its body and its name on its name, and a click there selects it; names are one line, cut with an ellipsis, at 10 px
// or more at 100%; no picture is stretched. Resized, boxes keep their places and the canvas its zoom.
// Browser zoom (Ctrl+/−) is what a smaller CSS viewport at a higher devicePixelRatio emulates: 1920×1080 at 150% is 1280×720 CSS px at
// DPR 1.5. CSS `zoom` on the body and CDP's page scale factor (a pinch's visual viewport) are no stand-ins: neither changes
// devicePixelRatio, so they'd judge a canvas no person gets.
const { test, expect, canvasIdle, canvasNames, png, screenshotPixels, selectedNames } = require('./fixtures');

const LONG = 'a_box_whose_name_goes_on_and_on_past_any_width';
// Each a solid colour of its own, far from paper and dark, and a shape of its own.
const PICTURES = [
  { name: 'red', colour: [190, 40, 40], w: 320, h: 200, at: { x: 0, y: 0 } },
  { name: 'green', colour: [40, 150, 60], w: 200, h: 320, at: { x: 420, y: 0 } },
  { name: 'blue', colour: [40, 80, 200], w: 300, h: 300, at: { x: 840, y: 0 } },
  { name: 'yellow', colour: [220, 180, 30], w: 400, h: 225, at: { x: 1260, y: 0 } },
  { name: 'purple', colour: [150, 50, 170], w: 240, h: 320, at: { x: 0, y: 560 } },
  { name: 'teal', colour: [30, 170, 180], w: 320, h: 180, at: { x: 420, y: 560 } },
  { name: 'orange', colour: [230, 120, 30], w: 210, h: 297, at: { x: 840, y: 560 } },
  { name: LONG, colour: [200, 60, 140], w: 320, h: 240, at: { x: 1260, y: 560 } },
];
const TEXTS = [
  { name: 'notes', text: 'A box of words, read at every size.', at: { x: 0, y: 1080 } },
  { name: 'more_notes', text: 'More words, under the pictures.', at: { x: 420, y: 1080 } },
];
const NAMES = [...PICTURES, ...TEXTS].map((box) => box.name);
const CASES = [
  { label: '1440×900 at 1×', viewport: { width: 1440, height: 900 }, dpr: 1 },
  { label: '1440×900 at 2×', viewport: { width: 1440, height: 900 }, dpr: 2 },
  { label: '1280×800 at 3×', viewport: { width: 1280, height: 800 }, dpr: 3 },
  { label: '1920×1080 zoomed to 150%', viewport: { width: 1280, height: 720 }, dpr: 1.5 },
  { label: '4K', viewport: { width: 3840, height: 2160 }, dpr: 1 },
  { label: 'a narrow window, 390×844 at 3×', viewport: { width: 390, height: 844 }, dpr: 3 },
];
const near = (pixel, colour) => pixel.every((value, i) => Math.abs(value - colour[i]) <= 12);

/** The canvas as drawn: its backing store and CSS size, its zoom, and each drawn box's body, name and what a press at each hits. */
const look = (page) => page.evaluate(() => {
  const root = document.querySelector('.canvas-root'), inspection = root?.canvasInspection, pixels = document.querySelector('.canvas-pixels');
  if (!inspection || !pixels) return null;
  const pane = root.getBoundingClientRect();
  // On the canvas itself, under nothing that floats over it.
  const clear = (x, y) => {
    if (x <= pane.left || y <= pane.top || x >= Math.min(pane.right, innerWidth) || y >= Math.min(pane.bottom, innerHeight)) return false;
    const there = document.elementFromPoint(x, y);
    return Boolean(there && root.contains(there) && !there.closest('.canvas-node'));
  };
  const boxes = inspection.names().flatMap((name) => {
    const card = inspection.card(name), shown = card?.rendered;
    if (!shown) return [];
    const middle = { x: shown.body.x + shown.body.width / 2, y: shown.body.y + shown.body.height / 2 };
    const title = shown.labels.title;
    const named = title && title.width >= 1 && title.height >= 1 ? { x: title.x + title.width / 2, y: title.y + title.height / 2 } : null;
    return [{
      name, position: { ...card.position }, body: shown.body, middle, media: shown.media,
      clear: clear(middle.x, middle.y), hit: inspection.hitTest(middle),
      title: title && { text: title.text, fullText: title.fullText, fontSize: title.fontSize }, named,
      namedClear: Boolean(named && clear(named.x, named.y)), namedHit: named && inspection.hitTest(named),
    }];
  });
  return {
    dpr: devicePixelRatio, zoom: inspection.camera().zoom, boxes,
    store: { width: pixels.width, height: pixels.height, cssWidth: pixels.clientWidth, cssHeight: pixels.clientHeight },
  };
});

/** The pointer off the canvas, on the top bar, so no box mounts its card over what's drawn (at a narrow window the canvas may have no
 *  open point beside the tools). */
async function passive(page) {
  await page.mouse.move(Math.round(page.viewportSize().width / 2), 2);
  await expect(page.locator('.canvas-node')).toHaveCount(0);
}

/** Every rule of the header, at `label`. Returns what it saw. */
async function judge(page, label) {
  await passive(page);
  await expect.poll(async () => {
    const now = await look(page);
    if (!now) return ['no canvas'];
    return PICTURES.filter((one) => now.boxes.some((box) => box.name === one.name && box.clear && !box.media.ready)).map((one) => one.name);
  }, { message: `${label}: every picture in view decoded`, timeout: 30_000 }).toEqual([]);
  const seen = await look(page);
  const read = screenshotPixels(await page.screenshot());
  const scale = read.width / page.viewportSize().width;
  const { store, dpr } = seen;
  expect.soft(Math.abs(store.width - store.cssWidth * dpr) <= 1 && Math.abs(store.height - store.cssHeight * dpr) <= 1,
    `${label}: drawn at the screen's density, ${store.width}×${store.height} for ${store.cssWidth}×${store.cssHeight} CSS px at ${dpr}×`).toBe(true);
  const inView = seen.boxes.filter((box) => box.clear);
  expect.soft(inView.length, `${label}: a box to see`).toBeGreaterThan(0);
  const colours = [], hits = [], titles = [], stretched = [];
  for (const box of inView) {
    const picture = PICTURES.find((one) => one.name === box.name);
    if (picture) {
      const pixel = read(Math.round(box.middle.x * scale), Math.round(box.middle.y * scale));
      if (!near(pixel, picture.colour)) colours.push(`${box.name}: ${pixel} at its middle, not ${picture.colour}`);
      const drawn = box.body.width / box.body.height, file = picture.w / picture.h;
      if (Math.abs(drawn / file - 1) > 0.03) stretched.push(`${box.name}: drawn ${drawn.toFixed(3)}, its file ${file.toFixed(3)}`);
    }
    if (box.hit?.name !== box.name || box.hit?.part !== 'body') hits.push(`${box.name}'s body: ${JSON.stringify(box.hit)}`);
    if (box.namedClear && (box.namedHit?.name !== box.name || box.namedHit?.part !== 'title')) hits.push(`${box.name}'s name: ${JSON.stringify(box.namedHit)}`);
  }
  for (const box of seen.boxes.filter((one) => one.title?.text)) {
    const { text, fullText, fontSize } = box.title;
    if (text.includes('\n')) titles.push(`${box.name}: ${text.split('\n').length} lines`);
    if (text !== fullText && !text.trimEnd().endsWith('…')) titles.push(`${box.name}: cut with no ellipsis, "${text}"`);
    if (fontSize < 10) titles.push(`${box.name}: ${fontSize.toFixed(1)} px at 100%`);
  }
  expect.soft(colours, `${label}: each picture in its colour where it's drawn`).toEqual([]);
  expect.soft(hits, `${label}: a press lands on what's drawn there`).toEqual([]);
  expect.soft(titles, `${label}: names one line, cut with an ellipsis, 10 px or more`).toEqual([]);
  expect.soft(stretched, `${label}: no picture stretched`).toEqual([]);
  const target = inView.find((box) => PICTURES.some((one) => one.name === box.name));
  if (target) {
    await page.mouse.click(target.middle.x, target.middle.y);
    await expect.configure({ soft: true }).poll(() => selectedNames(page), { message: `${label}: a click on ${target.name} selects it` }).toEqual([target.name]);
    await page.keyboard.press('Escape');
    await passive(page);
  }
  return seen;
}

test('density', async ({ page, project, newContext, signIn }) => {
  test.setTimeout(360_000);
  for (const one of PICTURES) await project.import(png(0, one.w, one.h, false, () => one.colour), { name: one.name, at: one.at });
  for (const one of TEXTS) await project.add({ name: one.name, kind: 'text', fields: { text: one.text }, at: one.at });
  await expect.poll(async () => (await canvasNames(page)).sort(), { message: 'every box on the canvas' }).toEqual([...NAMES].sort());

  for (const { label, viewport, dpr } of CASES) {
    await test.step(label, async () => {
      const context = await newContext({ viewport, deviceScaleFactor: dpr });
      const tab = await context.newPage();
      await signIn(tab, project.url());
      await expect(tab.locator('.stage')).toBeVisible();
      await expect.poll(async () => (await canvasNames(tab)).sort(), { message: `${label}: every box on the canvas` }).toEqual([...NAMES].sort());
      expect(await tab.evaluate(() => devicePixelRatio), `${label}: the density asked`).toBe(dpr);
      await tab.keyboard.press('Shift+Digit1');
      await canvasIdle(tab);
      await judge(tab, label);
      await context.close();
    });
  }

  await test.step('a window resized from 1600×1000 to 900×700 and back', async () => {
    await page.keyboard.press('Shift+Digit1');
    await canvasIdle(page);
    const before = await judge(page, '1600×1000');
    for (const size of [{ width: 900, height: 700 }, { width: 1600, height: 1000 }]) {
      const label = `resized to ${size.width}×${size.height}`;
      await page.setViewportSize(size);
      await canvasIdle(page);
      const now = await judge(page, label);
      expect.soft(Math.abs(now.zoom - before.zoom), `${label}: the canvas keeps its zoom`).toBeLessThan(0.0005);
      const moved = before.boxes.filter((was) => {
        const is = now.boxes.find((box) => box.name === was.name);
        return is && (Math.abs(is.position.x - was.position.x) > 0.5 || Math.abs(is.position.y - was.position.y) > 0.5);
      }).map((was) => was.name);
      expect.soft(moved, `${label}: every box where it was`).toEqual([]);
    }
  });
});
