// input (Paul, 11-15: "make sure the new pixijs canvas is robust"): every hand a person brings to the Pixi canvas, beyond what select, move,
// box-actions, focus and type already hold. A trackpad's pinch zooms about the pointer, nothing moving under it; its two-finger pan and a
// mouse's line-by-line wheel pan, the zoom unchanged. A finger drags open canvas to pan; a tap selects a box and a drag moves it; two fingers
// pinch to zoom. Space held turns a drag into a pan, even from on a box. The arrow keys walk the boxes and Enter opens the one chosen. In
// Focus, words selected by a drag and by Shift+arrows stay selected and the canvas stays still; Japanese and Chinese typed through an IME
// land once each, through a reload (Paul: typed text is never lost); words copied from one box paste into another. A right-click on open
// canvas makes nothing. CDP's touch and IME and the clipboard are Chromium's here: Firefox and WebKit say so and go on.
const { test, expect, canvasCard, canvasIdle, canvasNames, canvasPoint, canvasVisible, focus, openPoint, png, selectedNames } = require('./fixtures');
const { camera, sameCamera } = require('./geometry');
const { fit, leave } = require('./hands');

const BOXES = { a: { x: 0, y: 0 }, b: { x: 420, y: 0 }, c: { x: 840, y: 0 } };
const WORDS = 'The harbour lights came on one by one.';
const JAPANESE = ['k', 'こ', 'こn', 'こん', 'こんn', 'こんに', 'こんにt', 'こんにち', 'こんにちh', 'こんにちは'];
const CHINESE = ['n', 'ni', 'nih', 'niha', 'nihao', '你好', '你好s', '你好sh', '你好shi', '你好shij', '你好shijie', '你好世界'];
const TYPED = '「こんにちは」你好世界';

/** Every box's place in canvas units, by name. */
const places = (page, names) => Promise.all(names.map(async (name) => [name, (await canvasCard(page, name))?.position])).then(Object.fromEntries);
/** Where a canvas point is drawn on the screen now. */
const screenOf = (page, world) => page.evaluate(({ x, y }) => {
  const root = document.querySelector('.canvas-root'), view = root.canvasInspection.camera(), pane = root.getBoundingClientRect();
  return { x: pane.left + view.x + x * view.zoom, y: pane.top + view.y + y * view.zoom };
}, world);
const worldOf = (page, point) => page.evaluate(({ x, y }) => {
  const root = document.querySelector('.canvas-root'), view = root.canvasInspection.camera(), pane = root.getBoundingClientRect();
  return { x: (x - pane.left - view.x) / view.zoom, y: (y - pane.top - view.y) / view.zoom };
}, point);
const near = (a, b, by) => Math.abs(a.x - b.x) <= by && Math.abs(a.y - b.y) <= by;
/** A finger's touches through CDP: `points` per moment, [] to lift. */
async function touch(cdp, moments) {
  let down = false;
  for (const points of moments) {
    const type = !points.length ? 'touchEnd' : down ? 'touchMove' : 'touchStart';
    await cdp.send('Input.dispatchTouchEvent', { type, touchPoints: points.map(([x, y], id) => ({ x, y, id })) });
    down = points.length > 0;
  }
}
/** One finger from `from` to `to`: each moment its one point. */
const along = (from, to, steps) => Array.from({ length: steps + 1 }, (_, i) => [[from[0] + ((to[0] - from[0]) * i) / steps, from[1] + ((to[1] - from[1]) * i) / steps]]);
const selectionLength = (page) => page.evaluate(() => document.getSelection()?.toString().length ?? 0);

test('input', { tag: '@engines' }, async ({ page, project, browserName }) => {
  const chromium = browserName === 'chromium';
  const skip = (what) => test.info().annotations.push({ type: 'skipped', description: `${what}: CDP and its clipboard are Chromium's` });
  for (const [name, at] of Object.entries(BOXES)) await project.import(png(40 + name.charCodeAt(0), 320, 200), { name, at });
  await project.add({ name: 'words', kind: 'text', fields: { text: WORDS }, at: { x: 0, y: 420 } });
  await project.add({ name: 'pasted', kind: 'text', fields: { text: '', node_kind: 'text' }, at: { x: 420, y: 420 } });
  await project.add({ name: 'ime', kind: 'text', fields: { text: '', node_kind: 'text' }, at: { x: 840, y: 420 } });
  const names = [...Object.keys(BOXES), 'words', 'pasted', 'ime'];
  for (const name of names) await expect.poll(() => canvasVisible(page, name)).toBe(true);
  await fit(page);
  const region = page.getByRole('region', { name: 'Focused asset' });

  await test.step("a trackpad's pinch zooms about the pointer, nothing moving under it", async () => {
    const at = await openPoint(page), world = await worldOf(page, at), was = await places(page, names), before = await camera(page);
    await page.mouse.move(at.x, at.y);
    await page.keyboard.down('Control');
    for (let i = 0; i < 6; i++) await page.mouse.wheel(0, -8);
    await page.keyboard.up('Control');
    await canvasIdle(page);
    const after = await camera(page), drawn = await screenOf(page, world);
    expect.soft(after.zoom, 'it zoomed in').toBeGreaterThan(before.zoom * 1.02);
    expect.soft(near(drawn, at, 2), `the point under the pointer stays there: ${JSON.stringify({ at, drawn })}`).toBe(true);
    expect.soft(await places(page, names), 'no box moves').toEqual(was);
  });

  await test.step('a two-finger pan and a line-by-line wheel pan, the zoom unchanged', async () => {
    const at = await openPoint(page), before = await camera(page);
    await page.mouse.move(at.x, at.y);
    await page.mouse.wheel(30, 20);
    await canvasIdle(page);
    const panned = await camera(page), dx = panned.x - before.x, dy = panned.y - before.y;
    expect.soft(panned.zoom, 'a pan changes no zoom').toBeCloseTo(before.zoom, 6);
    expect.soft(dx < 0 && dy < 0 && Math.abs(dx / dy - 1.5) < 0.15, `it pans both ways at once, as the fingers went: ${dx}, ${dy}`).toBe(true);
    // A mouse wheel in lines (Firefox's own): 3 lines, a line at least 8 px.
    await page.evaluate(({ x, y }) => document.querySelector('.canvas-pixels').dispatchEvent(new WheelEvent('wheel', { deltaX: 0, deltaY: 3, deltaMode: 1, clientX: x, clientY: y, bubbles: true, cancelable: true })), at);
    await canvasIdle(page);
    const lined = await camera(page);
    expect.soft(lined.zoom, 'a wheel in lines changes no zoom').toBeCloseTo(before.zoom, 6);
    expect.soft(panned.y - lined.y, 'a wheel in lines pans by its lines').toBeGreaterThanOrEqual(24);
  });

  await test.step('a finger: a drag on open canvas pans, a tap selects a box and a drag moves it, two pinch to zoom', async () => {
    if (!chromium) return skip('touch');
    const cdp = await page.context().newCDPSession(page);
    await cdp.send('Emulation.setTouchEmulationEnabled', { enabled: true, maxTouchPoints: 5 });
    try {
      let at = await openPoint(page), before = await camera(page);
      await touch(cdp, [...along([at.x, at.y], [at.x - 120, at.y - 60], 8), []]);
      await canvasIdle(page);
      let now = await camera(page);
      expect.soft(near({ x: now.x - before.x, y: now.y - before.y }, { x: -120, y: -60 }, 12), `the canvas follows the finger: ${now.x - before.x}, ${now.y - before.y}`).toBe(true);
      expect.soft(now.zoom, 'a finger pan changes no zoom').toBeCloseTo(before.zoom, 6);

      // Every box in view again before a finger goes to one.
      await page.keyboard.press('Shift+Digit1');
      await canvasIdle(page);
      const body = await canvasPoint(page, 'b');
      await touch(cdp, [[[body.x, body.y]], []]);
      await expect.poll(() => selectedNames(page), { message: 'a tap selects it' }).toEqual(['b']);
      const was = (await canvasCard(page, 'b')).position;
      before = await camera(page);
      await touch(cdp, [...along([body.x, body.y], [body.x + 90, body.y + 40], 8), []]);
      await canvasIdle(page);
      const moved = (await canvasCard(page, 'b')).position;
      now = await camera(page);
      expect.soft(sameCamera(now, before), 'dragging a selected box moves no canvas').toBe(true);
      expect.soft(near({ x: (moved.x - was.x) * now.zoom, y: (moved.y - was.y) * now.zoom }, { x: 90, y: 40 }, 8), `the box follows the finger: ${JSON.stringify({ was, moved })}`).toBe(true);
      await expect.poll(async () => { const held = (await project.api.get(project.at('/canvas'))).cards.b; return held && near(held, moved, 1); }, { message: 'and stays moved' }).toBe(true);

      at = await openPoint(page);
      const others = await places(page, ['a', 'c']);
      before = await camera(page);
      const steps = 8, pinch = Array.from({ length: steps + 1 }, (_, i) => [[at.x - 20 - i * 10, at.y], [at.x + 20 + i * 10, at.y]]);
      await touch(cdp, [...pinch, []]);
      await canvasIdle(page);
      now = await camera(page);
      expect.soft(now.zoom, 'two fingers apart zoom in').toBeGreaterThan(before.zoom * 1.2);
      expect.soft(await places(page, ['a', 'c']), 'and move no box').toEqual(others);
    } finally {
      await cdp.send('Emulation.setTouchEmulationEnabled', { enabled: false }).catch(() => {});
      await cdp.detach().catch(() => {});
    }
  });

  await test.step('keys: Space and a drag pan, from on a box too; the arrows walk the boxes and Enter opens one', async () => {
    await fit(page);
    const from = await canvasPoint(page, 'a'), was = await places(page, names), before = await camera(page), chosen = await selectedNames(page);
    await page.keyboard.down('Space');
    await page.mouse.move(from.x, from.y);
    await page.mouse.down();
    await page.mouse.move(from.x + 60, from.y + 30, { steps: 8 });
    await page.mouse.up();
    await page.keyboard.up('Space');
    await canvasIdle(page);
    const now = await camera(page);
    expect.soft(near({ x: now.x - before.x, y: now.y - before.y }, { x: 60, y: 30 }, 3), `Space and a drag pan: ${now.x - before.x}, ${now.y - before.y}`).toBe(true);
    expect.soft(await places(page, names), 'and move no box').toEqual(was);
    expect.soft(await selectedNames(page), 'and choose nothing').toEqual(chosen);

    const clear = await openPoint(page);
    await page.mouse.click(clear.x, clear.y);
    const walked = [];
    for (const key of ['ArrowRight', 'ArrowRight', 'ArrowLeft']) {
      await page.keyboard.press(key);
      await expect.poll(async () => (await selectedNames(page)).length, { message: `${key} selects one box` }).toBe(1);
      walked.push((await selectedNames(page))[0]);
    }
    expect.soft(walked[1] !== walked[0] && walked[2] === walked[0], `the arrows walk on and back: ${walked.join(' → ')}`).toBe(true);
    await page.keyboard.press('Enter');
    await expect(region, 'Enter opens the box chosen').toHaveAttribute('data-focused-asset', walked[2]);
    await page.keyboard.press('Escape');
    await expect(region).toHaveCount(0);
    await canvasIdle(page);
  });

  await test.step('in Focus, words selected by a drag and by Shift+arrows stay selected, the canvas still', async () => {
    await focus(page, 'words');
    const text = region.getByRole('textbox', { name: 'Text', exact: true });
    await expect(text).toHaveText(WORDS);
    const r = await text.boundingBox(), held = await camera(page);
    await page.mouse.move(r.x + 4, r.y + 8);
    await page.mouse.down();
    await page.mouse.move(r.x + r.width * 0.6, r.y + 8, { steps: 8 });
    await page.mouse.up();
    const dragged = await selectionLength(page);
    expect.soft(dragged, 'a drag selects words').toBeGreaterThan(5);
    for (let i = 0; i < 3; i++) await page.keyboard.press('Shift+ArrowLeft');
    expect.soft(await selectionLength(page), 'Shift+← three times takes three back').toBe(dragged - 3);
    expect.soft(sameCamera(await camera(page), held), 'the canvas behind stays still').toBe(true);
    await expect.soft(region, 'still in Focus').toHaveAttribute('data-focused-asset', 'words');
  });

  await test.step('words copied from one box paste into another, once', async () => {
    if (!chromium) return skip('copy and paste');
    const text = region.getByRole('textbox', { name: 'Text', exact: true });
    await text.click();
    await page.keyboard.press('ControlOrMeta+a');
    await page.keyboard.press('ControlOrMeta+c');
    await leave(page);
    await focus(page, 'pasted');
    await expect(text).toBeFocused();
    await page.keyboard.press('ControlOrMeta+v');
    await expect.soft(text, 'shown once').toHaveText(WORDS);
    await expect.poll(async () => (await project.asset('pasted'))?.fields?.text, { message: 'saved once' }).toBe(WORDS);
    await leave(page);
  });

  await test.step('Japanese and Chinese through an IME: each character once, through a reload', async () => {
    if (!chromium) return skip('IME');
    if (await region.count()) await leave(page);
    await focus(page, 'ime');
    const text = region.getByRole('textbox', { name: 'Text', exact: true });
    await expect(text).toBeFocused();
    const cdp = await page.context().newCDPSession(page);
    try {
      for (const [steps, committed] of [[JAPANESE, '「こんにちは」'], [CHINESE, '你好世界']]) {
        for (const composing of steps) await cdp.send('Input.imeSetComposition', { text: composing, selectionStart: composing.length, selectionEnd: composing.length });
        await cdp.send('Input.insertText', { text: committed });
      }
    } finally {
      await cdp.detach().catch(() => {});
    }
    await expect.soft(text, 'shown as composed, once').toHaveText(TYPED);
    await expect.poll(async () => (await project.asset('ime'))?.fields?.text, { message: 'saved as composed, once' }).toBe(TYPED);
    await page.reload();
    await expect(region).toHaveAttribute('data-focused-asset', 'ime');
    await expect.soft(region.getByRole('textbox', { name: 'Text', exact: true }), 'and after a reload').toHaveText(TYPED);
    expect.soft((await project.asset('ime'))?.fields?.text, 'the file after a reload').toBe(TYPED);
    await leave(page);
  });

  await test.step('a right-click on open canvas makes nothing, and the canvas takes the next click', async () => {
    if (await region.count()) await leave(page);
    const before = (await canvasNames(page)).sort(), assets = (await project.assets()).length;
    const at = await openPoint(page);
    await page.mouse.click(at.x, at.y, { button: 'right' });
    await expect.soft(page.getByRole('menu'), 'no menu of a box').toHaveCount(0);
    expect.soft((await canvasNames(page)).sort(), 'no box drawn').toEqual(before);
    expect.soft((await project.assets()).length, 'none made').toBe(assets);
    await page.keyboard.press('Escape');
    const { x, y } = await canvasPoint(page, 'c');
    await page.mouse.click(x, y);
    await expect.poll(() => selectedNames(page), { message: 'a click after it selects' }).toEqual(['c']);
  });
});
