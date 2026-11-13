// box-actions (cl-973 core cell 14): two boxes copied and pasted over the canvas are two more with names of their own and the same tool, words,
// takes and approval, the same files by digest and no bytes copied, a mention between them naming the copy; one Undo takes both back (cl-924).
// Ctrl+D, an Alt-drag and the menu's Duplicate each make a copy; Ctrl+V in a box's words pastes words only. Star is the API's and leads Files
// with a ★ through Undo and Redo (cl-892); Rename renames the box, its mentions and the API's. Download saves a box's shown take as its name,
// and a take as its own (cl-891).
const { createHash } = require('crypto');
const { readdirSync, readFileSync } = require('fs');
const { join } = require('path');
const { test, expect, box, canvasIdle, focus, middle, png } = require('./fixtures');
const { drag, drawn, rows, rowsOf } = require('./geometry');

const WORDS = 'a lighthouse in <hei.2> beside <other>';
const sha = (bytes) => createHash('sha256').update(bytes).digest('hex');

test('box-actions', async ({ page, project, api }) => {
  // hei: two takes and BiRefNet on them; words: naming hei's second and other; other: a picture; two: two takes, the second approved.
  const takes = { hei: [png(40), png(80)], two: [png(120), png(160)] };
  await project.import(takes.hei[0], { name: 'hei', at: { x: 0, y: 0 } });
  await api.post(project.at('/assets/hei/versions'), { source: await project.upload(takes.hei[1]) });
  await project.add({ name: 'words', kind: 'text', fields: { text: WORDS }, at: { x: 360, y: 0 } });
  await project.import(png(200), { name: 'other', at: { x: 680, y: 0 } });
  await project.import(takes.two[0], { name: 'two', at: { x: 1000, y: 0 } });
  await api.post(project.at('/assets/two/versions'), { source: await project.upload(takes.two[1]) });
  await api.post(project.at('/approve'), { name: 'two', version: '2' });
  const { tools } = await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}&q=birefnet`);
  const cutout = tools.find((tool) => tool.package === 'birefnet' && tool.makes === 'image');
  expect(cutout, 'the Hub lists BiRefNet').toBeTruthy();
  await api.patch(project.at('/assets/hei'), { tool: cutout.id });
  await expect.poll(async () => (await project.asset('hei'))?.tool, { message: 'BiRefNet on hei' }).toBe(cutout.id);
  for (const name of ['hei', 'words', 'other', 'two']) await expect(box(page, name)).toBeVisible();
  await page.getByRole('button', { name: /more zoom options$/ }).click();
  await page.getByRole('menuitem', { name: /^Zoom to fit/ }).click();
  await canvasIdle(page);

  const names = async () => (await project.assets()).map((asset) => asset.name).sort();
  /** Its tool, fields, approval and each take's files by digest. */
  const made = async (name) => {
    const { asset, versions } = await api.get(project.at(`/assets/${encodeURIComponent(name)}`));
    return { tool: asset.tool ?? null, approved: asset.approved ?? null, sha256: asset.sha256 ?? null, takes: versions.map((version) => `${version.number}:${Object.values(version.files).map((file) => file.sha256).sort().join('+')}`) };
  };
  const files = () => readdirSync(join(project.path, 'files')).length;
  const on = (name) => middle(box(page, name).locator('.media').first());
  const clickOn = async (name, modifier) => {
    const { x, y } = await on(name);
    if (modifier) await page.keyboard.down(modifier);
    await page.mouse.click(x, y);
    if (modifier) await page.keyboard.up(modifier);
  };
  /** A box's menu, by a right click on it, and `item` of it. */
  const menuItem = async (name, item) => {
    const { x, y } = await on(name);
    await page.mouse.click(x, y, { button: 'right' });
    await page.getByRole('menu', { name: `${name}'s menu` }).getByRole('menuitem', { name: item, exact: true }).click();
  };
  /** The one name more than `before`, once there is one. */
  const another = async (before, what) => {
    let added;
    await expect.poll(async () => {
      added = (await names()).filter((name) => !before.includes(name));
      return added.length;
    }, { message: `${what} makes one box more` }).toBe(1);
    return added[0];
  };

  await test.step('copy two boxes (one mentions the other) and paste them over the canvas, then Ctrl+Z', async () => {
    const [hei, before, held] = [await made('hei'), await names(), files()];
    await clickOn('hei');
    await clickOn('words', 'Shift');
    await expect(box(page, 'words')).toHaveAttribute('aria-selected', 'true');
    await page.keyboard.press('ControlOrMeta+c');
    // The pointer on open canvas under the row, in the middle of what's visible: the copies land around it, all in view.
    const spot = await page.evaluate(() => {
      const left = document.querySelector('.tools').getBoundingClientRect().right;
      const below = Math.max(...[...document.querySelectorAll('.react-flow__node .card[data-asset]')].map((card) => card.getBoundingClientRect().bottom));
      return { x: Math.round((left + innerWidth) / 2), y: Math.round(below + 220) };
    });
    expect(await page.evaluate(({ x, y }) => document.elementFromPoint(x, y)?.classList.contains('react-flow__pane'), spot), `the pointer on open canvas at ${JSON.stringify(spot)}`).toBe(true);
    await page.mouse.move(spot.x, spot.y);
    await page.keyboard.press('ControlOrMeta+v');
    await expect.poll(names, { message: 'two boxes more' }).toEqual([...before, 'hei_2', 'words_2'].sort());
    for (const name of ['hei_2', 'words_2']) await expect.soft(box(page, name)).toBeVisible();
    expect.soft(await made('hei_2'), "hei's copy: its tool, takes and approval, by the same digests").toEqual(hei);
    expect.soft((await project.asset('hei_2')).fields, 'and its fields').toEqual((await project.asset('hei')).fields);
    expect.soft((await project.asset('words_2')).fields?.text, "the copy's mention names the copy").toBe('a lighthouse in <hei_2.2> beside <other>');
    expect.soft((await project.asset('words')).fields?.text, 'the words copied are as they were').toBe(WORDS);
    expect.soft(files(), 'no bytes copied').toBe(held);
    await page.mouse.move(2, 500);
    await page.keyboard.press('ControlOrMeta+z');
    await expect.poll(names, { message: 'one Undo takes both back' }).toEqual(before);
    for (const name of ['hei_2', 'words_2']) await expect.soft(box(page, name)).toHaveCount(0);
  });

  await test.step('duplicate with Ctrl+D, with Alt-drag and with menu › Duplicate; Ctrl+V inside a prompt', async () => {
    const original = (await project.asset('other')).sha256;
    let before = await names();
    await clickOn('other');
    await page.keyboard.press('ControlOrMeta+d');
    let copy = await another(before, 'Ctrl+D');
    expect.soft((await project.asset(copy)).sha256, `${copy}: Ctrl+D copies other`).toBe(original);

    before = await names();
    const [was, drawnWas] = [(await rows(project)).other, (await drawn(page)).other];
    const from = await middle(box(page, 'other').locator('.head .nm'));
    await page.keyboard.press('Escape');
    await drag(page, from, { x: from.x, y: from.y + 320 }, 'Alt');
    copy = await another(before, 'an Alt-drag');
    // What was carried goes back where it was, as it's drawn too.
    await expect.poll(async () => {
      const now = (await drawn(page)).other;
      return Math.abs(now.x - drawnWas.x) + Math.abs(now.y - drawnWas.y) < 1;
    }, { message: 'other drawn back where it was' }).toBe(true);
    expect.soft((await project.asset(copy)).sha256, `${copy}: an Alt-drag copies other`).toBe(original);
    const after = await rowsOf(project, ['other', copy]);
    expect.soft({ x: after.other.x, y: after.other.y }, 'and leaves other where it was').toEqual({ x: was.x, y: was.y });
    expect.soft(after[copy].y, 'the copy lands where it was let go').toBeGreaterThan(was.y + 200);

    before = await names();
    await menuItem('other', 'Duplicate');
    copy = await another(before, 'menu › Duplicate');
    expect.soft((await project.asset(copy)).sha256, `${copy}: Duplicate copies other`).toBe(original);

    // The words copied above, pasted into a box's words: words only, and no box.
    before = await names();
    const region = await focus(page, 'words');
    const field = region.locator('.mention-text[data-field="text"]').first();
    await field.click();
    await page.keyboard.press('ControlOrMeta+End');
    await page.keyboard.press('ControlOrMeta+v');
    await expect.poll(async () => ((await field.innerText()).match(/lighthouse/g) ?? []).length, { message: 'Ctrl+V in words pastes the words' }).toBe(2);
    await page.keyboard.press('Escape');
    await expect(region).toHaveCount(0);
    await canvasIdle(page);
    expect.soft(await names(), 'and makes no box').toEqual(before);
  });

  await test.step('menu › Star, check Files, then Ctrl+Z and Ctrl+Shift+Z; menu › Rename', async () => {
    await menuItem('other', 'Star');
    await expect.poll(async () => (await project.asset('other')).starred, { message: "Star is the API's" }).toBe(true);
    await page.locator('details.main-menu > summary').click();
    await page.locator('details.main-menu nav').getByRole('link', { name: 'Files', exact: true }).click();
    const rowsListed = page.locator('aside.drawer .assetlist .assetrow');
    const first = rowsListed.first();
    await expect.soft(first, 'Files lists it first').toHaveAttribute('data-asset', 'other');
    await expect.soft(first.getByLabel('Starred'), 'with ★').toHaveText('★');
    await page.keyboard.press('ControlOrMeta+z');
    await expect.poll(async () => Boolean((await project.asset('other')).starred), { message: 'Undo unstars it' }).toBe(false);
    await expect.soft(rowsListed.filter({ has: page.getByLabel('Starred') }), 'with no ★ in Files').toHaveCount(0);
    await page.keyboard.press('ControlOrMeta+Shift+z');
    await expect.poll(async () => (await project.asset('other')).starred, { message: 'Redo stars it again' }).toBe(true);
    await expect.soft(first, 'first in Files again').toHaveAttribute('data-asset', 'other');
    await expect.soft(first.getByLabel('Starred')).toHaveText('★');
    await page.keyboard.press('Escape');
    await expect(page.locator('aside.drawer')).toHaveCount(0);

    await menuItem('other', 'Rename');
    const input = box(page, 'other').locator('input[aria-label="Name"]');
    await input.fill('harbour');
    await input.press('Enter');
    await expect.poll(async () => (await names()).includes('harbour') && !(await names()).includes('other'), { message: 'renamed in the API' }).toBe(true);
    await expect.soft(box(page, 'harbour').locator('.head .nm'), 'and on the box').toHaveText('harbour');
    const text = (await project.asset('words')).fields.text;
    expect.soft(text.includes('<harbour>') && !text.includes('<other>'), `and in its mentions: ${text}`).toBe(true);
  });

  await test.step('menu › Download on a box showing take 2, then on take 1', async () => {
    expect((await project.asset('two')).approved, 'two shows its take 2').toBe(2);
    // The copies made above pushed two along: all of it in view again.
    await page.getByRole('button', { name: /more zoom options$/ }).click();
    await page.getByRole('menuitem', { name: /^Zoom to fit/ }).click();
    await canvasIdle(page);
    let saved = page.waitForEvent('download');
    await menuItem('two', 'Download');
    let file = await saved;
    expect.soft(file.suggestedFilename(), 'a box saves as its name').toBe('two.png');
    expect.soft(sha(readFileSync(await file.path())), 'the take it shows, 2').toBe(sha(takes.two[1]));

    const region = await focus(page, 'two');
    const take = await middle(region.getByRole('button', { name: 'version 1', exact: true }));
    await page.mouse.click(take.x, take.y, { button: 'right' });
    saved = page.waitForEvent('download');
    await page.getByRole('menu', { name: "two.1's menu" }).getByRole('menuitem', { name: 'Download', exact: true }).click();
    file = await saved;
    expect.soft(file.suggestedFilename(), 'a take saves as its own').toBe('two.1.png');
    expect.soft(sha(readFileSync(await file.path())), "take 1's bytes").toBe(sha(takes.two[0]));
    await expect(page.getByRole('menu', { name: "two.1's menu" }), 'Download shuts the menu').toHaveCount(0);
    await page.keyboard.press('Escape');
    await expect(region).toHaveCount(0);
  });
});
