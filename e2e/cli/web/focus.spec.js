// focus (cl-973 core cell 4): a double click opens a box in Focus, at its own address; every way out (Esc, Close focus, Back to canvas, the
// browser's Back, and while a trackpad's scroll or pinch still glides: cl-955) puts the canvas back exactly where it was, and Forward and a
// reload come back to Focus on the same box. A press that began inside Focus, the Assets drawer or a box's menu and was let go outside keeps
// it open; one that began outside closes it (Paul, 11-12). In Focus the wheel zooms the picture, its own file (cl-1184), never the canvas,
// with no blank frame. An edit's backdrop shows every picture its words use, as large as they can be (cl-949).
const { test, expect, canvasIdle, focus, middle, observeStill, openPoint, png, canvasPoint, canvasVisible } = require('./fixtures');
const { camera, drag, sameCamera } = require('./geometry');

// A tall picture, fitted in Focus narrower than its thumbnail and still its own file (cl-1184); and Qwen's Image with a take beside it.
const PICTURE = 'tall';
const TOOLED = 'harbour';
// cl-949's edit: Qwen's box with a take, its words mentioning three pictures of three shapes.
const EDIT = 'edit';
const USED = { wide: [320, 160], square: [200, 200], narrow: [160, 280] };

/** Focus as it stands once it has drawn in: its region, naming `name`, and the canvas still. */
async function inFocus(page, project, name) {
  const region = page.getByRole('region', { name: 'Focused asset' });
  await expect(region).toHaveAttribute('data-focused-asset', name);
  await canvasIdle(page);
  await expect.soft(page, `Focus is at ${name}'s address`).toHaveURL(project.url(`assets/${encodeURIComponent(name)}`));
  return region;
}

/** Out of Focus, and the canvas home: exactly where it was, at its own address. */
async function backHome(page, project, before, way) {
  await expect(page.getByRole('region', { name: 'Focused asset' }), `${way} leaves Focus`).toHaveCount(0);
  await canvasIdle(page);
  const now = await camera(page);
  expect.soft(sameCamera(now, before), `${way}: the canvas where it was, ${JSON.stringify(before)}, not ${JSON.stringify(now)}`).toBe(true);
  await expect.soft(page, `${way}: the canvas's address`).toHaveURL(project.url());
}

/** A point of Focus's own background, the canvas behind it: on none of what's its own (FocusedViewport.tsx OWN). */
const background = (page) => page.evaluate(() => {
  const own = '.media, .tl, .tl-player, .tl-transport, .focus-heading, .focus-warnings, .run-face, .here-side, .here-under, .image-controls, .focus-image-zoom, button, input, textarea, select, [contenteditable]';
  const room = document.querySelector('.focus-viewport').getBoundingClientRect();
  for (let y = room.bottom - 12; y > room.top + 40; y -= 16) for (let x = room.right - 12; x > room.left + 12; x -= 16) {
    const there = document.elementFromPoint(x, y);
    if (there?.closest('.focus-viewport') && !there.closest(own)) return { x, y };
  }
  throw new Error('no background in Focus');
});

/** Just inside what `locator` shows, on its first line. */
async function inside(locator) {
  const r = await locator.boundingBox();
  return { x: r.x + 6, y: r.y + Math.min(10, r.height / 2) };
}

/** A point in `locator`'s last words, a few letters from their end: where a press begins selecting words, on no chip. */
const inWords = (locator) => locator.evaluate((el) => {
  const walk = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
  let last = null;
  for (let node = walk.nextNode(); node; node = walk.nextNode()) if (node.textContent.trim() && !node.parentElement.closest('.chip')) last = node;
  if (!last) throw new Error(`no words in ${el.className}: ${el.textContent}`);
  const end = last.textContent.trimEnd().length;
  const range = document.createRange();
  range.setStart(last, Math.max(0, end - 6));
  range.setEnd(last, Math.max(1, end - 5));
  const r = range.getBoundingClientRect();
  return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
});

const selected = (page) => page.evaluate(() => document.getSelection()?.toString() ?? '');

test('focus', async ({ page, project, api }) => {
  const { tools } = await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}&q=qwen`);
  const qwen = tools.find((tool) => tool.package === 'qwen-image-2' && tool.makes === 'image' && tool.fields.some((field) => field.type === 'prompt'));
  expect(qwen, 'the Hub lists Qwen Image 2 with a prompt').toBeTruthy();
  await project.import(png(70, 1200, 2400), { name: PICTURE, at: { x: 0, y: 0 } });
  await project.add({ name: TOOLED, tool: qwen.id, kind: 'image', fields: { instructions: 'a lighthouse at dusk, waves on black rocks' }, at: { x: 420, y: 0 } });
  await api.post(project.at(`/assets/${TOOLED}/versions`), { source: await project.upload(png(150)) });
  await expect.poll(async () => (await project.asset(TOOLED)).versions, { message: `${TOOLED} has its take` }).toBe(1);
  for (const [i, [name, [w, h]]] of Object.entries(USED).entries()) await project.import(png(60 + i * 70, w, h), { name, at: { x: 840 + i * 360, y: 0 } });
  await project.add({ name: EDIT, tool: qwen.id, kind: 'image', fields: { instructions: '<wide>, <square> and <narrow> in one picture' }, at: { x: 840, y: 420 } });
  await api.post(project.at(`/assets/${EDIT}/versions`), { source: await project.upload(png(100, 300, 200)) });
  await expect.poll(async () => (await project.asset(EDIT)).versions, { message: `${EDIT} has its take` }).toBe(1);
  for (const name of [PICTURE, TOOLED, EDIT, ...Object.keys(USED)]) await expect.poll(() => canvasVisible(page, name)).toBe(true);
  // The canvas somewhere of its own: all in view, then a step out.
  await page.getByRole('button', { name: /more zoom options$/ }).click();
  await page.getByRole('menuitem', { name: /^Zoom to fit/ }).click();
  await canvasIdle(page);
  await page.getByRole('button', { name: 'Zoom out', exact: true }).click();
  await canvasIdle(page);
  const before = await camera(page);

  await test.step('note the viewport, then double-click a box', async () => {
    await focus(page, TOOLED);
    await inFocus(page, project, TOOLED);
  });

  await test.step('leave in turn by Esc, Close focus, Back to canvas, browser Back, then Forward, then reload in Focus', async () => {
    const ways = {
      Esc: () => page.keyboard.press('Escape'),
      'Close focus': () => page.getByRole('button', { name: 'Close focus', exact: true }).click(),
      'Back to canvas': () => page.getByRole('button', { name: 'Back to canvas', exact: true }).click(),
      "the browser's Back": () => page.goBack(),
    };
    for (const [i, [way, leave]] of Object.entries(ways).entries()) {
      if (i > 0) {
        await focus(page, TOOLED);
        await inFocus(page, project, TOOLED);
      }
      await leave();
      await backHome(page, project, before, way);
    }
    await page.goForward();
    await inFocus(page, project, TOOLED);
    await page.reload();
    await inFocus(page, project, TOOLED);
    await page.keyboard.press('Escape');
    await backHome(page, project, before, 'Esc after the reload');
  });

  await test.step('leave while a scroll, then a pinch, still glides', async () => {
    // cl-955 (Paul, 11-12: "I exit out, but rather than snapping back … I just sit there at like 400% zoom-in"): a trackpad's wheel goes
    // on after the hand has gone, and reaches the canvas as Focus closes.
    const glide = (ctrlKey) => page.evaluate((ctrlKey) => new Promise((done) => {
      const [x, y] = [innerWidth / 2, innerHeight / 2], end = performance.now() + 1200;
      const tick = () => {
        (document.elementFromPoint(x, y) ?? document.body).dispatchEvent(new WheelEvent('wheel', { deltaY: ctrlKey ? -3 : 9, ctrlKey, clientX: x, clientY: y, bubbles: true, cancelable: true }));
        if (performance.now() < end) setTimeout(tick, 16);
        else done();
      };
      tick();
    }), ctrlKey);
    for (const [way, ctrlKey] of [['a scroll', false], ['a pinch', true]]) {
      await focus(page, TOOLED);
      await inFocus(page, project, TOOLED);
      const gliding = glide(ctrlKey);
      await page.waitForTimeout(250);
      await page.keyboard.press('Escape');
      await gliding;
      await backHome(page, project, before, `left while ${way} glides`);
    }
  });

  await test.step('press inside the prompt and release over the canvas, then press and release outside', async () => {
    const region = await focus(page, TOOLED);
    await region.getByRole('button', { name: 'Edit image', exact: true }).click();
    const prompt = region.locator('.in-frame .mention-text').first();
    await expect(prompt).toBeVisible();
    // Edit fits the frame again as it opens its words: pressed once they've stopped moving, the press is on them.
    await expect.poll(() => observeStill(page, 300, '.focus-viewport .in-frame .mention-text'), { message: 'the words at rest' }).toEqual([]);
    const away = await background(page);
    // Each change of the selection as the drag goes, for what's said if none is left at the end.
    await page.evaluate(() => {
      const said = (window.__selections = []);
      document.addEventListener('selectionchange', () => {
        const at = document.getSelection();
        said.push(`${Math.round(performance.now())} ${at?.isCollapsed ? 'caret' : `"${at?.toString().slice(-24)}"`} at ${at?.anchorOffset}-${at?.focusOffset} in ${at?.anchorNode?.parentElement?.className ?? at?.anchorNode?.nodeName}`);
      });
    });
    await drag(page, await inWords(prompt), away);
    await expect.soft(region, 'a press from the prompt let go outside keeps Focus').toBeVisible();
    expect.soft((await selected(page)).length, `and the words it selected, as the drag went: ${(await page.evaluate(() => window.__selections)).join('; ')}`).toBeGreaterThan(0);
    await page.mouse.click(away.x, away.y);
    await backHome(page, project, before, 'a press and release outside');
  });

  await test.step('do the same in the Assets drawer and in the box menu', async () => {
    await page.locator('details.main-menu > summary').click();
    await page.locator('details.main-menu nav').getByRole('link', { name: 'Assets', exact: true }).click();
    const drawer = page.locator('aside.drawer');
    await expect(drawer).toBeVisible();
    await expect(page).toHaveURL(project.url('assets'));
    // Slid in to where it stays.
    await expect.poll(() => drawer.evaluate((aside) => aside.getAnimations({ subtree: true }).filter((one) => one.playState === 'running' && one.timeline === document.timeline && one.effect?.getComputedTiming().iterations !== Infinity).length), { message: 'the drawer slid in' }).toBe(0);
    let away = await openPoint(page);
    await drag(page, await inside(drawer.locator('.drawer-head b').first()), away);
    await expect.soft(drawer, 'a press from the drawer let go on the canvas keeps it').toBeVisible();
    expect.soft((await selected(page)).length, 'and the words it selected').toBeGreaterThan(0);
    await page.mouse.click(away.x, away.y);
    await expect.soft(drawer, 'a press and release on the canvas closes it').toHaveCount(0);
    await expect.soft(page).toHaveURL(project.url());

    const { x, y } = await canvasPoint(page, PICTURE);
    await page.mouse.click(x, y, { button: 'right' });
    const menu = page.getByRole('menu', { name: `${PICTURE}'s menu` });
    await expect(menu).toBeVisible();
    away = await openPoint(page);
    await drag(page, await inside(menu.getByRole('menuitem').first()), away);
    await expect.soft(menu, 'a press from the menu let go on the canvas keeps it').toBeVisible();
    await page.mouse.click(away.x, away.y);
    await expect.soft(menu, 'a press and release on the canvas closes it').toHaveCount(0);
  });

  await test.step('scroll the wheel in Focus', async () => {
    const region = await focus(page, PICTURE);
    await inFocus(page, project, PICTURE);
    const shown = region.locator('.focus-viewer .buffered-image').first();
    const picture = () => shown.evaluate((el) => ({ resolution: el.dataset.displayedResolution ?? null, width: Number(el.dataset.displayedWidth), loaded: Boolean(el.querySelector('img')?.complete && el.querySelector('img').naturalWidth > 0) }));
    const own = { resolution: 'original', width: 1200, loaded: true };
    await expect.poll(picture, { message: 'fitted, its own file drawn' }).toMatchObject(own);
    const imageZoom = () => region.locator('.focus-image-transform').evaluate((el) => new DOMMatrix(getComputedStyle(el).transform).a);
    const canvasHeld = await camera(page);
    // Every frame from here, whether a decoded picture is drawn.
    await page.evaluate(() => {
      const frames = (window.__blank = { total: 0, blank: 0, id: 0 });
      const look = () => {
        const image = document.querySelector('.focus-viewer .buffered-image img');
        frames.total++;
        if (!image?.complete || !image.naturalWidth) frames.blank++;
        frames.id = requestAnimationFrame(look);
      };
      frames.id = requestAnimationFrame(look);
    });
    const viewer = await middle(region.locator('.focus-viewer'));
    await page.mouse.move(viewer.x, viewer.y);
    await page.mouse.wheel(0, -350);
    await expect.poll(imageZoom, { message: 'the wheel zooms the picture' }).toBeGreaterThan(1.9);
    expect.soft(await picture(), 'zoomed, its own file still').toMatchObject(own);
    const frames = await page.evaluate(() => {
      cancelAnimationFrame(window.__blank.id);
      return window.__blank;
    });
    expect.soft(frames.total, 'frames looked at').toBeGreaterThan(0);
    expect.soft(frames.blank, 'frames with no picture drawn').toBe(0);
    expect.soft(sameCamera(await camera(page), canvasHeld), 'the canvas behind stays where it was').toBe(true);
    await expect.soft(region, 'still in Focus').toBeVisible();
    await page.keyboard.press('Escape');
    await backHome(page, project, before, 'Esc after the wheel');
  });

  await test.step("an edit's backdrop: every picture its words use, each a real share, together the frame", async () => {
    // cl-949 (Paul, 11-12: "it should display all 3, as large as possible in the background, full-bleed where possible").
    const region = await focus(page, EDIT);
    await region.getByRole('button', { name: 'Edit image', exact: true }).click();
    await expect(region.locator('.in-frame .mention-text').first()).toBeVisible();
    const used = Object.keys(USED);
    const behind = () => region.locator('.card.here').first().evaluate((card) => {
      const backdrop = card.querySelector('.backdrop');
      if (!backdrop) return { pictures: [], covered: 0 };
      const frame = backdrop.getBoundingClientRect();
      const pictures = [...backdrop.querySelectorAll('.spread-picture')].map((cell) => {
        const r = cell.getBoundingClientRect(), img = cell.querySelector('img');
        return { key: cell.dataset.picture, share: (r.width * r.height) / (frame.width * frame.height), loaded: Boolean(img?.complete && img.naturalWidth > 0) };
      });
      return { pictures, covered: pictures.reduce((sum, one) => sum + one.share, 0) };
    });
    await expect.poll(async () => { const now = await behind(); return used.filter((key) => now.pictures.some((one) => one.key === key && one.loaded)); }, { message: 'all three behind the edit', timeout: 20_000 }).toEqual(used);
    const now = await behind();
    expect.soft(now.pictures.filter((one) => used.includes(one.key) && one.share < 0.12), `each a real share: ${JSON.stringify(now)}`).toEqual([]);
    expect.soft(now.covered, `together the frame: ${JSON.stringify(now)}`).toBeGreaterThanOrEqual(0.93);
  });
});
