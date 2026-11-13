// delete-undo (cl-973 core cell 7): a box goes by Delete on a selection, Backspace in Focus, its menu's Delete and its trash can, from the canvas
// and the API at once, the history saying so; Delete is a trash can, and ✕ only closes (cl-939). Undo brings each back where it was with its takes
// and approval, Redo takes it again. Backspace in a prompt edits only its words. A deleted box that another reads is named as gone there and
// resolves nowhere, until Undo (cl-919); an API client's delete undoes from the page; and an undo a later edit would clobber is refused, saying
// which.
const { test, expect, box, canvasIdle, focus, middle, png, fixtureTool } = require('./fixtures');
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
  for (const name of all) await expect(box(page, name)).toBeVisible();
  const laid = await rowsOf(project, all);
  const kept = Object.fromEntries(await Promise.all(Object.keys(FOUR).map(async (name) => [name, await project.asset(name)])));
  for (const name of Object.keys(FOUR)) expect(kept[name], `${name}: two takes, the first approved`).toMatchObject({ versions: 2, approved: 1, pinned: true });
  await page.getByRole('button', { name: /more zoom options$/ }).click();
  await page.getByRole('menuitem', { name: /^Zoom to fit/ }).click();
  await canvasIdle(page);

  const history = async () => (await api.get(project.at('/history'))).entries;
  const undoButton = page.locator('.top .history').getByRole('button', { name: 'Undo', exact: true });
  /** `name` gone: from the canvas at once, from the API, and the history says so. */
  const gone = async (name, how) => {
    await expect.soft(box(page, name), `${how}: ${name} off the canvas at once`).toHaveCount(0, { timeout: 1000 });
    await expect.poll(() => project.asset(name), { message: `${how}: ${name} out of the API` }).toBeUndefined();
    await expect.poll(async () => (await history()).some((entry) => entry.words === `Deleted ${name}` && !entry.undone), { message: `${how}: the history says "Deleted ${name}"` }).toBe(true);
  };
  /** `name` back where it was, with its takes and approval. */
  const back = async (name, how) => {
    await expect.poll(() => project.asset(name), { message: `${how}: ${name} back in the API` }).toBeTruthy();
    await expect.soft(box(page, name), `${how}: ${name} back on the canvas`).toBeVisible();
    const [now, row] = [await project.asset(name), (await rowsOf(project, [name]))[name]];
    expect.soft({ versions: now.versions, approved: now.approved, pinned: now.pinned, sha256: now.sha256 }, `${how}: ${name}'s takes and approval`).toEqual({ versions: kept[name].versions, approved: kept[name].approved, pinned: kept[name].pinned, sha256: kept[name].sha256 });
    expect.soft({ x: row.x, y: row.y }, `${how}: ${name} where it was`).toEqual({ x: laid[name].x, y: laid[name].y });
  };
  const on = (name) => middle(box(page, name).locator('.media').first());

  await test.step('delete boxes in four ways: Delete on a selection, Backspace in Focus, menu › Delete, and the trash icon', async () => {
    let at = await on('d1');
    await page.mouse.click(at.x, at.y);
    await expect(box(page, 'd1')).toHaveAttribute('aria-selected', 'true');
    await page.keyboard.press('Delete');
    await gone('d1', 'Delete on a selection');

    const region = await focus(page, 'd2');
    const close = region.getByRole('button', { name: 'Close focus', exact: true });
    expect.soft(await close.locator('svg').getAttribute('data-icon'), "Focus's ✕ only closes").toBe('close');
    await page.keyboard.press('Backspace');
    await expect(region).toHaveCount(0);
    await gone('d2', 'Backspace in Focus');
    await canvasIdle(page);

    at = await on('d3');
    await page.mouse.click(at.x, at.y, { button: 'right' });
    await page.getByRole('menu', { name: "d3's menu" }).getByRole('menuitem', { name: 'Delete', exact: true }).click();
    await gone('d3', 'menu › Delete');

    const trash = box(page, 'd4').locator('.head button[aria-label="Delete"]');
    await page.mouse.move(...Object.values(await on('d4')));
    await expect(trash).toBeVisible();
    expect.soft(await trash.locator('svg').getAttribute('data-icon'), "a box's Delete is a trash can").toBe('trash');
    at = await middle(trash);
    await page.mouse.click(at.x, at.y);
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
    await page.keyboard.press('ControlOrMeta+Shift+z');
    await expect.soft(box(page, 'd2'), 'Redo takes d2 off the canvas').toHaveCount(0, { timeout: 1000 });
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
    await expect.soft(box(page, G.name)).toBeVisible();
  });

  await test.step('delete F, which G mentions', async () => {
    const at = await on('F');
    await page.mouse.click(at.x, at.y);
    await page.keyboard.press('Delete');
    await gone('F', 'F');
    const said = page.locator('.notes .note .line').filter({ hasText: 'Deleted F.' });
    await expect.soft(said, 'the note names who reads it').toHaveText(`Deleted F. ${G.name} reads this and isn't approved yet.`);
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
    await expect.soft(box(page, 'F')).toBeVisible();
    region = await focus(page, G.name);
    await expect.soft(chip, "and G's mention reads it again").not.toHaveClass(/\bmissing\b/);
    await page.keyboard.press('Escape');
    await expect(region).toHaveCount(0);
    await canvasIdle(page);
  });

  await test.step('let an API client delete H, then undo it from the page', async () => {
    await api.delete(project.at('/assets/H'));
    await expect(box(page, 'H'), "the API client's delete shows").toHaveCount(0);
    await expect(undoButton).toHaveAttribute('title', /^Undo: Deleted H, by the API/);
    await undoButton.click();
    await expect.poll(() => project.asset('H'), { message: 'the page undoes it' }).toBeTruthy();
    await expect.soft(box(page, 'H')).toBeVisible();
  });

  await test.step('undo an entry that a later edit would clobber, then reload', async () => {
    const trash = box(page, 'K').locator('.head button[aria-label="Delete"]');
    await page.mouse.move(...Object.values(await on('K')));
    const at = await middle(trash);
    await page.mouse.click(at.x, at.y);
    await gone('K', 'K');
    // Another writer's K, in its place.
    const theirs = await project.import(png(250), { name: 'K' });
    await expect(box(page, 'K')).toBeVisible();
    await page.locator('.notes .note').filter({ hasText: 'Deleted K.' }).getByRole('button', { name: 'Undo', exact: true }).click();
    const refused = page.locator('.notes .note .line').filter({ hasText: "Couldn't undo" });
    await expect.soft(refused, 'the undo is refused').toHaveCount(1);
    await expect.soft(refused, 'naming what it would clobber').toContainText('K is in the project again');
    await expect.soft(refused, 'and the edit since').toContainText('Added K');
    await page.reload();
    await expect(box(page, 'K')).toBeVisible();
    expect.soft((await project.asset('K'))?.sha256, "K is the other writer's").toBe(theirs.sha256);
  });
});
