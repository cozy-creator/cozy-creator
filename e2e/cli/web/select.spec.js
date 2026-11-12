// select (cl-973 core cell 3), at zoom 1 and 0.25: a click selects a box and opens nothing; Shift adds one; a Shift-drag selects exactly
// what it touches; Ctrl+A selects all; a click on the canvas clears it; a hover never looks selected. A selected box has a ring of the
// selection's own colour, with its glow, 3 px or more on screen.
const { test, expect, box, canvasIdle, middle, openPoint, png, zoomOf } = require('./fixtures');

// Pictures 240 wide on the canvas, a row of three and one under them, clear of each other.
const PLACES = { A: { x: 0, y: 0 }, B: { x: 360, y: 0 }, C: { x: 720, y: 0 }, D: { x: 0, y: 380 } };
const NAMES = Object.keys(PLACES);

/** How `name`'s picture is outlined, measured on screen: `look` is the selection's colour, a hover's or neither. */
const ring = (page, name) => page.evaluate((name) => {
  const colour = (value) => {
    const probe = document.createElement('i');
    probe.style.color = value;
    document.body.append(probe);
    const said = getComputedStyle(probe).color;
    probe.remove();
    return said;
  };
  const [select, hover] = [colour('var(--select)'), colour('var(--line2)')];
  const media = document.querySelector(`.react-flow__node .card[data-asset="${CSS.escape(name)}"] > .media`);
  const style = getComputedStyle(media);
  const zoom = new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform).a;
  return {
    solid: style.outlineStyle === 'solid',
    look: style.outlineColor === select ? 'selection' : style.outlineColor === hover ? 'hover' : style.outlineColor,
    // The glow is the ring's colour, faded.
    glow: style.boxShadow.includes(select.slice(4, -1)),
    px: Number((parseFloat(style.outlineWidth) * zoom).toFixed(2)),
  };
}, name);

const selected = (page) => page.locator('.react-flow__node .card[aria-selected="true"]').evaluateAll((cards) => cards.map((card) => card.dataset.asset).sort());

async function ringed(page, names) {
  for (const name of NAMES) {
    const said = await ring(page, name);
    if (names.includes(name)) {
      expect.soft(said, `${name} ringed in the selection's colour, with its glow`).toMatchObject({ solid: true, look: 'selection', glow: true });
      expect.soft(said.px, `${name}'s ring on screen, px`).toBeGreaterThanOrEqual(3);
    } else {
      expect.soft(said, `${name} not ringed`).toMatchObject({ look: expect.not.stringMatching(/^selection$/), glow: false });
    }
  }
}

async function click(page, name, modifier) {
  const { x, y } = await middle(box(page, name).locator('.media').first());
  if (modifier) await page.keyboard.down(modifier);
  await page.mouse.click(x, y);
  if (modifier) await page.keyboard.up(modifier);
}

/** The zoom menu's `items` in turn, then Zoom out `out` times: the canvas still at `percent`. */
async function zoomTo(page, items, out, percent) {
  for (const item of items) {
    await page.getByRole('button', { name: /more zoom options$/ }).click();
    await page.getByRole('menuitem', { name: new RegExp(`^${item}`) }).click();
    await canvasIdle(page);
  }
  for (let i = 0; i < out; i++) await page.getByRole('button', { name: 'Zoom out', exact: true }).click();
  await canvasIdle(page);
  await expect(page.getByRole('button', { name: /more zoom options$/ })).toHaveText(percent);
}

test('select', async ({ page, project }) => {
  for (const [i, name] of NAMES.entries()) await project.import(png(40 + 50 * i), { name, at: PLACES[name] });
  for (const name of NAMES) await expect(box(page, name)).toBeVisible();
  const url = page.url();

  // All four in view, then at 100% about the middle; then 50% and two steps out.
  for (const [zoom, items, out, percent] of [[1, ['Zoom to fit', 'Zoom to 100%'], 0, '100%'], [0.25, ['Zoom to 50%'], 2, '25%']]) {
    await test.step(`at zoom ${zoom}`, async () => {
      await zoomTo(page, items, out, percent);
      expect(await zoomOf(page)).toBeCloseTo(zoom, 3);

      await test.step('a click selects A and opens nothing', async () => {
        await click(page, 'A');
        await expect.poll(() => selected(page)).toEqual(['A']);
        await expect.soft(page.getByRole('region', { name: 'Focused asset' })).toHaveCount(0);
        expect.soft(page.url(), 'the same address').toBe(url);
        await ringed(page, ['A']);
      });

      await test.step('Shift adds B', async () => {
        await click(page, 'B', 'Shift');
        await expect.poll(() => selected(page)).toEqual(['A', 'B']);
        await ringed(page, ['A', 'B']);
      });

      await test.step('Esc clears it', async () => {
        await page.keyboard.press('Escape');
        await expect.poll(() => selected(page)).toEqual([]);
        await ringed(page, []);
      });

      await test.step('a Shift-drag selects what it touches: A, B and C', async () => {
        const [a, c] = [await box(page, 'A').locator('.media').first().boundingBox(), await box(page, 'C').locator('.media').first().boundingBox()];
        const from = { x: a.x - 12, y: a.y + a.height / 4 };
        const to = { x: c.x + c.width / 2, y: c.y + (c.height * 3) / 4 };
        expect(await page.evaluate(({ x, y }) => document.elementFromPoint(x, y)?.classList.contains('react-flow__pane'), from), 'the drag starts on open canvas').toBe(true);
        await page.keyboard.down('Shift');
        await page.mouse.move(from.x, from.y);
        await page.mouse.down();
        await page.mouse.move((from.x + to.x) / 2, (from.y + to.y) / 2, { steps: 4 });
        await page.mouse.move(to.x, to.y, { steps: 4 });
        await page.mouse.up();
        await page.keyboard.up('Shift');
        await expect.poll(() => selected(page)).toEqual(['A', 'B', 'C']);
        await ringed(page, ['A', 'B', 'C']);
      });

      await test.step('Ctrl+A selects all', async () => {
        await page.keyboard.press('ControlOrMeta+a');
        await expect.poll(() => selected(page)).toEqual(NAMES);
        await ringed(page, NAMES);
      });

      await test.step('a click on the canvas clears it', async () => {
        const { x, y } = await openPoint(page);
        await page.mouse.click(x, y);
        await expect.poll(() => selected(page)).toEqual([]);
        await ringed(page, []);
      });

      await test.step('a hover never looks selected', async () => {
        const { x, y } = await middle(box(page, 'D').locator('.media').first());
        await page.mouse.move(x, y);
        await expect.poll(() => ring(page, 'D').then((said) => said.look), 'D looks hovered').toBe('hover');
        expect.soft(await ring(page, 'D'), 'a hover has no glow').toMatchObject({ glow: false });
        await expect.soft(box(page, 'D')).toHaveAttribute('aria-selected', 'false');
        await page.mouse.move(2, 500);
      });
    });
  }
});
