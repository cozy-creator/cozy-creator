// hints (cl-1188; Paul, 11-17: "pre-filled text in text boxes (not real text, but suggested text) should be clearly ghosted in the UI, so
// it's obvious that's not something there"; he read an Orbit box's suggested subject as its words): every text box's suggestion looks
// alike, lighter than what's typed, in italics and never bold, and goes once there's a value. A prompt's (a mention box's `::before`) by
// its computed style; an input's (its `::placeholder`, which no computed style reads) by its pixels: it leans, and it's lighter than the
// same letters typed. In paper and dark. Its looks are kept for a person to look at: Focus, the model picker, New run, a machine's drawer
// and Projects, at 1440×900 on paper and dark.
const { mkdirSync } = require('fs');
const { join } = require('path');
const { test, expect, canvasVisible, focus, screenshotPixels, themed } = require('./fixtures');
const { fit } = require('./hands');

const BOX = 'unwritten';
// Upright, a column of I's has its ink at one x on every row; in italics it leans right going up.
const PROBE = 'IIIIIIIIIIII';

/** A prompt's suggestion and its typed words, as drawn: the `::before` an empty mention box shows, the box's own, and `--hint`. */
const promptLook = (locator) => locator.evaluate((el) => {
  const hint = getComputedStyle(el, '::before'), own = getComputedStyle(el);
  const probe = document.createElement('span');
  probe.style.color = 'var(--hint)';
  document.body.append(probe);
  const token = getComputedStyle(probe).color;
  probe.remove();
  return { content: hint.content, color: hint.color, fontStyle: hint.fontStyle, fontWeight: hint.fontWeight, typed: { color: own.color, fontWeight: own.fontWeight }, token };
});

/** The words drawn in an input: how they lean (the slope of their ink's middle, x over y, row by row: italics < 0) and how far their ink
 *  stands from the box's ground (the 90th percentile, 0–255). Its border and corners are left out. */
async function inkOf(input) {
  const read = screenshotPixels(await input.screenshot({ caret: 'hide' }));
  const lum = ([r, g, b]) => 0.2126 * r + 0.7152 * g + 0.0722 * b;
  const edge = 5, ground = lum(read(edge, edge));
  const rows = [], far = [];
  for (let y = edge; y < read.height - edge; y++) {
    let weight = 0, sum = 0;
    for (let x = edge; x < read.width - edge; x++) {
      const away = Math.abs(lum(read(x, y)) - ground);
      if (away < 24) continue;
      weight += away;
      sum += away * x;
      far.push(away);
    }
    if (weight > 0) rows.push({ y, x: sum / weight, weight });
  }
  if (rows.length < 4) return { slope: null, ink: 0, rows: rows.length };
  const total = rows.reduce((s, r) => s + r.weight, 0);
  const my = rows.reduce((s, r) => s + r.y * r.weight, 0) / total, mx = rows.reduce((s, r) => s + r.x * r.weight, 0) / total;
  const slope = rows.reduce((s, r) => s + r.weight * (r.y - my) * (r.x - mx), 0) / rows.reduce((s, r) => s + r.weight * (r.y - my) ** 2, 0);
  far.sort((a, b) => a - b);
  return { slope: Number(slope.toFixed(3)), ink: Math.round(far[Math.floor(far.length * 0.9)]), rows: rows.length };
}

test('hints', async ({ page, project, api }) => {
  const shots = join(process.env.OUT, 'web', 'hints');
  mkdirSync(shots, { recursive: true });
  const { tools } = await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}&q=qwen`);
  const qwen = tools.find((tool) => tool.package === 'qwen-image-2' && tool.makes === 'image' && tool.fields.some((field) => field.type === 'prompt'));
  expect(qwen, 'the Hub lists Qwen Image 2 with a prompt').toBeTruthy();
  await project.add({ name: BOX, tool: qwen.id, kind: 'image', fields: {}, at: { x: 0, y: 0 } });
  await expect.poll(() => canvasVisible(page, BOX)).toBe(true);
  await page.setViewportSize({ width: 1440, height: 900 });

  /** The page at `rest` (the canvas, or a drawer), in `theme`, from its own address. */
  const open = async (theme, rest = '') => {
    await project.look(theme);
    await page.goto(project.url(rest));
    await expect(page.locator('.stage')).toBeVisible();
    await themed(page, theme);
  };
  /** Its prompt in Focus, empty: the box's own words, opened to be written if they aren't. */
  const prompt = async () => {
    await fit(page);
    const region = await focus(page, BOX);
    const words = region.locator('.mention-text').first();
    if (!(await words.isVisible())) await region.getByRole('button', { name: 'Edit image', exact: true }).click();
    await expect(words).toBeVisible();
    return { region, words };
  };

  for (const theme of ['paper', 'dark']) {
    await test.step(`${theme}: a prompt's suggestion lighter than its words, italic, not bold, gone once typed`, async () => {
      await open(theme);
      const { region, words } = await prompt();
      const look = await promptLook(words);
      test.info().annotations.push({ type: `prompt ${theme}`, description: JSON.stringify(look) });
      expect.soft(look.content, 'its suggestion shown').not.toBe('none');
      expect.soft(look.fontStyle, 'in italics').toBe('italic');
      expect.soft(look.fontWeight, 'not bold, as typed words are').toBe('400');
      expect.soft(look.color, "in the suggestion's colour").toBe(look.token);
      expect.soft(look.color, 'not the colour typed words are').not.toBe(look.typed.color);
      await page.screenshot({ path: join(shots, `focus-prompt-1440x900-${theme}.png`) });
      await words.click();
      await page.keyboard.type('a');
      await expect.poll(async () => (await promptLook(words)).content, { message: 'typed, its suggestion gone' }).toBe('none');
      await page.keyboard.press('Backspace');
      await region.getByRole('button', { name: 'Models and LoRAs', exact: true }).click();
      const picker = page.getByRole('dialog', { name: 'Models and LoRAs', exact: true });
      await picker.locator('button[data-model-slot="model"]').click();
      await expect(picker.getByRole('searchbox', { name: 'Search models for model', exact: true })).toBeVisible();
      await page.screenshot({ path: join(shots, `model-picker-1440x900-${theme}.png`) });
    });

    await test.step(`${theme}: an input's suggestion leans and is lighter than the same letters typed`, async () => {
      await open(theme, 'new');
      const input = page.getByRole('textbox', { name: 'Package or function', exact: true });
      await expect(input).toBeVisible();
      await page.screenshot({ path: join(shots, `new-run-1440x900-${theme}.png`) });
      await input.evaluate((el, probe) => { el.placeholder = probe; el.blur(); }, PROBE);
      const hint = await inkOf(input);
      await input.fill(PROBE);
      await input.evaluate((el) => el.blur());
      const typed = await inkOf(input);
      test.info().annotations.push({ type: `input ${theme}`, description: JSON.stringify({ hint, typed }) });
      expect.soft(typed.slope, `typed, upright: ${JSON.stringify(typed)}`).toBeGreaterThan(-0.06);
      expect.soft(hint.slope, `its suggestion, in italics: ${JSON.stringify(hint)}`).toBeLessThan(-0.12);
      expect.soft(hint.ink, `its suggestion lighter than typed (${typed.ink})`).toBeLessThan(typed.ink - 20);
      await input.fill('');
    });

    await test.step(`${theme}: a machine's drawer and Projects`, async () => {
      for (const [rest, name] of [['machines/local', 'machine'], ['projects', 'projects']]) {
        await open(theme, rest);
        await expect(page.locator('aside.drawer')).toBeVisible();
        await page.screenshot({ path: join(shots, `${name}-1440x900-${theme}.png`) });
      }
    });
  }
});
