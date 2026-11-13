// canvas-nav (cl-973 core cell 15), on 300 boxes made by one API batch: − and + step along the zooms about the middle of what's visible; the
// zoom menu's Zoom to fit shows every box and its 100% is scale 1; Shift+1 and Shift+0 do what they do; at 11% a pan by the wheel keeps all 300
// drawn. The dots follow the canvas's scale and are never over a picture.
const { test, expect, canvasIdle, openPoint, png } = require('./fixtures');
const { camera, colours, sameCamera } = require('./geometry');

const COUNT = 300;
const ACROSS = 18;
/** The zooms − and + step along (ZoomControl.tsx ZOOMS). */
const ZOOMS = [0.05, 0.1, 0.15, 0.2, 0.25, 0.33, 0.5, 0.67, 0.75, 0.9, 1, 1.25, 1.5, 2, 2.5, 3, 4];
const stepped = (zoom, by) => (by > 0 ? ZOOMS.find((step) => step > zoom * 1.001) : [...ZOOMS].reverse().find((step) => step < zoom / 1.001));
/** The dots' step on screen at `zoom`: 24 on the canvas, doubled while under 12 (Sheet.tsx Dots). */
const dotStep = (zoom) => {
  let gap = 24 * zoom;
  while (gap < 12) gap *= 2;
  return gap;
};

/** The middle of what's visible, between the tools and the canvas's right edge, as a point of the canvas (CanvasHost.tsx zoomTo). */
const middleOnCanvas = (page) => page.evaluate(() => {
  const pane = document.querySelector('.react-flow').getBoundingClientRect();
  const left = document.querySelector('.tools')?.getBoundingClientRect().right ?? pane.left;
  const right = document.querySelector('.drawer')?.getBoundingClientRect().left ?? pane.right;
  const m = new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform);
  return { x: ((left + right) / 2 - pane.left - m.e) / m.a, y: (pane.height / 2 - m.f) / m.a };
});

test('canvas-nav', async ({ context, project, api, signIn }) => {
  // One picture of one colour, 300 boxes of it by one apply, and their places by one canvas patch: 18 across, a gap between each.
  const file = await project.upload(png(0, 320, 200, false, () => [190, 40, 40]));
  const nodes = Object.fromEntries(Array.from({ length: COUNT }, (_, i) => [`n${i}`, { kind: 'image', takes: [{ file }] }]));
  const applied = await api.post(project.at('/apply'), { nodes });
  expect(applied.applied, `one apply makes ${COUNT} boxes: ${JSON.stringify(applied.errors ?? []).slice(0, 300)}`).toBe(true);
  const cards = Object.fromEntries(Array.from({ length: COUNT }, (_, i) => [`n${i}`, { x: (i % ACROSS) * 304, y: Math.floor(i / ACROSS) * 240, w: 240 }]));
  await api.patch(project.at('/canvas'), { cards });
  const page = await context.newPage();
  await signIn(page, project.url());
  const cardsDrawn = page.locator('.react-flow__node .card[data-asset]');
  // Opened for the first time, it fits them all.
  await expect(cardsDrawn).toHaveCount(COUNT);
  await canvasIdle(page);
  const zoomButton = page.getByRole('button', { name: /more zoom options$/ });
  const zoomMenu = async (item) => {
    await zoomButton.click();
    await page.getByRole('menuitem', { name: new RegExp(`^${item}`) }).click();
    await canvasIdle(page);
  };
  const shows = async (zoom) => expect.soft(zoomButton, `the zoom says ${Math.round(zoom * 100)}%`).toHaveText(`${Math.round(zoom * 100)}%`);
  /** The dots drawn at the step `zoom` asks for. */
  const dotsFollow = async (zoom) => {
    const step = Number(await page.locator('.canvas-dots pattern').getAttribute('width'));
    expect.soft(step, `the dots' step at ${zoom}`).toBeCloseTo(dotStep(zoom), 3);
  };
  /** No dot over the picture of the box nearest the middle, inside its edge. */
  const noDotOnAPicture = async (where) => {
    await page.mouse.move(2, 500);
    const centre = await page.evaluate(() => {
      const pane = document.querySelector('.react-flow').getBoundingClientRect();
      const left = document.querySelector('.tools').getBoundingClientRect().right;
      return { x: (left + pane.right) / 2, y: pane.top + pane.height / 2 };
    });
    const media = await page.evaluate(({ x, y }) => {
      const all = [...document.querySelectorAll('.react-flow__node .card[data-asset] > .media')].map((el) => el.getBoundingClientRect());
      return all.sort((a, b) => Math.hypot(a.x + a.width / 2 - x, a.y + a.height / 2 - y) - Math.hypot(b.x + b.width / 2 - x, b.y + b.height / 2 - y))[0].toJSON();
    }, centre);
    const inset = 6;
    const said = await colours(page, { x: media.x + inset, y: media.y + inset, width: media.width - 2 * inset, height: media.height - 2 * inset });
    expect.soft(said.off, `${where}: no dot over a picture, all ${said.of} pixels its own colour ${said.common}`).toBe(0);
  };

  await test.step('Zoom out and Zoom in', async () => {
    const start = await camera(page);
    for (const [label, by] of [['Zoom out', -1], ['Zoom in', 1]]) {
      const [was, middle] = [(await camera(page)).zoom, await middleOnCanvas(page)];
      await page.getByRole('button', { name: label, exact: true }).click();
      await canvasIdle(page);
      const now = await camera(page);
      expect.soft(now.zoom, `${label} steps from ${was}`).toBeCloseTo(stepped(was, by), 4);
      await shows(now.zoom);
      const after = await middleOnCanvas(page);
      expect.soft(Math.hypot(after.x - middle.x, after.y - middle.y), `${label} keeps the middle where it was`).toBeLessThan(1);
    }
    expect.soft((await camera(page)).zoom, 'out and in again is a step on from the fit').not.toBe(start.zoom);
  });

  let fitted;
  let hundred;
  await test.step("the zoom menu's Zoom to fit and 100%", async () => {
    await zoomMenu('Zoom to fit');
    fitted = await camera(page);
    const inside = await cardsDrawn.evaluateAll((all) => all.filter((card) => {
      const r = card.getBoundingClientRect();
      return r.left >= 0 && r.top >= 0 && r.right <= innerWidth && r.bottom <= innerHeight;
    }).length);
    expect.soft(inside, 'fit shows every box').toBe(COUNT);
    await shows(fitted.zoom);
    await dotsFollow(fitted.zoom);
    await expect.poll(() => page.locator('.react-flow__node .card[data-asset="n150"] img').evaluate((image) => image.complete && image.naturalWidth > 0), { message: 'the pictures drawn' }).toBe(true);
    await noDotOnAPicture('fitted');

    await zoomMenu('Zoom to 100%');
    hundred = await camera(page);
    expect.soft(hundred.zoom, '100% is scale 1').toBe(1);
    await shows(1);
    await dotsFollow(1);
    const open = await openPoint(page);
    const dots = await colours(page, { x: open.x - 36, y: open.y - 36, width: 72, height: 72 });
    expect.soft(dots.off, 'dots are drawn on open canvas').toBeGreaterThan(0);
    await noDotOnAPicture('at 100%');
  });

  await test.step('Shift+0 and Shift+1', async () => {
    await page.keyboard.press('Shift+Digit1');
    await canvasIdle(page);
    const fit = await camera(page);
    expect.soft(sameCamera(fit, fitted), `Shift+1 is Zoom to fit: ${JSON.stringify(fit)}, ${JSON.stringify(fitted)}`).toBe(true);
    await page.keyboard.press('Shift+Digit0');
    await canvasIdle(page);
    const one = await camera(page);
    expect.soft(sameCamera(one, hundred), `Shift+0 is Zoom to 100%: ${JSON.stringify(one)}, ${JSON.stringify(hundred)}`).toBe(true);
  });

  await test.step('wheel-pan at 11%', async () => {
    // The page's own last view saved first, so the one set here is the one a reload reads.
    const now = await camera(page);
    await expect.poll(async () => {
      const { viewport } = await api.get(project.at('/view'));
      return Boolean(viewport) && sameCamera({ x: viewport.x, y: viewport.y, zoom: viewport.zoom }, now);
    }, { message: "the page's view saved" }).toBe(true);
    await api.patch(project.at('/view'), { viewport: { x: 800, y: 150, zoom: 0.11 } });
    await page.reload();
    await expect(cardsDrawn).toHaveCount(COUNT);
    await canvasIdle(page);
    expect((await camera(page)).zoom, 'at 11%').toBeCloseTo(0.11, 4);
    await dotsFollow(0.11);
    // Each frame for 2 s, a wheel's sideways pan, 480 px one way then back, as see.js pans: the fewest boxes drawn in any frame.
    const panned = await page.evaluate(() => new Promise((done) => {
      const pane = document.querySelector('.react-flow__pane');
      const viewport = document.querySelector('.react-flow__viewport');
      const places = new Set();
      let [sign, count, fewest] = [1, 0, Infinity];
      const end = performance.now() + 2000;
      const step = (now) => {
        places.add(getComputedStyle(viewport).transform);
        fewest = Math.min(fewest, document.querySelectorAll('.react-flow__node .card[data-asset]').length);
        if (++count % 40 === 0) sign = -sign;
        pane.dispatchEvent(new WheelEvent('wheel', { deltaX: sign * 12, deltaY: 0, clientX: innerWidth / 2, clientY: innerHeight / 2, bubbles: true, cancelable: true }));
        if (now < end) requestAnimationFrame(step);
        else done({ places: places.size, fewest });
      };
      requestAnimationFrame(step);
    }));
    expect.soft(panned.places, 'the wheel pans the canvas').toBeGreaterThan(10);
    expect.soft(panned.fewest, `all ${COUNT} boxes stay drawn while it pans`).toBe(COUNT);
    expect.soft((await camera(page)).zoom, 'a pan changes no zoom').toBeCloseTo(0.11, 4);
  });
});
