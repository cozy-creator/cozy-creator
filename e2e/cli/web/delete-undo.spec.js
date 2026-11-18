// delete-undo (cl-973 core cell 7): a box goes by Delete on a selection, Backspace in Focus, its menu's Delete and its trash can, from the canvas
// and the API at once, the history saying so; Delete is a trash can, and ✕ only closes (cl-939). Undo brings each back where it was with its takes
// and approval, Redo takes it again. Backspace in a prompt edits only its words. A deleted box that another reads is named as gone there and
// resolves nowhere, until Undo (cl-919); an API client's delete undoes from the page; and an undo a later edit would clobber is refused, saying
// which.
const { test, expect, canvasCard, canvasIdle, focus, middle, openPoint, png, fixtureTool, canvasNames, canvasPoint, canvasVisible, selectedNames, hoverBox } = require('./fixtures');

/** A box's own control (`delete`, `cancel`) where the canvas draws it, read with nothing under the pointer. */
async function controlPoint(page, name, part) {
  const away = await openPoint(page);
  await page.mouse.move(away.x, away.y);
  let point = null;
  await expect.poll(async () => {
    const control = (await canvasCard(page, name))?.rendered?.controls?.find((one) => one.part === part);
    point = control ? { x: control.x + control.width / 2, y: control.y + control.height / 2 } : null;
    return Boolean(point);
  }, { message: `${name}'s ${part} drawn` }).toBe(true);
  return point;
}
/** Its trash pressed where it's drawn: the old canvas mounts a DOM card there under the pointer, the new draws it (cl-939: a trash can). */
async function pressTrash(page, name) {
  const at = await controlPoint(page, name, 'delete');
  await page.mouse.move(at.x, at.y);
  const trash = page.locator(`.canvas-node .card[data-asset=${JSON.stringify(name)}] .head button[aria-label="Delete"]`);
  const icon = (await trash.count()) ? await trash.locator('svg').getAttribute('data-icon') : (await canvasCard(page, name))?.rendered?.parts?.delete?.icon ?? null;
  expect.soft(icon, "a box's Delete is a trash can").toBe('trash');
  await page.mouse.click(at.x, at.y);
}
const { rowsOf } = require('./geometry');

// Four pictures to delete four ways, each with two takes and the first approved; then F, G reading it, H for an API client, K to clobber.
const FOUR = { d1: { x: 0, y: 0 }, d2: { x: 300, y: 0 }, d3: { x: 600, y: 0 }, d4: { x: 900, y: 0 } };
const REST = { F: { x: 0, y: 420 }, H: { x: 680, y: 420 }, K: { x: 980, y: 420 } };
const G = { name: 'G', at: { x: 300, y: 420 }, words: 'a hat on <F> by the sea' };

test('delete-undo', async ({ page, project, api, guards }) => {
  // The clobbering undo, refused on purpose: Chrome says so in its console.
  guards.allow(/status of 4\d\d\b.*\/history\/undo\b/);
  let shade = 20;
  for (const [name, at] of Object.entries({ ...FOUR, ...REST })) {
    await project.import(png((shade += 23)), { name, at });
    if (!(name in FOUR)) continue;
    await api.post(project.at(`/assets/${name}/versions`), { source: await project.upload(png((shade += 23))) });
    await api.post(project.at('/approve'), { name, version: '1' });
  }
  await project.add({ name: G.name, tool: fixtureTool('draw.py/draw'), kind: 'image', fields: { instructions: G.words }, at: G.at });
  const all = [...Object.keys(FOUR), ...Object.keys(REST), G.name];
  for (const name of all) await expect.poll(() => canvasVisible(page, name)).toBe(true);
  const laid = await rowsOf(project, all);
  const kept = Object.fromEntries(await Promise.all(Object.keys(FOUR).map(async (name) => [name, await project.asset(name)])));
  for (const name of Object.keys(FOUR)) expect(kept[name], `${name}: two takes, the first approved`).toMatchObject({ versions: 2, approved: 1, pinned: true });
  await page.getByRole('button', { name: /more zoom options$/ }).click();
  await page.getByRole('menuitem', { name: /^Zoom to fit/ }).click();
  await canvasIdle(page);

  const history = async () => (await api.get(project.at('/history'))).entries;
  const undoButton = page.locator('.top .history').getByRole('button', { name: 'Undo', exact: true });
  /** From now, the page notes how long after the next press (its first keydown or pointerdown, as the page took it) it first draws a frame
   *  without `name`: "at once" is judged from the press on the page's own clock, never by a round trip to it (e2e 44142655129: d2 was gone
   *  in the first sample, which a busy page took 801 ms to answer) nor by the cell finding what to press (44340216031: 1,382 ms from the
   *  watch, 1.34 s of it finding K's trash; K was gone within 45 ms of the click). */
  const watch = (name) => page.evaluate((name) => {
    const inspection = document.querySelector('.canvas-root').canvasInspection;
    window.goneIn = { ...window.goneIn, [name]: null };
    let pressed = null;
    const press = (event) => { pressed ??= event.timeStamp; };
    const types = ['keydown', 'pointerdown'];
    for (const type of types) addEventListener(type, press, { capture: true });
    const look = () => {
      if (inspection.names().includes(name)) return requestAnimationFrame(look);
      for (const type of types) removeEventListener(type, press, { capture: true });
      window.goneIn[name] = pressed === null ? 'no press' : performance.now() - pressed;
    };
    requestAnimationFrame(look);
  }, name);
  /** How long after its press `name` left the canvas, in ms, once it has; said on every run. */
  const offIn = async (name, how) => {
    await expect.poll(() => page.evaluate((name) => window.goneIn?.[name] ?? null, name), { message: `${how}: ${name} off the canvas` }).not.toBeNull();
    const ms = await page.evaluate((name) => window.goneIn[name], name);
    expect(ms, `${how}: ${name} left the canvas with no key or pointer press after its watch`).not.toBe('no press');
    test.info().annotations.push({ type: `${name}-off-canvas-ms`, description: `${Math.round(ms)} (${how})` });
    return ms;
  };
  /** `name` gone: from the canvas at once (from its press), from the API, and the history says so. */
  const gone = async (name, how) => {
    expect.soft(await offIn(name, how), `${how}: ${name} off the canvas at once, in ms`).toBeLessThan(1000);
    await expect.poll(() => project.asset(name), { message: `${how}: ${name} out of the API` }).toBeUndefined();
    await expect.poll(async () => (await history()).some((entry) => entry.words === `Deleted ${name}` && !entry.undone), { message: `${how}: the history says "Deleted ${name}"` }).toBe(true);
  };
  /** `name` back where it was, with its takes and approval. */
  const back = async (name, how) => {
    await expect.poll(() => project.asset(name), { message: `${how}: ${name} back in the API` }).toBeTruthy();
    await expect.configure({ soft: true }).poll(() => canvasVisible(page, name), { message: `${how}: ${name} back on the canvas` }).toBe(true);
    const [now, row] = [await project.asset(name), (await rowsOf(project, [name]))[name]];
    expect.soft({ versions: now.versions, approved: now.approved, pinned: now.pinned, sha256: now.sha256 }, `${how}: ${name}'s takes and approval`).toEqual({ versions: kept[name].versions, approved: kept[name].approved, pinned: kept[name].pinned, sha256: kept[name].sha256 });
    expect.soft({ x: row.x, y: row.y }, `${how}: ${name} where it was`).toEqual({ x: laid[name].x, y: laid[name].y });
  };
  const on = (name) => canvasPoint(page, name);

  await test.step('delete boxes in four ways: Delete on a selection, Backspace in Focus, menu › Delete, and the trash icon', async () => {
    let at = await on('d1');
    await page.mouse.click(at.x, at.y);
    await expect.poll(() => selectedNames(page)).toContain('d1');
    await watch('d1');
    await page.keyboard.press('Delete');
    await gone('d1', 'Delete on a selection');

    const region = await focus(page, 'd2');
    const close = region.getByRole('button', { name: 'Close focus', exact: true });
    expect.soft(await close.locator('svg').getAttribute('data-icon'), "Focus's ✕ only closes").toBe('close');
    await watch('d2');
    await page.keyboard.press('Backspace');
    await expect(region).toHaveCount(0);
    await gone('d2', 'Backspace in Focus');
    await canvasIdle(page);

    at = await on('d3');
    await page.mouse.click(at.x, at.y, { button: 'right' });
    await watch('d3');
    await page.getByRole('menu', { name: "d3's menu" }).getByRole('menuitem', { name: 'Delete', exact: true }).click();
    await gone('d3', 'menu › Delete');

    await watch('d4');
    await pressTrash(page, 'd4');
    await gone('d4', 'the trash icon');
  });

  await test.step('Ctrl+Z twice, the top bar Undo, then Ctrl+Shift+Z', async () => {
    await page.mouse.move(2, 500);
    await page.keyboard.press('ControlOrMeta+z');
    await back('d4', 'the first Ctrl+Z');
    await page.keyboard.press('ControlOrMeta+z');
    await back('d3', 'the second Ctrl+Z');
    await expect(undoButton).toHaveAttribute('title', /^Undo: Deleted d2, by this tab/);
    await undoButton.click();
    await back('d2', "the top bar's Undo");
    await watch('d2');
    await page.keyboard.press('ControlOrMeta+Shift+z');
    expect.soft(await offIn('d2', 'Redo'), 'Redo takes d2 off the canvas at once, in ms').toBeLessThan(1000);
    await expect.poll(() => project.asset('d2'), { message: 'Redo deletes d2 again' }).toBeUndefined();
  });

  await test.step('press Backspace inside a prompt', async () => {
    const region = await focus(page, G.name);
    const prompt = region.locator('.in-frame .mention-text').first();
    await prompt.click();
    await page.keyboard.press('End');
    await page.keyboard.press('Backspace');
    await expect.soft(region, 'Focus stays').toBeVisible();
    await page.keyboard.press('Escape');
    await expect(region).toHaveCount(0);
    await canvasIdle(page);
    await expect.poll(async () => (await project.asset(G.name))?.fields?.instructions, { message: 'Backspace takes the last letter of the words, and no box' }).toBe(G.words.slice(0, -1));
    await expect.configure({ soft: true }).poll(() => canvasVisible(page, G.name)).toBe(true);
  });

  await test.step('delete F, which G mentions', async () => {
    const at = await on('F');
    await page.mouse.click(at.x, at.y);
    await watch('F');
    await page.keyboard.press('Delete');
    await gone('F', 'F');
    const said = page.locator('.notes .note .line').filter({ hasText: 'Deleted F.' });
    await expect.soft(said, 'the note names who reads it').toHaveText(`Deleted F. ${G.name} uses it and isn't approved yet.`);
    const line = await said.evaluate((el) => el.getBoundingClientRect().height / (parseFloat(getComputedStyle(el).lineHeight) || parseFloat(getComputedStyle(el).fontSize) * 1.3));
    expect.soft(Math.round(line), 'in one line').toBe(1);
    const answer = await api.raw('GET', project.at('/assets/F'));
    expect.soft(answer.status, 'F resolves nowhere: its route 404s').toBe(404);

    let region = await focus(page, G.name);
    const chip = region.locator('.in-frame .mention-text .chip[data-mention="F"]');
    await expect.soft(chip, "G's mention of F is struck").toHaveClass(/\bmissing\b/);
    await expect.soft(chip, 'saying F was deleted').toHaveAttribute('title', 'F was deleted');
    await page.keyboard.press('Escape');
    await expect(region).toHaveCount(0);
    await canvasIdle(page);

    await page.keyboard.press('ControlOrMeta+z');
    await expect.poll(() => project.asset('F'), { message: 'Undo brings F back' }).toBeTruthy();
    await expect.configure({ soft: true }).poll(() => canvasVisible(page, 'F')).toBe(true);
    region = await focus(page, G.name);
    await expect.soft(chip, "and G's mention reads it again").not.toHaveClass(/\bmissing\b/);
    await page.keyboard.press('Escape');
    await expect(region).toHaveCount(0);
    await canvasIdle(page);
  });

  await test.step('let an API client delete H, then undo it from the page', async () => {
    await api.delete(project.at('/assets/H'));
    await expect.poll(async () => (await canvasNames(page)).includes('H'), { message: "the API client's delete shows" }).toBe(false);
    await expect(undoButton).toHaveAttribute('title', /^Undo: Deleted H, by the API/);
    await undoButton.click();
    await expect.poll(() => project.asset('H'), { message: 'the page undoes it' }).toBeTruthy();
    await expect.configure({ soft: true }).poll(() => canvasVisible(page, 'H')).toBe(true);
  });

  await test.step('undo an entry that a later edit would clobber, then reload', async () => {
    await watch('K');
    await pressTrash(page, 'K');
    await gone('K', 'K');
    // Another writer's K, in its place.
    const theirs = await project.import(png(250), { name: 'K' });
    await expect.poll(() => canvasVisible(page, 'K')).toBe(true);
    await page.locator('.notes .note').filter({ hasText: 'Deleted K.' }).getByRole('button', { name: 'Undo', exact: true }).click();
    const refused = page.locator('.notes .note .line').filter({ hasText: "Couldn't undo" });
    await expect.soft(refused, 'the undo is refused').toHaveCount(1);
    await expect.soft(refused, 'naming what it would clobber').toContainText('K is in the project again');
    await expect.soft(refused, 'and the edit since').toContainText('Added K');
    await page.reload();
    await expect.poll(() => canvasVisible(page, 'K')).toBe(true);
    expect.soft((await project.asset('K'))?.sha256, "K is the other writer's").toBe(theirs.sha256);
  });
});
