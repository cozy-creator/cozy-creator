// mention (cl-973 core cell 9): names in a prompt, on a local picture tool (project/tool.py's still, which takes pictures only). `<am` offers
// what it matches and a pick is a chip; a box let go on the box is its `<name>`; names pasted, plain or rich, are chips at once; the caret
// never lands in a chip; a video it can't use is struck through, saying why on hover (Paul, 11-12); a rename is the chip's and the stored
// field's; a selection of three let go on it appends three. Rendered, its prompt holds Amy's file and only Walk's name, and `cozy run watch`
// says so once.
const { test, expect, box, focus, middle, fixtureTool, MP4, png } = require('./fixtures');
const { carry, carryBox, fit, leave } = require('./hands');

const STILL = fixtureTool('tool.py/still');
const RUN_MS = 120_000;

/** A paste as a browser hands it over: plain text, and with `html` as a rich-text program copies it. */
const paste = (textbox, text, html) => textbox.evaluate((element, { text, html }) => {
  const data = new DataTransfer();
  data.setData('text/plain', text);
  if (html) data.setData('text/html', html);
  element.dispatchEvent(new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true }));
}, { text, html });

/** The names its chips stand for, in order. */
const chips = (textbox) => textbox.locator('[data-mention]').evaluateAll((all) => all.map((one) => one.dataset.mention));

/** Where the caret is: in a chip, just after one (its name), or elsewhere. */
const caret = (page) => page.evaluate(() => {
  const at = getSelection();
  if (!at?.rangeCount) return 'none';
  const node = at.anchorNode;
  const inside = (node instanceof Element ? node : node?.parentElement)?.closest('[data-mention]');
  if (inside) return `inside ${inside.dataset.mention}`;
  const before = node instanceof Element ? node.childNodes[at.anchorOffset - 1] : node?.nodeType === Node.TEXT_NODE && at.anchorOffset === 0 ? node.previousSibling : null;
  return before?.dataset?.mention ? `after ${before.dataset.mention}` : 'in words';
});

test('mention', async ({ page, project, api, cli }) => {
  await project.import(png(160), { name: 'amy', at: { x: 0, y: 0 } });
  await project.import(MP4, { name: 'walk', at: { x: 360, y: 0 } });
  await project.import(png(60), { name: 'bea', at: { x: 720, y: 0 } });
  await project.add({ name: 'still', tool: STILL, kind: 'image', at: { x: 360, y: 460 } });
  await expect.poll(async () => (await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}`)).tools.some((tool) => tool.local && tool.id === STILL), { message: 'the project lists its local tool' }).toBe(true);
  for (const name of ['amy', 'walk', 'bea', 'still']) await expect(box(page, name)).toBeVisible();
  await fit(page);
  const region = page.getByRole('region', { name: 'Focused asset' });
  const words = region.locator('.in-frame .mention-text[data-field="instructions"]');
  const said = async () => (await project.asset('still'))?.fields?.instructions ?? '';

  await test.step('`<am` offers what it matches, and a pick is a chip', async () => {
    await focus(page, 'still');
    await expect(words).toBeFocused();
    await page.keyboard.type('A sheet of <am');
    const offered = page.getByRole('listbox');
    await expect(offered.getByRole('option')).toHaveText(['amy']);
    await page.keyboard.press('Enter');
    await expect(words.locator('.chip[data-mention="amy"]')).toHaveText('<Amy>');
    await expect.soft(offered, 'the list goes with the pick').toHaveCount(0);
    await page.keyboard.type('and');
    await expect.poll(said, { message: 'saved as its name' }).toBe('A sheet of <amy> and');
    expect.soft((await project.asset('still')).making ?? [], 'Enter on a name renders nothing').toEqual([]);
  });

  await test.step('Walk let go on the box, and names pasted plain and rich: chips at once', async () => {
    await leave(page);
    await carryBox(page, 'walk', await middle(box(page, 'still').locator('.media')));
    await expect.poll(said, { message: 'the drop is its name' }).toBe('A sheet of <amy> and <walk>');
    await focus(page, 'still');
    await expect(words).toBeFocused();
    const plain = ' with <amy> and <walk.1>';
    await paste(words, plain);
    expect.soft(await chips(words), 'plain: each a chip at once').toEqual(['amy', 'walk', 'amy', 'walk.1']);
    await paste(words, plain, `<p><b>${plain.replace(/</g, '&lt;')}</b></p>`);
    expect.soft(await chips(words), 'rich: each a chip at once').toEqual(['amy', 'walk', 'amy', 'walk.1', 'amy', 'walk.1']);
    await expect.poll(said, { message: 'the file holds the words as pasted' }).toBe(`A sheet of <amy> and <walk>${plain}${plain}`);
  });

  await test.step("a video the tool can't use is struck through, saying why", async () => {
    const walk = words.locator('.chip[data-mention="walk"]');
    await expect(walk).toHaveClass(/\bunused\b/);
    expect.soft(await walk.evaluate((el) => getComputedStyle(el).textDecorationLine), 'struck through').toContain('line-through');
    await expect.soft(walk, 'why, on hover').toHaveAttribute('title', /can't use videos/);
    await expect.soft(words.locator('.chip[data-mention="amy"]').first(), 'a picture it uses').not.toHaveClass(/\bunused\b/);
  });

  await test.step('the caret never lands in a chip', async () => {
    // Just before the first chip, then across it each way.
    await words.evaluate((el) => {
      const chip = el.querySelector('[data-mention]');
      getSelection().collapse(el, [...el.childNodes].indexOf(chip));
    });
    await page.keyboard.press('ArrowRight');
    expect.soft(await caret(page), 'arrowed across it').toBe('after amy');
    await page.keyboard.press('ArrowLeft');
    expect.soft(await caret(page), 'and back').not.toMatch(/^inside/);
    await words.locator('.chip[data-mention="amy"]').first().click();
    expect.soft(await caret(page), 'a click on it puts the caret after it').toBe('after amy');
  });

  await test.step('Amy renamed Ann: the chip and the stored field', async () => {
    await leave(page);
    await box(page, 'amy').locator('.nm').click();
    const field = page.getByRole('textbox', { name: 'Name', exact: true });
    await field.fill('ann');
    await field.press('Enter');
    await expect(box(page, 'ann')).toBeVisible();
    await expect.poll(said, { message: 'the stored field names Ann' }).toBe('A sheet of <ann> and <walk> with <ann> and <walk.1> with <ann> and <walk.1>');
    await focus(page, 'still');
    await expect.soft(words.locator('.chip[data-mention="ann"]')).toHaveText(['<Ann>', '<Ann>', '<Ann>']);
    await expect.soft(words.locator('.chip[data-mention="amy"]')).toHaveCount(0);
  });

  await test.step('a selection of three let go on it appends three', async () => {
    await leave(page);
    const row = await Promise.all(['ann', 'walk', 'bea'].map((name) => box(page, name).boundingBox()));
    // From open canvas under the row's left end to past its top right: the rectangle touches the three and not the box under them.
    const from = { x: row[0].x - 16, y: Math.max(...row.map((at) => at.y + at.height)) + 16 };
    const to = { x: row[2].x + row[2].width + 16, y: Math.min(...row.map((at) => at.y)) - 16 };
    expect(await page.evaluate(({ x, y }) => document.elementFromPoint(x, y)?.classList.contains('react-flow__pane'), from), 'the rectangle starts on open canvas').toBe(true);
    await page.keyboard.down('Shift');
    await page.mouse.move(from.x, from.y);
    await page.mouse.down();
    await page.mouse.move(to.x, to.y, { steps: 8 });
    await page.mouse.up();
    await page.keyboard.up('Shift');
    await expect(page.locator('.react-flow__node .card[aria-selected="true"]')).toHaveCount(3);
    const was = await said();
    const held = page.locator('.react-flow__nodesselection-rect');
    await carry(page, await middle(held), await middle(box(page, 'still').locator('.media')));
    await expect.poll(async () => (await said()).startsWith(was) && (await said()).slice(was.length).match(/<[^>]+>/g)?.sort(), { message: 'three refs appended' }).toEqual(['<ann>', '<bea>', '<walk>']);
  });

  await test.step("rendered: Ann's file, Walk only by name, said once", async () => {
    await focus(page, 'still');
    const before = (await project.asset('still')).versions ?? 0;
    await region.getByRole('button', { name: 'Render', exact: true }).click();
    await expect.poll(async () => (await project.asset('still'))?.versions ?? 0, { message: 'its take', timeout: RUN_MS }).toBe(before + 1);
    const run = (await project.asset('still')).prefixes.at(-1).run;
    const watched = await cli.json('--full', 'run', 'watch', String(run), '--once');
    const sent = watched.request?.input?.instructions;
    expect.soft([...new Set(sent?.references?.map((reference) => reference.key))].sort(), "the prompt's files: Ann's, and Bea's").toEqual(['ann', 'bea']);
    expect.soft(sent?.text ?? '', 'Walk is no reference').not.toMatch(/<walk/);
    const prompt = watched.result?.prompt ?? '';
    expect.soft(prompt, 'what the tool read: Ann by her file').toContain('<ann>');
    expect.soft(prompt, 'and Walk by name only').not.toMatch(/<walk/);
    expect.soft(prompt).toMatch(/\bwalk\b/);
    const told = await cli('run', 'watch', String(run));
    const lines = `${told.stdout}\n${told.stderr}`.split('\n').filter((line) => line.includes('<walk> is left out:'));
    expect.soft(told.code, 'cozy run watch').toBe(0);
    expect.soft(lines, 'said once, as a note').toHaveLength(1);
    expect.soft(lines[0] ?? '', 'a note, not a warning').toMatch(/note: /);
  });
});
