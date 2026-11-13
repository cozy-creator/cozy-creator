// type (cl-973 core cell 8): typing into boxes. 200 characters into a Text box, each shown the frame it's typed, in order, the caret at its
// end, and the API holding exactly that after a pause. A tool box's prompt typed in Focus and left at once, and typed and reloaded at once,
// keeps every character (cl-904). Another writer's words while the page's are on their way, and a second tab typing in the same box: both
// stand, merged by character, with nothing asked. H3's Shot: Summary before its Beats, + Beat adds one, and a beat holds six lines with no
// scroll of its own.
const { test, expect, box, focus } = require('./fixtures');
const { fit, leave } = require('./hands');

const SENTENCE = 'The quick brown fox jumps over 12 lazy dogs; ';
const TWO_HUNDRED = `${SENTENCE.repeat(5).slice(0, 199)}.`;
const SIX = ['Rain on the harbour.', 'A gull lifts off.', 'Ropes creak.', 'A bell, far off.', 'Lamps come on.', 'She looks back.'];

/** What `textbox` shows and where its caret is, each frame after each keystroke: recorded in the page as it types. */
const recordKeys = (textbox) => textbox.evaluate((el) => {
  const seen = (window.keystrokes = { inputs: 0, frames: [] });
  const caret = () => {
    const selection = getSelection();
    if (!selection.rangeCount || !el.contains(selection.anchorNode)) return null;
    const before = document.createRange();
    before.selectNodeContents(el);
    before.setEnd(selection.anchorNode, selection.anchorOffset);
    return before.toString().length;
  };
  el.addEventListener('input', () => {
    const n = (seen.inputs += 1);
    requestAnimationFrame(() => seen.frames.push({ n, text: el.textContent.replace(/ /g, ' '), caret: caret() }));
  });
});

/** A page's writes of `name` held until `release()`: each PATCH asked is in `asked`. */
async function holdWrites(page, name) {
  let release;
  const held = new Promise((done) => (release = done));
  const asked = [];
  const pattern = (url) => new URL(url).pathname.endsWith(`/assets/${name}`);
  const handler = async (route) => {
    if (route.request().method() !== 'PATCH') return route.fallback();
    asked.push(route.request().postDataJSON());
    await held;
    await route.continue();
  };
  await page.route(pattern, handler);
  return { asked, release: async () => { release(); await page.unroute(pattern, handler); } };
}

test('type', { tag: '@engines' }, async ({ page, context, project, api, signIn }) => {
  const qwen = (await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}&q=qwen`)).tools.find((tool) => tool.package === 'qwen-image-2' && tool.makes === 'image');
  const prompt = qwen?.fields.find((field) => field.type === 'prompt')?.field;
  expect(prompt, 'the Hub lists Qwen Image 2 with a prompt').toBeTruthy();
  await project.add({ name: 'note', kind: 'text', at: { x: 0, y: 0 } });
  await project.add({ name: 'ask', tool: qwen.id, kind: 'image', at: { x: 420, y: 0 } });
  await project.add({ name: 'shared', kind: 'text', fields: { text: 'What Calcifer held.' }, at: { x: 840, y: 0 } });
  await project.add({ name: 'scene', tool: 'minimax-h3/generate', kind: 'video', at: { x: 0, y: 420 } });
  for (const name of ['note', 'ask', 'shared', 'scene']) await expect(box(page, name)).toBeVisible();
  await fit(page);
  const region = page.getByRole('region', { name: 'Focused asset' });
  const fieldOf = async (name, field) => (await project.asset(name))?.fields?.[field];
  const shown = (textbox) => textbox.evaluate((el) => el.textContent.replace(/ /g, ' '));

  await test.step('200 characters into a Text box', async () => {
    await focus(page, 'note');
    const text = region.getByRole('textbox', { name: 'Text', exact: true });
    await expect(text, 'Focus puts the caret in its words').toBeFocused();
    await recordKeys(text);
    await page.keyboard.type(TWO_HUNDRED);
    await expect.poll(() => shown(text)).toBe(TWO_HUNDRED);
    const { inputs, frames } = await page.evaluate(() => window.keystrokes);
    expect.soft(inputs, 'one input a keystroke').toBe(TWO_HUNDRED.length);
    expect.soft(frames.filter((frame) => frame.text.length < frame.n), 'a keystroke not shown the frame after it').toEqual([]);
    expect.soft(frames.filter((frame) => !TWO_HUNDRED.startsWith(frame.text)), 'shown out of order').toEqual([]);
    expect.soft(frames.filter((frame) => frame.caret !== frame.text.length), 'the caret anywhere but its end').toEqual([]);
    await expect.poll(() => fieldOf('note', 'text'), { message: 'after a pause, the API holds exactly what was typed' }).toBe(TWO_HUNDRED);
  });

  await test.step("a tool box's prompt, left at once and reloaded at once", async () => {
    await leave(page);
    await focus(page, 'ask');
    const words = region.locator('.in-frame [role=textbox]').first();
    await expect(words).toBeFocused();
    const left = 'Typed, and Focus left at once.';
    await page.keyboard.type(left);
    await page.keyboard.press('Escape');
    await expect(region).toHaveCount(0);
    await expect.poll(() => fieldOf('ask', prompt), { message: 'left at once, every character saved' }).toBe(left);
    await focus(page, 'ask');
    await expect.soft(words, 'there when it is opened again').toHaveText(left);
    await expect(words).toBeFocused();
    const reloaded = ' Then the page reloaded at once.';
    await page.keyboard.type(reloaded);
    await page.reload();
    await expect(region).toHaveAttribute('data-focused-asset', 'ask');
    await expect.poll(() => fieldOf('ask', prompt), { message: 'reloaded at once, every character saved' }).toBe(left + reloaded);
    await expect.soft(words).toHaveText(left + reloaded);
  });

  const merged = 'Theirs. What Calcifer held. Mine.';
  await test.step("another writer's words while the page's are on their way: both stand", async () => {
    await leave(page);
    await focus(page, 'shared');
    const text = region.getByRole('textbox', { name: 'Text', exact: true });
    await expect(text).toBeFocused();
    const hold = await holdWrites(page, 'shared');
    await page.keyboard.type(' Mine.');
    await expect.poll(() => hold.asked.length, { message: "the page's save asked" }).toBeGreaterThan(0);
    await api.patch(project.at('/assets/shared'), { fields: { text: 'Theirs. What Calcifer held.' } });
    await expect.poll(() => fieldOf('shared', 'text')).toBe('Theirs. What Calcifer held.');
    await expect.soft(text, "the page's unsent words stand").toHaveText('What Calcifer held. Mine.');
    await hold.release();
    await expect.poll(() => fieldOf('shared', 'text'), { message: 'both, merged by character' }).toBe(merged);
    await expect.soft(text, 'the page shows what was merged').toHaveText(merged);
    await expect.soft(page.getByRole('alertdialog'), 'nothing asked').toHaveCount(0);
    await expect.soft(page.getByRole('button', { name: /keep mine|keep theirs|take the file/i }), 'nothing asked').toHaveCount(0);
  });

  await test.step('two tabs typing in one box both keep theirs', async () => {
    const other = await context.newPage();
    await signIn(other, project.url('assets/shared'));
    const there = other.getByRole('region', { name: 'Focused asset' }).getByRole('textbox', { name: 'Text', exact: true });
    const here = region.getByRole('textbox', { name: 'Text', exact: true });
    await expect(there).toHaveText(merged);
    const hold = await holdWrites(page, 'shared');
    await here.click();
    await page.keyboard.press('ControlOrMeta+End');
    await page.keyboard.type(' A-end.');
    await expect.poll(() => hold.asked.length, { message: "the first tab's save asked" }).toBeGreaterThan(0);
    await there.click();
    await other.keyboard.press('ControlOrMeta+Home');
    await other.keyboard.type('B-start. ');
    await expect.poll(() => fieldOf('shared', 'text'), { message: "the second tab's words saved" }).toBe(`B-start. ${merged}`);
    await hold.release();
    const both = `B-start. ${merged} A-end.`;
    await expect.poll(() => fieldOf('shared', 'text'), { message: "both tabs' words, merged by character" }).toBe(both);
    await expect.soft(here, 'the first tab shows both').toHaveText(both);
    await expect.soft(there, 'the second tab shows both').toHaveText(both);
    await other.close();
  });

  await test.step("H3's Shot: Summary before its Beats, + Beat, and six lines in a beat", async () => {
    await leave(page);
    await focus(page, 'scene');
    const heads = region.locator('.in-frame .frame-sections .beat-toggle b');
    await expect(heads.first()).toHaveText('Summary');
    const summary = region.getByRole('textbox', { name: 'Summary', exact: true });
    await expect(summary).toBeFocused();
    await page.keyboard.type('A harbour at dusk.');
    await expect.poll(() => fieldOf('scene', 'summary')).toBe('A harbour at dusk.');
    const rows = region.locator('.in-frame .beats-edit[data-field="beats"] > .beat');
    const before = await rows.count();
    const add = region.locator('.in-frame .beats-last').getByRole('button', { name: '+ Beat', exact: true });
    await add.click();
    await add.click();
    await expect(rows).toHaveCount(before + 2);
    await expect.poll(async () => (await fieldOf('scene', 'beats'))?.length, { message: 'two beats added' }).toBe(before + 2);
    const sections = await heads.allTextContents();
    expect.soft(sections, 'Summary, then each beat').toEqual(['Summary', ...Array.from({ length: before + 2 }, (_, i) => `Beat ${i + 1}`)]);
    const last = before + 1;
    const beat = region.locator(`.mention-text[data-field="beats.${last}.description"]`);
    await expect(beat, 'the beat added is open to write in').toBeVisible();
    await beat.click();
    for (const [i, line] of SIX.entries()) {
      if (i) await page.keyboard.press('Shift+Enter');
      await page.keyboard.type(line);
    }
    await expect.poll(async () => (await fieldOf('scene', 'beats'))?.[last]?.description, { message: 'six lines in the beat' }).toBe(SIX.join('\n'));
    const measured = await beat.evaluate((el) => {
      const style = getComputedStyle(el);
      const line = Number.parseFloat(style.lineHeight) || Number.parseFloat(style.fontSize) * 1.4;
      return { lines: el.clientHeight / line, scroll: el.scrollHeight - el.clientHeight };
    });
    expect.soft(measured.lines, 'the beat as tall as six lines').toBeGreaterThanOrEqual(6);
    expect.soft(measured.scroll, 'no scroll of its own, px').toBeLessThanOrEqual(1);
  });
});
