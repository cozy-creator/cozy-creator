// fields (cl-973 core cell 10): a box's fields, drawn from what its tool declares. On the every-type tool (project/boxes.py's shot), in Focus:
// each control has its field's role and shows its default; a slider, a menu, a switch, segments, a number and words each set go to the API
// at once, a reset unsets its field and shows the default again, and `cozy render` sends them. A When field (project/when.py) dims as its
// controller goes off, saying why, keeps its value, and `cozy run` ignores it. An image slot refuses a video, saying why; the corner pulled
// to 1237:689 lands on 16:9. On H3, Models & LoRAs: its base model listed once, a LoRA added with its trigger appended once and drawn, its
// strength kept as the model is replaced in place. Qwen Image 2's VAE slot lists the seeded VAE. No control lacks a declared field.
const { test, expect, box, focus, middle, fixtureTool, MP4, png } = require('./fixtures');
const { carry, fit, leave } = require('./hands');

const EVERY = fixtureTool('boxes.py/shot');
const WHEN = fixtureTool('when.py/image');
const RUN_MS = 120_000;

/** A point of `name`'s picture on the canvas a hand can press to carry it: not under what floats over it, nor on a button. */
const reach = (page, name) => page.evaluate((name) => {
  const node = document.querySelector(`.react-flow__node:has(.card[data-asset="${name}"])`);
  const at = (node?.querySelector('.card > .media') ?? node)?.getBoundingClientRect();
  if (!at) return null;
  for (let y = at.top + 4; y < Math.min(at.bottom, innerHeight); y += 8) for (let x = at.left + 4; x < Math.min(at.right, innerWidth); x += 8) {
    const el = document.elementFromPoint(x, y);
    if (el?.closest('.react-flow__node')?.querySelector('.card')?.dataset.asset === name && !el.closest('button')) return { x, y };
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
  const controls = scope.flatMap((part) => [...part.querySelectorAll('input, select, [role=radiogroup], [role=switch], [role=slider], [role=textbox], button.dropdown')]);
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
  for (const name of ['walk', 'coast', 'every', 'when', 'scene', 'still']) await expect(box(page, name)).toBeVisible();
  await fit(page);
  const region = page.getByRole('region', { name: 'Focused asset' });
  const fieldsOf = async (name) => (await project.asset(name))?.fields ?? {};

  const steps = region.getByRole('slider', { name: 'Steps', exact: true });
  const resolution = region.getByRole('combobox', { name: 'Resolution', exact: true });
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
    expect.soft(ran.stderr, 'cozy run says it ignores it').toContain('creativity: "wild" is ignored: it applies only with prompt_upsampling on');
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
      const at = new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform);
      const right = Math.max(...[...document.querySelectorAll('.focus-viewport .focus-card > .here-under, .focus-viewport .focus-card .media')].map((el) => el.getBoundingClientRect().right));
      return { x: Math.round((right + 24 - at.e) / at.a), y: Math.round((innerHeight / 2 - at.f) / at.a) };
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
    expect.soft(said, 'said as it is carried over').toMatchObject({ refuses: true, why: expect.stringContaining('takes a picture') });
    await expect.soft(page.locator('.notes .note .line', { hasText: 'takes a picture' }), 'and said once let go').toHaveCount(1);
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

  await test.step('H3: Models & LoRAs: the base model once, a LoRA, its strength, the model replaced in place', async () => {
    await leave(page);
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
    expect(triggers, "the seeded LoRA's triggers").toHaveLength(1);
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

  await test.step("Qwen Image 2's VAE slot lists the seeded VAE", async () => {
    await leave(page);
    await fit(page);
    await focus(page, 'still');
    await region.getByRole('button', { name: 'Models and LoRAs', exact: true }).click();
    const picker = page.getByRole('dialog', { name: 'Models and LoRAs', exact: true });
    await picker.locator('button[data-model-slot]', { hasText: /^VAE$/ }).click();
    await expect(picker.locator('.model-picker-choices [role="radio"]').filter({ hasText: 'madebyollin / Texture Fix VAE' })).toHaveCount(1);
    await picker.getByRole('button', { name: 'Close model picker', exact: true }).click();
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
});
