// rule-loras (cl-1281; Paul, 11-18: "when you add in lora-A in the webui, it modifies your prompt to append the trigger word for that lora …
// which you as an author (the end user) can remove, but it adds it there"): a LoRA a tool chose by its own rule is drawn as one put on the
// box is. On project/edits.py, whose two slots' defaults are LoRAs its rule over `edit` picks between: a box made from the palette gets its
// default edit's trigger; one the API made gets none as the page loads it; choosing an edit puts its trigger after the prompt, drawn, and
// another edit replaces it; deleted, it stays deleted through a reload and an edit of the words. The chosen LoRA has its row beneath the
// box, its release's strength said in words, no control. A LoRA added in Models & LoRAs appends its own, once. Of a LoRA with several
// triggers only the first goes in (cl-1287; Paul, 11-19: "you can have loras with many trigger words, sure. Just include one though when you
// add it to the user's prompt in the webui, but tensorhub can record many trigger words"): the rest are chips on its row, one click puts one
// in. On the Hub's H3 Edit, when its edits are slots (packages #1017), its VFX edit's trigger comes and goes with its Edit. Shots at
// 1440×900 and 1920×1080, paper and dark.
const { test, expect, themed, canvasIdle, canvasVisible, focus, fixtureTool, openPoint } = require('./fixtures');
const { carry } = require('./geometry');
const { fit, leave, toolRow } = require('./hands');
const { mkdirSync } = require('fs');
const { join } = require('path');

const EDITS = fixtureTool('edits.py/edit');
const VFX = 'alissonerdx/minimax-h3-vfx-edit';
const SWAP = 'nrdx/bfs-body-swap-selective-lora';
const WORDS = 'A harbour at dusk.';

test('rule-loras', async ({ page, project, api, guards }) => {
  test.setTimeout(480_000);
  // H3 Edit's other edits' LoRAs aren't on this Hub: their cards are 404s, which the browser logs.
  guards.allow(/status of 404\b.*\/v1\/models\//);
  const softly = expect.configure({ soft: true });
  const shots = join(process.env.OUT, 'web', 'rule-loras');
  mkdirSync(shots, { recursive: true });
  const asset = (name) => project.asset(name);
  const summaryOf = async (name) => String((await asset(name))?.fields?.summary ?? '');
  const chosenOf = async (name) => (await asset(name))?.chosen?.map((held) => held.model.split('@')[0]) ?? null;
  const cardTriggers = async (ref) => (await api.get(`/v1/models/${ref}`)).triggers ?? [];
  const [vfx, swap] = [await cardTriggers(VFX), await cardTriggers(SWAP)];
  expect(vfx, "VFX edit's trigger, as the Hub's seed gave its card").toEqual(['vfx_edit']);
  expect(swap, "body swap's").toEqual(['bfs_swap', 'head swap']);
  const region = page.getByRole('region', { name: 'Focused asset' });
  const editOf = region.getByRole('combobox', { name: 'Edit', exact: true });
  /** The triggers drawn in the open box's prompt, as its highlight paints them. */
  const lit = () => page.evaluate(() => {
    const held = [...document.querySelectorAll('[data-focused-asset] [data-field="summary"]')];
    const marks = globalThis.CSS?.highlights?.get('lora-trigger');
    return marks ? [...marks].filter((range) => held.some((el) => el.contains(range.startContainer))).map((range) => range.toString()) : [];
  });
  /** Box `name` open in Focus, from wherever the page is. */
  const open = async (name) => {
    await canvasIdle(page);
    if ((await region.count()) && (await region.getAttribute('data-focused-asset')) === name) return;
    if (await region.count()) await leave(page);
    await fit(page);
    await focus(page, name);
  };
  /** A reload from the canvas: one from Focus opens it again as the page settles, under a zoom menu being asked for (44427960225). */
  const reload = async () => {
    if (await region.count()) await leave(page);
    await page.reload();
    await expect(page.locator('.stage')).toBeVisible();
    await canvasIdle(page);
  };
  /** Box `name` open at 1440×900 on paper, and with `all` at 1920×1080 and in dark too, each a reload: the page and its card kept for a
   *  person to look at. */
  const look = async (what, name, all = false) => {
    const size = page.viewportSize();
    for (const theme of all ? ['paper', 'dark'] : ['paper']) for (const [width, height] of all ? [[1440, 900], [1920, 1080]] : [[1440, 900]]) {
      await project.look(theme);
      await page.setViewportSize({ width, height });
      await reload();
      await themed(page, theme);
      await open(name);
      await canvasIdle(page);
      await page.screenshot({ path: join(shots, `${what}-${width}x${height}-${theme}.png`) });
      await region.locator('.focus-card').first().screenshot({ path: join(shots, `${what}-card-${width}x${height}-${theme}.png`) });
    }
    await project.look(null);
    await page.setViewportSize(size);
    await reload();
    await open(name);
  };

  await project.add({ name: 'edited', tool: EDITS, kind: 'image', fields: { summary: WORDS }, at: { x: 0, y: 0 } });
  await project.add({ name: 'scene', tool: 'minimax-h3/generate', kind: 'video', fields: { summary: WORDS, beats: [{ description: 'Boats come in.', duration: 5 }] }, at: { x: 0, y: 520 } });
  await expect.poll(async () => (await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}`)).tools.some((one) => one.local && one.id === EDITS),
    { message: 'the project lists edits.py', timeout: 180_000 }).toBe(true);
  await expect.poll(() => chosenOf('edited'), { message: "edited's rule chose VFX edit's LoRA, as Calcifer reads its box", timeout: 120_000 }).toEqual([VFX]);
  for (const name of ['edited', 'scene']) await expect.poll(() => canvasVisible(page, name)).toBe(true);

  await test.step('a box made from the palette: its default edit brings its LoRA, and its trigger', async () => {
    const row = await toolRow(page, 'Image', EDITS);
    await carry(page, row, await openPoint(page));
    let made;
    await expect.poll(async () => (made = (await project.assets()).find((one) => one.tool === EDITS && one.name !== 'edited')?.name) ?? null, { message: 'a box of edits.py, dropped' }).not.toBeNull();
    await softly.poll(() => summaryOf(made), { message: "the new box's prompt holds VFX edit's trigger" }).toBe('vfx_edit');
  });

  await test.step('loaded, a box gets nothing: its chosen LoRA shows beneath it, at its release strength', async () => {
    await open('edited');
    const loras = region.locator('.active-models li.active-lora', { hasText: 'VFX Edit' });
    await softly.poll(() => loras.count(), { message: 'its chosen LoRA has a row beneath it' }).toBe(1);
    await expect.soft(loras.locator('.active-lora-strength'), "at its release's strength, said as words").toHaveText("Strength 1 (its release's)");
    await expect.soft(loras.locator('input'), 'no control: nothing in a request sets it').toHaveCount(0);
    await expect.soft(loras.getByRole('button', { name: 'vfx_edit', exact: true }), 'with its trigger').toBeVisible();
    await canvasIdle(page);
    expect.soft(await summaryOf('edited'), 'a load puts no trigger in').toBe(WORDS);
  });

  await test.step('an edit chosen: its first trigger after the prompt, drawn; another replaces it', async () => {
    await editOf.selectOption({ label: 'Body swap' });
    await expect.poll(() => chosenOf('edited'), { message: 'body swap chosen' }).toEqual([SWAP]);
    await softly.poll(() => summaryOf('edited'), { message: "body swap's first trigger alone after the prompt" }).toBe(`${WORDS} bfs_swap`);
    await softly.poll(lit, { message: 'drawn' }).toEqual(['bfs_swap']);
    await look('swap', 'edited');
    await editOf.selectOption({ label: 'VFX edit' });
    await expect.poll(() => chosenOf('edited'), { message: 'VFX edit chosen again' }).toEqual([VFX]);
    await softly.poll(() => summaryOf('edited'), { message: "body swap's trigger replaced by VFX edit's" }).toBe(`${WORDS} vfx_edit`);
    await softly.poll(lit, { message: 'drawn' }).toEqual(['vfx_edit']);
    await look('vfx', 'edited', true);
  });

  await test.step('deleted, a trigger stays deleted: through a reload, and the words typed on', async () => {
    const summary = region.getByRole('textbox', { name: 'Summary', exact: true });
    await summary.click();
    await page.keyboard.press(process.platform === 'darwin' ? 'Meta+ArrowDown' : 'Control+End');
    for (let i = 0; i < ' vfx_edit'.length; i++) await page.keyboard.press('Backspace');
    await expect.poll(() => summaryOf('edited'), { message: 'the trigger deleted' }).toBe(WORDS);
    await reload();
    await open('edited');
    await canvasIdle(page);
    expect.soft(await summaryOf('edited'), 'reloaded, nothing comes back').toBe(WORDS);
    await region.getByRole('textbox', { name: 'Summary', exact: true }).click();
    await page.keyboard.press(process.platform === 'darwin' ? 'Meta+ArrowDown' : 'Control+End');
    await page.keyboard.type(' Boats come in.');
    await expect.poll(() => summaryOf('edited'), { message: 'the words typed on' }).toBe(`${WORDS} Boats come in.`);
    await canvasIdle(page);
    expect.soft(await summaryOf('edited'), 'nor with the words typed on').toBe(`${WORDS} Boats come in.`);
    await look('deleted', 'edited');
  });

  await test.step('a LoRA of two triggers added in Models & LoRAs appends its first, once; the other is a chip that puts it in', async () => {
    await open('scene');
    await region.getByRole('button', { name: 'Models and LoRAs', exact: true }).click();
    const picker = page.getByRole('dialog', { name: 'Models and LoRAs', exact: true });
    await picker.locator('button[data-model-slot]', { hasText: /^Base Model$/ }).click();
    await picker.getByRole('button', { name: 'LoRAs', exact: true }).click();
    await picker.locator('.model-picker-lora-option', { hasText: /BFS Body Swap/i }).click();
    await expect.poll(async () => (await asset('scene'))?.fields?.models?.base_model?.lora?.[0]?.ref, { message: 'a LoRA added' }).toMatch(new RegExp(`^${SWAP}`));
    await softly.poll(() => summaryOf('scene'), { message: 'its first trigger alone after the prompt, once' }).toBe(`${WORDS} bfs_swap`);
    await picker.getByRole('button', { name: 'Close model picker', exact: true }).click();
    await softly.poll(lit, { message: 'drawn' }).toEqual(['bfs_swap']);
    const row = region.locator('.active-models li.active-lora', { hasText: /BFS Body Swap/i });
    for (const word of swap) await expect.soft(row.getByRole('button', { name: word, exact: true }), `"${word}" a chip on its row`).toBeVisible();
    await look('added', 'scene', true);
    expect.soft(await summaryOf('scene'), 'reloaded, the same').toBe(`${WORDS} bfs_swap`);
    // The other, clicked with the caret at the prompt's end: in once, after the first.
    await region.getByRole('textbox', { name: 'Summary', exact: true }).click();
    await page.keyboard.press(process.platform === 'darwin' ? 'Meta+ArrowDown' : 'Control+End');
    await row.getByRole('button', { name: 'head swap', exact: true }).click();
    await softly.poll(async () => { const said = await summaryOf('scene'); return said.startsWith(`${WORDS} bfs_swap`) && said.split('head swap').length - 1; },
      { message: '"head swap" in once, after the first' }).toBe(1);
    await softly.poll(lit, { message: 'both drawn' }).toEqual(['bfs_swap', 'head swap']);
    await look('chip', 'scene');
  });

  // The Hub's H3 Edit, once its edits are slots by rule (packages #1017; its rules on raw weights need cozy-runtime #2295): of its five
  // LoRAs only VFX edit's is on this Hub.
  const h3 = (await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}`)).tools.find((one) => one.id === 'minimax-h3/edit');
  const slotted = Boolean(h3?.models?.some((slot) => slot.slot === 'vfx_edit'));
  test.info().annotations.push({ type: 'h3-edit', description: slotted ? 'its edits are slots: checked' : `its edits aren't slots in ${h3?.release ?? 'no release'}: not checked` });
  if (!slotted) return;
  await test.step("H3 Edit: VFX edit's trigger comes with its Edit, and goes with it", async () => {
    await project.add({ name: 'plate', tool: 'minimax-h3/edit', kind: 'video', fields: { edit: 'head_swap', summary: WORDS, beats: [{ description: 'Boats come in.', duration: 5 }] }, at: { x: 520, y: 0 } });
    await expect.poll(() => chosenOf('plate'), { message: "plate's rule chose head swap's LoRA", timeout: 120_000 }).toEqual(['alissonerdx/minimax-h3-head-swap']);
    await open('plate');
    await editOf.selectOption({ label: 'VFX edit' });
    await expect.poll(() => chosenOf('plate'), { message: 'VFX edit chosen' }).toEqual([VFX]);
    await softly.poll(() => summaryOf('plate'), { message: "VFX edit's trigger after the prompt" }).toBe(`${WORDS} vfx_edit`);
    await softly.poll(lit, { message: 'drawn' }).toEqual(['vfx_edit']);
    await look('h3-edit', 'plate', true);
    await editOf.selectOption({ label: 'Head swap' });
    await expect.poll(() => chosenOf('plate'), { message: 'head swap chosen again' }).toEqual(['alissonerdx/minimax-h3-head-swap']);
    await softly.poll(() => summaryOf('plate'), { message: "VFX edit's trigger gone with it" }).toBe(WORDS);
  });
});
