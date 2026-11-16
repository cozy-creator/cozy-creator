// fields (cl-973 core cell 10): a box's fields, drawn from what its tool declares. On the every-type tool (project/boxes.py's shot), in Focus:
// each control has its field's role and shows its default; a slider, a menu, a switch, segments, a number and words each set go to the API
// at once, a reset unsets its field and shows the default again, and `cozy render` sends them. A When field (project/when.py) dims as its
// controller goes off, saying why, keeps its value, and `cozy run` ignores it. An image slot refuses a video, saying why; the corner pulled
// to 1237:689 lands on 16:9. On H3, Models & LoRAs: its base model listed once, a LoRA added with its trigger appended once and drawn, its
// strength kept as the model is replaced in place. Qwen Image 2's VAE slot lists the seeded VAE. No control lacks a declared field.
// A box says its shape as film and media say it (cl-983): a take the shape its render asked, whatever size its model rounded to
// (project/rounded.py sizes as H3: 16:9 at 1344x768, 1.37:1 at 1024x768, which is 4:3 in pixels), still once its box's shape is cleared;
// a file its pixels' nearest standard ratio within about 2%, else the simplest whole one; a corner pulled near 4:3 sets 4:3, and far from
// any sets the ratio as pulled and says it. Dark says the same.
const { test, expect, canvasIdle, focus, middle, fixtureTool, MP4, png, canvasVisible, canvasLabel, hoverBox } = require('./fixtures');
const { carry } = require('./geometry');
const { fit, leave } = require('./hands');

const EVERY = fixtureTool('boxes.py/shot');
const WHEN = fixtureTool('when.py/image');
const ROUNDED = fixtureTool('rounded.py/shot');
const DRAW = fixtureTool('draw.py/draw');
const RUN_MS = 120_000;
// web/app/src/canvas/ratios.ts's.
const STANDARD = ['16:9', '9:16', '1:1', '4:3', '3:4', '4:5', '5:4', '3:2', '2:3', '21:9', '9:21', '2:1', '1:2', '2.39:1', '1.85:1', '1.90:1', '1.43:1', '1.37:1', '1.66:1'];
const ratioOf = (shape) => shape.split(':').map(Number).reduce((w, h) => w / h);

/** A point of `name`'s picture on the canvas a hand can press to carry it: not under what floats over it, nor on a button. */
const reach = (page, name) => page.evaluate((name) => {
  const root = document.querySelector('.canvas-root'), scene = root?.canvasInspection, at = scene?.card(name)?.body;
  if (!at) return null;
  for (let y = at.y + 4; y < Math.min(at.y + at.height, innerHeight); y += 8) for (let x = at.x + 4; x < Math.min(at.x + at.width, innerWidth); x += 8) {
    const el = document.elementFromPoint(x, y);
    if (el && root.contains(el) && scene.hitTest({ x, y })?.name === name && !el.closest('button')) return { x, y };
  }
  return null;
}, name);

/** Every control of the open box that isn't one of its tool's fields: each `[data-field]` by its first part, each control outside a group's
 *  rows by its label (a slot's typed name and a clip's in and out by their field's). [] when each is declared. */
const undeclared = (region, tool) => region.locator('.focus-card').evaluate((card, { names, labels }) => {
  const odd = [];
  const scope = [...card.querySelectorAll('.edit-face, .in-frame')];
  for (const el of scope.flatMap((part) => [...part.querySelectorAll('[data-field]')])) {
    if (!names.includes(el.dataset.field.split('.')[0])) odd.push(`data-field ${el.dataset.field}`);
  }
  const controls = scope.flatMap((part) => [...part.querySelectorAll('input, select, [role=radiogroup], [role=switch], [role=slider], [role=textbox]')]);
  for (const el of controls) {
    if (el.closest('.beats-edit, .group-block')) continue;
    const label = (el.getAttribute('aria-label') ?? '').split(': ')[0];
    if (!labels.includes(label)) odd.push(`${el.tagName.toLowerCase()} "${el.getAttribute('aria-label')}"`);
  }
  return [...new Set(odd)];
}, { names: tool.fields.map((field) => field.field), labels: tool.fields.map((field) => field.label) });

test('fields', async ({ page, project, api, cli }) => {
  const listed = async () => (await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}`)).tools;
  const qwen = (await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}&q=qwen`)).tools.find((tool) => tool.package === 'qwen-image-2' && tool.makes === 'image' && (tool.models?.length || tool.model));
  expect(qwen, 'the Hub lists Qwen Image 2 with its models').toBeTruthy();
  await project.import(MP4, { name: 'walk', at: { x: 0, y: 0 } });
  await project.import(png(40), { name: 'coast', at: { x: 0, y: 260 } });
  await project.add({ name: 'every', tool: EVERY, kind: 'video', fields: { plate: '<walk>', aspect_ratio: '1:1' }, at: { x: 360, y: 0 } });
  await project.add({ name: 'when', tool: WHEN, kind: 'image', fields: { instructions: 'a lighthouse at dusk', creativity: 'wild' }, at: { x: 720, y: 0 } });
  await project.add({ name: 'scene', tool: 'minimax-h3/generate', kind: 'video', fields: { summary: 'A harbour at dusk.', beats: [{ description: 'Boats come in.', duration: 5 }] }, at: { x: 360, y: 460 } });
  await project.add({ name: 'still', tool: qwen.id, kind: 'image', fields: { [qwen.fields.find((field) => field.type === 'prompt').field]: 'A lighthouse.' }, at: { x: 720, y: 460 } });
  let every, when;
  await expect.poll(async () => {
    const tools = await listed();
    [every, when] = [EVERY, WHEN].map((id) => tools.find((tool) => tool.local && tool.id === id));
    return Boolean(every && when);
  }, { message: 'the project lists its local tools' }).toBe(true);
  for (const name of ['walk', 'coast', 'every', 'when', 'scene', 'still']) await expect.poll(() => canvasVisible(page, name)).toBe(true);
  await fit(page);
  const region = page.getByRole('region', { name: 'Focused asset' });
  const fieldsOf = async (name) => (await project.asset(name))?.fields ?? {};

  const steps = region.getByRole('slider', { name: 'Steps', exact: true });
  const resolution = region.getByRole('combobox', { name: 'Resolution', exact: true });
  const edit = region.getByRole('combobox', { name: 'Edit', exact: true });
  const turbo = region.getByRole('switch', { name: 'Turbo', exact: true });
  const quality = region.getByRole('radiogroup', { name: 'Quality', exact: true });
  const megapixels = region.getByRole('slider', { name: 'Megapixels', exact: true });
  const negative = region.getByRole('textbox', { name: 'Negative', exact: true });
  const chosen = (select) => select.evaluate((el) => el.selectedOptions[0]?.textContent ?? null);

  await test.step('in Focus on the every-type tool: each control its role and default; each change in the API at once; a reset', async () => {
    await focus(page, 'every');
    await expect.soft(steps, 'a number: a slider at its default').toHaveValue('8');
    expect.soft(await chosen(resolution), 'five options: a menu at its default').toBe('720p');
    await expect.soft(turbo, 'a switch, on by default').toBeChecked();
    await expect.soft(quality.getByRole('radio', { name: 'standard', exact: true }), 'three options: segments at the default').toHaveAttribute('aria-checked', 'true');
    await expect.soft(region.locator('label.slide.auto', { has: page.getByRole('slider', { name: 'Megapixels', exact: true }) }), 'a number unset: Auto').toHaveCount(1);
    await expect.soft(negative, 'words, empty').toHaveText('');
    expect.soft(await undeclared(region, every), 'controls of no declared field').toEqual([]);

    await steps.focus();
    await page.keyboard.press('ArrowRight');
    await expect.poll(async () => (await fieldsOf('every')).steps, { message: 'the slider, in the API' }).toBe(9);
    await resolution.selectOption({ label: '1080p' });
    await expect.poll(async () => (await fieldsOf('every')).resolution, { message: 'the menu, in the API' }).toBe('1080p');
    // Options that say what they are (cl-1066, Paul 11-16: a list drawn inside the box was "clipped by the bounding box it's inside of"):
    // the browser's own menu, which nothing clips, and what the chosen one does on a line under it.
    await expect.soft(edit, 'a described choice is a menu').toHaveJSProperty('tagName', 'SELECT');
    expect.soft(await chosen(edit), 'at its default').toBe('Character swap');
    await edit.selectOption({ label: 'Head swap' });
    await expect.poll(async () => (await fieldsOf('every')).edit, { message: 'the described menu, in the API' }).toBe('head');
    await expect.soft(region.locator('.described-says'), 'what the chosen one does, under it').toHaveText('Only the head');
    await expect.soft(page.locator('.dropdown-list'), 'no list drawn inside the box').toHaveCount(0);
    await turbo.click();
    await expect.poll(async () => (await fieldsOf('every')).turbo, { message: 'the switch, in the API' }).toBe(false);
    await quality.getByRole('radio', { name: 'full', exact: true }).click();
    await expect.poll(async () => (await fieldsOf('every')).quality, { message: 'the segments, in the API' }).toBe('full');
    await megapixels.focus();
    await page.keyboard.press('ArrowRight');
    await expect.poll(async () => typeof (await fieldsOf('every')).megapixels, { message: 'the number, in the API' }).toBe('number');
    await negative.click();
    await page.keyboard.type('blurry');
    await expect.poll(async () => (await fieldsOf('every')).negative, { message: 'the words, in the API' }).toBe('blurry');

    await region.getByRole('button', { name: 'Reset Quality', exact: true }).click();
    await expect.poll(async () => 'quality' in (await fieldsOf('every')), { message: 'a reset unsets its field' }).toBe(false);
    await expect.soft(quality.getByRole('radio', { name: 'standard', exact: true }), 'and shows the default again').toHaveAttribute('aria-checked', 'true');
  });

  await test.step("a When field dims as its controller goes off, saying why, keeps its value, and `cozy run` ignores it", async () => {
    await leave(page);
    await focus(page, 'when');
    const upsampling = region.getByRole('switch', { name: 'Prompt upsampling', exact: true });
    const creativity = region.getByRole('radiogroup', { name: 'Creativity', exact: true });
    const dulled = region.locator('fieldset.when-off', { has: page.getByRole('radiogroup', { name: 'Creativity', exact: true }) });
    await expect(upsampling).toBeChecked();
    await expect(dulled).toHaveCount(0);
    await expect.soft(creativity.getByRole('radio', { name: 'wild', exact: true })).toHaveAttribute('aria-checked', 'true');
    await upsampling.click();
    await expect(dulled, 'dimmed at once').toHaveCount(1);
    await expect.soft(dulled, 'why, on hover').toHaveAttribute('title', 'Only with Prompt upsampling on');
    await expect.soft(creativity.getByRole('radio', { name: 'balanced', exact: true }), 'nothing in it can be pressed').toBeDisabled();
    expect.soft(Number(await dulled.evaluate((el) => getComputedStyle(el).opacity)), 'dimmed').toBeLessThan(0.6);
    await expect.soft(creativity.getByRole('radio', { name: 'wild', exact: true }), 'its value kept').toHaveAttribute('aria-checked', 'true');
    await expect.poll(() => fieldsOf('when'), { message: 'the controller off, the value kept' }).toMatchObject({ prompt_upsampling: false, creativity: 'wild' });
    await upsampling.click();
    await expect(dulled, 'back with it on').toHaveCount(0);
    await expect.soft(creativity.getByRole('radio', { name: 'wild', exact: true })).toHaveAttribute('aria-checked', 'true');
    await expect.poll(async () => (await fieldsOf('when')).prompt_upsampling, { message: 'the controller on' }).toBe(true);

    const runs = async () => ((await api.get(project.at('/runs?limit=100'))).runs ?? []).filter((one) => one.target?.endsWith('/image') && one.status === 'completed').map((one) => one.number);
    const before = await runs();
    const ran = await cli('--json', 'run', WHEN, 'instructions=a lighthouse', 'prompt_upsampling=false', 'creativity=wild', '--on', 'local');
    expect.soft(ran.code, 'cozy run').toBe(0);
    expect.soft(ran.stderr, 'cozy run says it ignores it').toContain('creativity applies only with prompt_upsampling on, so Cozy ignored wild and used the default.');
    let made;
    await expect.poll(async () => (made = (await runs()).find((number) => !before.includes(number))), { message: 'its run', timeout: RUN_MS }).toBeTruthy();
    expect.soft((await api.get(project.at(`/runs/${made}`))).result?.given, 'the function got its default').toBe('creativity=balanced temporal_overlap=0');
  });

  await test.step('an image slot refuses a video, saying why; the corner pulled to 1237:689 lands on 16:9', async () => {
    await leave(page);
    await focus(page, 'every');
    const slot = region.locator('.slot-control[data-field="first_frame"]');
    await slot.scrollIntoViewIfNeeded();
    // The canvas right of what Focus shows, where walk is carried from.
    const strip = await page.evaluate(() => {
      const root = document.querySelector('.canvas-root'), at = root.canvasInspection.camera(), pane = root.getBoundingClientRect();
      const right = Math.max(...[...document.querySelectorAll('.focus-viewport .focus-card > .here-under, .focus-viewport .focus-card .media')].map((el) => el.getBoundingClientRect().right));
      return { x: Math.round((right + 24 - pane.x - at.x) / at.zoom), y: Math.round((innerHeight / 2 - pane.y - at.y) / at.zoom) };
    });
    await project.api.patch(project.at('/canvas'), { cards: { walk: strip } });
    const stood = async () => {
      const row = (await project.api.get(project.at('/canvas'))).cards.walk;
      return { x: row?.x, y: row?.y };
    };
    await expect.poll(stood).toEqual(strip);
    let from;
    await expect.poll(async () => (from = await reach(page, 'walk')), { message: 'walk in reach beside Focus' }).toBeTruthy();
    const said = await carry(page, from, await middle(slot), async () => ({ refuses: /\brefuses\b/.test(await slot.getAttribute('class')), why: await slot.locator('.refusal').textContent() }));
    expect.soft(said, 'said as it is carried over').toMatchObject({ refuses: true, why: expect.stringContaining('takes an image') });
    await expect.soft(page.locator('.notes .note .line', { hasText: 'takes an image' }), 'and said once let go').toHaveCount(1);
    expect.soft(await fieldsOf('every'), 'refused').not.toHaveProperty('first_frame');
    expect.soft(await stood(), 'refused, walk stays where it was: a drop is no move').toEqual(strip);

    const corner = region.locator('.corner').first();
    const media = await region.locator('.focus-image-transform > .media').first().boundingBox();
    const grip = await corner.boundingBox();
    const start = { x: grip.x + grip.width / 2, y: grip.y + grip.height / 2 };
    const width = media.width + 160;
    const badge = region.locator('[data-output-aspect]').first();
    await page.mouse.move(start.x, start.y);
    await page.mouse.down();
    await page.mouse.move(start.x + 160, start.y + ((width * 689) / 1237 - media.height), { steps: 20 });
    await expect.soft(badge, 'its size, as it is pulled').toHaveText(/^16:9/);
    await page.mouse.up();
    await expect.poll(async () => (await fieldsOf('every')).aspect_ratio, { message: 'its shape saved' }).toBe('16:9');
    await expect.soft(badge, 'its size').toHaveText(/^16:9/);
  });

  await test.step("Qwen Image 2's VAE slot lists the seeded VAE", async () => {
    await leave(page);
    await fit(page);
    await focus(page, 'still');
    const viewer = region.locator('.focus-viewer').first();
    const before = await viewer.boundingBox();
    await region.getByRole('button', { name: 'Models and LoRAs', exact: true }).click();
    const picker = page.getByRole('dialog', { name: 'Models and LoRAs', exact: true });
    await expect(picker).toBeVisible();
    // Paul, 11-15: in Focus the picker opens in front of the picture; the picture keeps its size.
    const after = await viewer.boundingBox();
    expect.soft(after, 'the picture keeps its size and place with the picker open').toEqual(before);
    const box = await picker.boundingBox();
    expect.soft(box && after && box.y < after.y + after.height && box.y + box.height > after.y, 'the picker lies over the picture').toBe(true);
    const front = await picker.evaluate((element) => {
      const rect = element.getBoundingClientRect();
      const top = document.elementFromPoint(rect.left + rect.width / 2, rect.top + 20);
      return Boolean(top && element.contains(top));
    });
    expect.soft(front, 'the picker is in front of the picture').toBe(true);
    // Every row takes its own click: nothing of the frame (its zoom or shape controls) lies over any of them.
    const covered = await picker.locator('button[data-model-slot]').evaluateAll((rows) => rows.flatMap((row) => {
      row.scrollIntoView({ block: 'nearest' });
      const r = row.getBoundingClientRect();
      const top = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
      return top && row.contains(top) ? [] : [row.getAttribute('data-model-slot')];
    }));
    expect.soft(covered, 'no slot row is covered by the frame').toEqual([]);
    // Every slot the tool declares is a row (Qwen shows one prompt helper: the expander, as nothing here edits a picture), and each opens.
    const rows = picker.locator('button[data-model-slot]');
    await expect(rows, 'one row per slot').toHaveCount(5);
    expect.soft(await rows.evaluateAll((all) => all.map((row) => row.getAttribute('data-model-slot')).sort()), 'its slots').toEqual(['expander', 'model', 'text_encoder', 'turbo_lora', 'vae']);
    for (const slot of ['expander', 'model', 'text_encoder', 'turbo_lora']) {
      await picker.locator(`button[data-model-slot="${slot}"]`).click();
      await expect(picker.locator(`section.model-picker-slot[data-model-slot="${slot}"]`), `${slot} opens`).toBeVisible();
      await picker.getByRole('button', { name: 'Back to model slots', exact: true }).click();
    }
    await picker.locator('button[data-model-slot]', { hasText: /^VAE$/ }).click();
    await expect(picker.locator('.model-picker-choices [role="radio"]').filter({ hasText: 'madebyollin / Texture Fix VAE' })).toHaveCount(1);
    await page.keyboard.press('Escape');
    await expect(picker, 'Escape closes it').toHaveCount(0);
    await region.getByRole('button', { name: 'Models and LoRAs', exact: true }).click();
    await expect(picker).toBeVisible();
    const open = await picker.boundingBox();
    // A press that begins inside and lets go outside dismisses nothing; one that begins outside does.
    await page.mouse.move(open.x + 40, open.y + 30);
    await page.mouse.down();
    await page.mouse.move(open.x - 40, open.y + open.height + 60, { steps: 4 });
    await page.mouse.up();
    await expect.soft(picker, 'a press from inside let go outside keeps it').toBeVisible();
    await page.mouse.click(after.x + 8, after.y + after.height - 8);
    await expect(picker, 'a press outside closes it').toHaveCount(0);
    await expect(region, 'and only it: Focus stays').toBeVisible();
    await leave(page);
  });

  await test.step('`cozy render` sends the saved fields', async () => {
    const saved = await fieldsOf('every');
    const rendered = await cli('--json', 'render', 'every', '--on', 'local');
    expect.soft(rendered.code, `cozy render: ${rendered.stderr.slice(-400)}`).toBe(0);
    await expect.poll(async () => (await project.asset('every'))?.prefixes?.length ?? 0, { message: 'its take', timeout: RUN_MS }).toBe(1);
    const run = (await project.asset('every')).prefixes[0].run;
    const input = (await cli.json('--full', 'run', 'watch', String(run), '--once')).request?.input ?? {};
    expect.soft(input, "the run's request").toMatchObject({ steps: 9, resolution: '1080p', turbo: false, megapixels: saved.megapixels, negative: 'blurry', aspect_ratio: '16:9' });
    expect.soft(input.quality ?? 'standard', 'the field reset goes as its default').toBe('standard');
  });

  await test.step('a box says its shape as film and media say it: a take what it asked, a file its pixels\' nearest, a corner what it set', async () => {
    const imports = [['wide', 1344, 768, '16:9'], ['tall', 768, 1344, '9:16'], ['square', 1024, 1024, '1:1'], ['odd', 1000, 333, '3:1']];
    for (const [i, [name, width, height]] of imports.entries()) await project.import(png(60 + 40 * i, width, height), { name, at: { x: 1300 + 420 * i, y: 0 } });
    await project.add({ name: 'widescreen', tool: ROUNDED, kind: 'video', fields: { instructions: 'a harbour at dawn', aspect_ratio: '16:9' }, at: { x: 1300, y: 700 } });
    await project.add({ name: 'academy', tool: ROUNDED, kind: 'video', fields: { instructions: 'a harbour at dusk', aspect_ratio: '1.37:1' }, at: { x: 1760, y: 700 } });
    await project.add({ name: 'pulled', tool: DRAW, kind: 'image', fields: { instructions: 'a lighthouse', aspect_ratio: '1:1' }, at: { x: 2220, y: 700 } });
    const rendered = await cli('--json', 'render', 'widescreen', 'academy', '--on', 'local');
    expect.soft(rendered.code, `cozy render: ${rendered.stderr.slice(-400)}`).toBe(0);
    const take = (row) => row?.prefixes?.at(-1);
    const made = {};
    for (const name of ['widescreen', 'academy']) {
      await expect.poll(async () => {
        const row = await project.asset(name);
        return Boolean((take(row)?.main?.width && !take(row).main.absent) || row?.failure);
      }, { message: `${name}'s take`, timeout: RUN_MS }).toBe(true);
      const row = await project.asset(name);
      made[name] = { aspect_ratio: take(row)?.aspect_ratio, width: take(row)?.main?.width, height: take(row)?.main?.height };
      if (row.failure) made[name].failure = row.failure;
    }
    expect.soft(made, 'each take its rounded size, and the shape it asked').toEqual({ widescreen: { aspect_ratio: '16:9', width: 1344, height: 768 }, academy: { aspect_ratio: '1.37:1', width: 1024, height: 768 } });
    await fit(page);
    /** What a box at rest says under it of its shape. */
    const says = (name) => canvasLabel(page, name, 'shape');
    for (const [name, , , want] of imports) await expect.configure({ soft: true }).poll(() => says(name), { message: `${name}, a file, says its pixels' ${want}` }).toBe(want);
    await expect.configure({ soft: true }).poll(() => says('widescreen'), { message: 'a 16:9 render of 1344x768 says 16:9' }).toBe('16:9');
    await expect.configure({ soft: true }).poll(() => says('academy'), { message: 'a 1.37:1 render of 1024x768 says 1.37:1' }).toBe('1.37:1');
    // Its box's shape cleared, its take still says what it asked for, never its pixels' 4:3.
    await api.patch(project.at('/assets/academy'), { fields: { aspect_ratio: null } });
    await expect.poll(async () => (await fieldsOf('academy')).aspect_ratio ?? null, { message: 'academy cleared' }).toBe(null);
    await expect.configure({ soft: true }).poll(() => says('academy'), { message: 'its take says what it asked, once its box is cleared' }).toBe('1.37:1');

    /** `pulled`'s corner pulled until its picture is `ratio` wide to 1 tall: what it said as it was held, and what it saved. */
    const pull = async (ratio) => {
      const node = await hoverBox(page, 'pulled');
      const corner = await node.locator('.corner').first().boundingBox();
      const media = await node.locator('.media').first().boundingBox();
      const before = (await fieldsOf('pulled')).aspect_ratio;
      const from = { x: corner.x + corner.width / 2, y: corner.y + corner.height / 2 };
      const by = 80;
      await page.mouse.move(from.x, from.y);
      await page.mouse.down();
      await page.mouse.move(from.x + by, from.y + ((media.width + by) / ratio - media.height), { steps: 20 });
      const said = page.locator('.ratio-live');
      await expect.soft(said, `its shape said as it's pulled to ${ratio}`).toHaveCount(1);
      const live = (await said.count()) ? (await said.first().innerText()).trim() : null;
      await page.mouse.up();
      let saved;
      await expect.poll(async () => (saved = (await fieldsOf('pulled')).aspect_ratio) !== before, { message: `its shape saved, pulled to ${ratio}` }).toBe(true);
      await canvasIdle(page);
      return { live, saved };
    };
    const near = await pull(1.32);
    expect.soft(near, 'pulled near 4:3, it sets 4:3').toEqual({ live: '4:3', saved: '4:3' });
    await expect.configure({ soft: true }).poll(() => says('pulled'), { message: 'and says it' }).toBe('4:3');
    const free = await pull(1.6);
    const off = (a, b) => Math.abs(Math.log(ratioOf(a) / (typeof b === 'number' ? b : ratioOf(b))));
    const as = free.saved;
    expect.soft(typeof as === 'string' && !STANDARD.includes(as) && off(as, 1.6) < 0.03 && !STANDARD.some((shape) => off(as, shape) <= 0.02), `pulled far from any, it stays as pulled: ${JSON.stringify(free)}`).toBe(true);
    expect.soft(free.live, 'said as pulled').toBe(as);
    await expect.configure({ soft: true }).poll(() => says('pulled'), { message: 'and says it' }).toBe(String(as));

    await page.evaluate(() => localStorage.setItem('cozy.theme', 'dark'));
    await page.reload();
    await expect.poll(() => canvasVisible(page, 'pulled')).toBe(true);
    await fit(page);
    const wanted = { wide: '16:9', tall: '9:16', square: '1:1', odd: '3:1', widescreen: '16:9', academy: '1.37:1', pulled: String(as) };
    for (const [name, want] of Object.entries(wanted)) await expect.configure({ soft: true }).poll(() => says(name), { message: `dark: ${name} says ${want}` }).toBe(want);
  });

  // Last: a Hub whose LoRA cards hold no triggers fails it, with every step before it already reported.
  await test.step('H3: Models & LoRAs: the base model once, a LoRA, its strength, the model replaced in place', async () => {
    await fit(page);
    await focus(page, 'scene');
    await region.getByRole('button', { name: 'Models and LoRAs', exact: true }).click();
    const picker = page.getByRole('dialog', { name: 'Models and LoRAs', exact: true });
    await expect(picker).toBeVisible();
    await expect.soft(picker.locator('button[data-model-slot]', { hasText: /^Base Model$/ }), 'its base model listed once').toHaveCount(1);
    await picker.locator('button[data-model-slot]', { hasText: /^Base Model$/ }).click();
    const radios = picker.locator('.model-picker-choices [role="radio"]');
    await radios.filter({ hasText: 'sexgod1979 / Pinkcherry' }).click();
    await expect.poll(async () => (await fieldsOf('scene')).models?.base_model?.ref, { message: 'a base model picked' }).toBe('sexgod1979/pinkcherry');
    // 360 Orbit: its card's one trigger is a whole prompt, commas and all (cl-971).
    const triggers = (await api.get('/v1/models/pablodawson/360-orbit')).triggers ?? [];
    expect(triggers, "the trigger the Hub's seed gave its card").toHaveLength(1);
    const [trigger] = triggers;
    await picker.getByRole('button', { name: 'LoRAs', exact: true }).click();
    await picker.locator('.model-picker-lora-option', { hasText: /360 Orbit/i }).click();
    let lora;
    await expect.poll(async () => (lora = (await fieldsOf('scene')).models?.base_model?.lora?.[0]?.ref), { message: 'a LoRA added' }).toMatch(/^pablodawson\/360-orbit/);
    await expect.poll(async () => {
      const summary = String((await fieldsOf('scene')).summary ?? '');
      return summary.startsWith('A harbour at dusk.') && summary.split(trigger).length - 1;
    }, { message: 'its trigger after the prompt, once' }).toBe(1);
    const strength = picker.locator('.model-picker-adapters input[type="number"]').first();
    await strength.fill('1.5');
    await strength.press('Enter');
    await expect.poll(async () => (await fieldsOf('scene')).models?.base_model?.lora?.[0]?.strength, { message: 'its strength' }).toBe(1.5);
    await picker.getByRole('button', { name: 'Models', exact: true }).click();
    await radios.filter({ hasText: 'tenstrip / Eros Max' }).click();
    await expect.poll(async () => (await fieldsOf('scene')).models, { message: 'the model replaced in place, the LoRA kept at its strength' })
      .toEqual({ base_model: { ref: 'tenstrip/eros-max', lora: [{ ref: lora, strength: 1.5 }] } });
    await expect.soft(picker.locator('.model-picker-choices [aria-checked="true"]'), 'one model in the slot').toHaveText([/Eros Max/]);
    await picker.getByRole('button', { name: 'Close model picker', exact: true }).click();
    await expect(picker).toHaveCount(0);
    const lit = () => page.evaluate(() => {
      const held = [...document.querySelectorAll('.focus-viewport .card [data-field="summary"]')];
      const marks = globalThis.CSS?.highlights?.get('lora-trigger');
      return marks ? [...marks].filter((range) => held.some((el) => el.contains(range.startContainer))).map((range) => range.toString().toLowerCase()) : [];
    });
    await expect.poll(lit, { message: 'its trigger drawn in the prompt' }).toEqual([trigger.toLowerCase()]);
  });
});
