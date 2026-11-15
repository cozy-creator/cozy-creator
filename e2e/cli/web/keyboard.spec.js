// keyboard (Paul, 11-15: "make sure the new pixijs canvas is robust"; cl-991's Access contract: "keyboard selection/navigation and focus
// return, accessible names/state/actions … semantic canvas navigation"): the canvas with no mouse, as a keyboard and a screen reader meet
// it. Tab reaches it, a named region whose description says its keys; arrows select every box in one order, each brought into view and said
// by name in a live region; Enter opens Focus and Esc comes back with the box still selected and the keys still on the canvas; Delete
// deletes it and Ctrl+Z restores it; the menu key opens the box's menu, whose items take the keys; Rename is reachable by Tab in Focus. What
// a screen reader is given is kept as a11y.json.
const { writeFileSync } = require('fs');
const { test, expect, canvasIdle, canvasNames, canvasPoint, canvasVisible, selectedNames, png } = require('./fixtures');

// Wide apart, so at 100% some are out of view and an arrow has to bring them in.
const BOXES = [
  { name: 'first_picture', at: { x: 0, y: 0 }, shade: 40 },
  { name: 'second_picture', at: { x: 900, y: 0 }, shade: 90 },
  { name: 'notes', at: { x: 1800, y: 0 }, text: 'Words, in a box of their own.' },
  { name: 'third_picture', at: { x: 0, y: 900 }, shade: 140 },
  { name: 'fourth_picture', at: { x: 900, y: 900 }, shade: 190 },
  { name: 'more_notes', at: { x: 1800, y: 900 }, text: 'More words.' },
];
const NAMES = BOXES.map((box) => box.name);

const region = (page) => page.getByRole('region', { name: 'Project canvas', exact: true });
const onCanvas = (page) => page.evaluate(() => Boolean(document.activeElement?.classList.contains('canvas-root')));

test('keyboard', { tag: '@engines' }, async ({ page, project }, testInfo) => {
  for (const box of BOXES) {
    if (box.text) await project.add({ name: box.name, kind: 'text', fields: { text: box.text }, at: box.at });
    else await project.import(png(box.shade), { name: box.name, at: box.at });
  }
  await expect.poll(async () => (await canvasNames(page)).sort(), { message: 'every box on the canvas' }).toEqual([...NAMES].sort());
  await page.keyboard.press('Shift+Digit0');
  await canvasIdle(page);
  const order = [];

  await test.step('Tab reaches the canvas: a named region whose description says its keys', async () => {
    let reached = false;
    for (let i = 0; i < 60 && !reached; i++) {
      await page.keyboard.press('Tab');
      reached = await onCanvas(page);
    }
    expect(reached, 'Tab reaches the canvas').toBe(true);
    await expect(region(page)).toBeFocused();
    await expect.soft(region(page), 'its description names its keys').toHaveAccessibleDescription(/arrow keys.*Enter/i);
  });

  await test.step('arrows select every box in one order, each brought into view and said by name', async () => {
    const said = region(page).locator('[aria-live]');
    for (let i = 0; i < NAMES.length; i++) {
      await page.keyboard.press('ArrowRight');
      await canvasIdle(page);
      const selected = await selectedNames(page);
      expect(selected, `arrow ${i + 1} selects one box`).toHaveLength(1);
      order.push(selected[0]);
      await expect.soft(said, `${selected[0]} said`).toHaveText(selected[0]);
      expect.soft(await canvasVisible(page, selected[0]), `${selected[0]} brought into view`).toBe(true);
    }
    expect.soft([...order].sort(), 'every box reached').toEqual([...NAMES].sort());
    const back = [order.at(-1)];
    for (let i = 1; i < NAMES.length; i++) {
      await page.keyboard.press('ArrowLeft');
      await canvasIdle(page);
      back.push((await selectedNames(page))[0]);
    }
    expect.soft(back, 'back the same way').toEqual([...order].reverse());
    await expect(region(page), 'the keys stay on the canvas').toBeFocused();
  });

  const chosen = () => order[0];

  await test.step('Enter opens Focus; Esc comes back with it selected and the keys on the canvas', async () => {
    expect(await selectedNames(page)).toEqual([chosen()]);
    await page.keyboard.press('Enter');
    const focused = page.getByRole('region', { name: 'Focused asset' });
    await expect(focused).toHaveAttribute('data-focused-asset', chosen());
    await canvasIdle(page);
    await page.keyboard.press('Escape');
    await expect(focused).toHaveCount(0);
    await canvasIdle(page);
    expect.soft(await selectedNames(page), 'still selected').toEqual([chosen()]);
    expect.soft(await onCanvas(page), 'the keys back on the canvas').toBe(true);
  });

  await test.step('Delete deletes it; Ctrl+Z restores it', async () => {
    await region(page).focus();
    await page.keyboard.press('ArrowRight');
    await page.keyboard.press('ArrowLeft');
    await canvasIdle(page);
    const name = (await selectedNames(page))[0];
    expect(name, 'a box selected by the keys').toBeTruthy();
    await page.keyboard.press('Delete');
    await expect.poll(async () => (await canvasNames(page)).includes(name), { message: `${name} gone from the canvas` }).toBe(false);
    await expect.poll(async () => Boolean(await project.asset(name)), { message: `${name} gone from the project` }).toBe(false);
    await page.keyboard.press('ControlOrMeta+z');
    await expect.poll(async () => (await canvasNames(page)).includes(name), { message: `${name} back` }).toBe(true);
    await expect.poll(async () => Boolean(await project.asset(name)), { message: `${name} back in the project` }).toBe(true);
  });

  await test.step("the menu key opens the box's menu, and its items take the keys", async () => {
    await region(page).focus();
    await page.keyboard.press('ArrowRight');
    await page.keyboard.press('ArrowLeft');
    await canvasIdle(page);
    const name = (await selectedNames(page))[0];
    const menu = page.getByRole('menu', { name: `${name}'s menu`, exact: true });
    await page.keyboard.press('Shift+F10');
    if (!(await menu.isVisible())) await page.keyboard.press('ContextMenu');
    await expect.soft(menu, `Shift+F10 or the menu key opens ${name}'s menu`).toBeVisible({ timeout: 3_000 });
    if (!(await menu.isVisible())) {
      // Its own keys judged all the same: opened as a mouse opens it.
      const { x, y } = await canvasPoint(page, name);
      await page.mouse.click(x, y, { button: 'right' });
      await expect(menu).toBeVisible();
    }
    const items = menu.getByRole('menuitem');
    await expect.soft(items.first(), 'the keys go to its first item').toBeFocused({ timeout: 3_000 });
    await page.keyboard.press('ArrowDown');
    await expect.soft(items.nth(1), 'ArrowDown moves to the next').toBeFocused({ timeout: 3_000 });
    expect.soft(await items.allInnerTexts(), 'Rename and Delete are among them').toEqual(expect.arrayContaining(['Rename', 'Delete']));
    await page.keyboard.press('Escape');
    await expect(menu).toHaveCount(0);
    expect.soft(await onCanvas(page), 'Esc puts the keys back on the canvas').toBe(true);
  });

  await test.step('Rename, reached by Tab in Focus', async () => {
    await region(page).focus();
    await page.keyboard.press('ArrowRight');
    await page.keyboard.press('ArrowLeft');
    await canvasIdle(page);
    const name = (await selectedNames(page))[0];
    await page.keyboard.press('Enter');
    const focused = page.getByRole('region', { name: 'Focused asset' });
    await expect(focused).toHaveAttribute('data-focused-asset', name);
    let reached = false;
    for (let i = 0; i < 40 && !reached; i++) {
      await page.keyboard.press('Tab');
      reached = await page.evaluate(() => document.activeElement?.getAttribute('aria-label') === 'Rename node');
    }
    expect(reached, 'Tab reaches Rename node').toBe(true);
    await page.keyboard.press('Enter');
    const field = page.getByRole('textbox', { name: 'Name', exact: true });
    await expect(field).toBeFocused();
    await page.keyboard.press('ControlOrMeta+a');
    await page.keyboard.type('renamed_by_keys');
    await page.keyboard.press('Enter');
    await expect.poll(async () => Boolean(await project.asset('renamed_by_keys')), { message: 'renamed' }).toBe(true);
    await page.keyboard.press('Escape');
    await expect(focused).toHaveCount(0);
  });

  await test.step('what a screen reader is given, kept', async () => {
    const tree = await region(page).ariaSnapshot();
    writeFileSync(testInfo.outputPath('a11y.json'), JSON.stringify({ region: tree, boxes: await canvasNames(page) }, null, 2));
    expect.soft(tree, 'the canvas is named for a screen reader').toContain('Project canvas');
  });
});
