// create-tools (cl-973 core cell 2's tools half, split from create so each runs well inside its 180 s: release e2e 44431308176 timed out
// at 191 s on Linux and 217 s on Windows, every step passing, on runners 1.5× slower than its 123 s / 146 s): any image tool goes on any
// picture (Paul, 11-16): a words-only tool, which reads none (the seed's E2E Words, web/words, as a third party's may: se-175), goes on a
// picture with no tool and offers no reference, and the picture its words start from is struck through in red, its reason on hover and in
// one line under them; SDXL (since sdxl 0.15) and Qwen Image 2, which read pictures, go on in its place and use it. The X on the tool line
// takes the tool off, Undo puts it back. The words-only tool's struck picture is kept at 1440×900, on paper and dark.
// cl-1084's Tools follow-ups (Paul, 11-16: "reference inputs are offered only by tools that can use them (a text-to-image-only tool like SDXL
// shows none)"; "One name per model, cased right"): `<` offers only what the box's tool can use, so the words-only tool's list has no
// pictures and SDXL's and Qwen Image 2's have; MiniMax H3 Edit is named once, as its package writes it, on a box's tool line, kept on paper
// and dark.
// cl-1182 (Paul, 11-17, after taking qwen-image's tool for qwen-image-2's: "oh my bad, I used qwen image 1 instead of 2.1. We should
// probably call it 'qwen image 1' to avoid confusion like I just did"): Tools lists Qwen Image 1 and Qwen Image 2.1, and so does a box's tool
// line and `cozy explain`; Qwen Image 1's box offers 1, 1.7 or 2 MP, 1.7 by default. Kept on paper and dark.
// Its boxes are made as the page posts them (create drops and drags its own): a picture, a video, a sound, a Text box and a Qwen Image
// 2.1 box, the five create has made by then, so `<` is offered each kind.
const { mkdirSync } = require('fs');
const { join } = require('path');
const { test, expect, themed, canvasCard, canvasIdle, canvasNames, canvasPoint, canvasVisible, focus, png, MP4, wav } = require('./fixtures');
const { carry, rows } = require('./geometry');
const { fit, leave } = require('./hands');

/** A tool as the page names it (shown.ts toolName): its label alone where that says its package's name already. */
const named = (tool) => (!tool.package_title || tool.label.toLowerCase().includes(tool.package_title.toLowerCase()) ? tool.label : `${tool.package_title} · ${tool.label}`);
/** Whether a tool reads a picture: a slot of one, or a prompt that takes one. */
const reads = (fields) => fields.some((field) => (field.type === 'slot' && field.accept === 'image') ||
  (field.type === 'prompt' && (!field.takes || field.takes.includes('image'))) || (field.type === 'group' && reads(field.members ?? [])));

test('create-tools', { tag: '@engines' }, async ({ page, project, api, cli }) => {
  const { tools } = await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}`);
  const palette = page.getByRole('complementary', { name: 'Tools' });
  await expect(palette.getByRole('button', { name: 'Hide the tools' }), 'the tools open').toBeVisible();
  const category = (operation) => palette.locator(`button[data-operation="${operation}"]`);
  const implementations = (operation) => palette.locator(`[data-implementation][data-tool-operation="${operation}"]`);
  /** `operation`'s list open, and only it. */
  const openOperation = async (operation) => {
    if (await category(operation).getAttribute('aria-expanded') !== 'true') await category(operation).click();
    await expect(category(operation)).toHaveAttribute('aria-expanded', 'true');
  };
  const shots = join(process.env.OUT, 'web', 'create-tools');
  mkdirSync(shots, { recursive: true });
  /** The page at each of `sizes` on paper and dark, each a reload as the page's own switch keeps it, `again` putting back what's looked at,
   *  and `part` of it cropped: kept for a person to look at (Paul, 11-12: look at your own UI). Then as it was. */
  const look = async (what, again, part, sizes = [[1440, 900]]) => {
    const [was, size] = [(await project.state()).appearance?.theme ?? null, page.viewportSize()];
    const theme = (chosen) => project.look(chosen);
    for (const one of ['paper', 'dark']) for (const [width, height] of sizes) {
      await theme(one);
      await page.setViewportSize({ width, height });
      await page.reload();
      await expect(page.locator('.stage')).toBeVisible();
      await themed(page, one);
      await again();
      await canvasIdle(page);
      await page.screenshot({ path: join(shots, `${what}-${width}x${height}-${one}.png`) });
      await part().screenshot({ path: join(shots, `${what}-part-${width}x${height}-${one}.png`) });
    }
    await theme(was);
    await page.setViewportSize(size);
    await page.reload();
    await expect(page.locator('.stage')).toBeVisible();
    await again();
  };

  // The boxes create makes by hand, made as the page posts them: a picture, a video, a sound, a Text box, and a Qwen Image 2.1 box.
  const names = { picture: 'coast', video: 'take', sound: 'sea', text: 'text', tool: 'image_1' };
  await project.import(png(150), { name: names.picture, at: { x: 0, y: 480 } });
  await project.import(wav(), { name: names.sound, at: { x: 420, y: 480 } });
  await project.import(MP4, { name: names.video, at: { x: 0, y: 0 } });
  // As Tools › Text posts one (place.ts): no words yet.
  await project.add({ name: names.text, kind: 'text', fields: { text: '', node_kind: 'text' }, at: { x: 420, y: 0 } });
  await project.add({ name: names.tool, tool: 'qwen-image-2/image', kind: 'image', fields: { node_kind: 'image' }, at: { x: 840, y: 0 } });
  for (const name of Object.values(names)) await expect.poll(() => canvasVisible(page, name), { message: `${name} on the canvas` }).toBe(true);
  await fit(page);

  // A picture with no tool, right of the rest (cl-1068).
  const PHOTO = 'photo';
  /** What `<` typed at the end of `region`'s words offers, each name with what its box is, and the `<` taken back. */
  const offered = async (region) => {
    const words = region.locator('.in-frame .mention-text').first();
    if (!(await words.evaluate((el) => el === document.activeElement))) await words.click();
    await page.keyboard.press('ControlOrMeta+End');
    await page.keyboard.type(' <');
    const list = page.locator('.name-menu[role="listbox"]');
    await list.waitFor({ state: 'visible', timeout: 5000 }).catch(() => {});
    const shown = await list.getByRole('option').allTextContents();
    await page.keyboard.press('Backspace');
    await page.keyboard.press('Backspace');
    await expect(list, 'the list goes with the `<`').toHaveCount(0);
    await expect.poll(async () => (await project.asset(PHOTO)).fields?.instructions ?? '', { message: 'the words as they were' }).not.toMatch(/<\s*$/);
    const kinds = {};
    for (const name of shown) kinds[name] = (await project.asset(/^self\.\d+$/.test(name) ? PHOTO : name))?.kind ?? 'none';
    return kinds;
  };
  let reader;
  await test.step("any image tool goes on a picture: a words-only tool, which reads none, offers no reference and strikes the picture through in red, saying why; SDXL and Qwen Image 2 use it", async () => {
    // Paul, 11-16: "you can attach an image tool to any image. However, it won't be able to use references to images if it's not capable of
    // doing so, such as our SDXL tool"; "if you write <Self.3> or something it has a red line crossing it out".
    const held = Object.values(await rows(project)).filter((row) => Number.isFinite(row.x) && Number.isFinite(row.y));
    const at = { x: Math.round(Math.max(...held.map((row) => row.x + (row.w ?? 360))) + 120), y: Math.round(Math.min(...held.map((row) => row.y))) };
    await project.import(png(90), { name: PHOTO, at });
    await expect.poll(() => canvasNames(page), { message: 'the picture on the page' }).toContain(PHOTO);
    await fit(page);
    const onIt = () => canvasPoint(page, PHOTO);
    const before = await project.asset(PHOTO);
    expect(before.tool || null, 'the picture has no tool').toBeNull();
    const count = (await project.assets()).length;
    /** PHOTO in Focus, its words open in the frame over its picture. */
    const writing = async () => {
      const region = await focus(page, PHOTO);
      const words = region.locator('.in-frame .mention-text').first();
      const edit = region.getByRole('button', { name: 'Edit image', exact: true });
      await expect(words.or(edit).first()).toBeVisible();
      if (!(await words.isVisible())) await edit.click();
      await expect(words).toBeVisible();
      return region;
    };

    // A tool of Image that reads its words only, by what it declares: no picture slot, no prompt that takes one.
    await openOperation('image');
    const listed = await implementations('image').evaluateAll((all) => all.map((one) => one.dataset.tool));
    const wordsTool = tools.find((tool) => listed.includes(tool.id) && !reads(tool.fields));
    expect(wordsTool, `a tool that makes a picture and reads none, among ${listed.join(', ')}`).toBeTruthy();
    const wordsId = wordsTool.id;
    const words = implementations('image').and(palette.locator(`[data-tool="${wordsId}"]`));
    await words.scrollIntoViewIfNeeded();
    await carry(page, words, await onIt());
    await expect.poll(async () => (await project.asset(PHOTO)).tool, { message: `${wordsId} goes on the picture` }).toBe(wordsId);
    expect.soft((await project.asset(PHOTO)).versions, 'its picture stays').toBe(before.versions);
    expect.soft((await project.assets()).length, 'and makes no box').toBe(count);
    // Its words, as Edit opens them, start from the picture (`<self.N>`); a few more typed, then saved as Focus is left.
    let region = await writing();
    const typed = region.locator('.in-frame .mention-text').first();
    if (!(await typed.evaluate((el) => el === document.activeElement))) await typed.click();
    await page.keyboard.press('End');
    await page.keyboard.type(' a lighthouse');
    await expect.poll(async () => JSON.stringify((await project.asset(PHOTO)).fields), { message: 'what was typed, saved' }).toContain('a lighthouse');
    await leave(page);
    let photo;
    await expect.poll(async () => JSON.stringify({ fields: (photo = await project.asset(PHOTO)).fields, unused: photo.unused ?? [] }),
      { message: `Cozy says ${wordsId} doesn't support the image its words start from` }).toMatch(/"mention":"self\.\d+"/);
    const self = photo.unused.find((one) => /^self\.\d+$/.test(one.mention));
    // Paul, 11-16 (cl-1084): "does not support this reference".
    expect.soft(self.why, 'said in plain words').toBe(`${wordsTool.package_title} doesn't support reference images`);
    region = await writing();
    const chip = region.locator(`.in-frame .chip[data-mention=${JSON.stringify(self.mention)}]`).first();
    await expect(chip, 'the image it doesn\'t support, marked').toHaveClass(/\bunsupported\b/);
    const style = await chip.evaluate((el) => {
      const probe = document.body.appendChild(document.createElement('i'));
      probe.style.color = 'var(--bad)';
      const red = getComputedStyle(probe).color;
      probe.remove();
      const s = getComputedStyle(el);
      return { line: s.textDecorationLine, color: s.color, stroke: s.textDecorationColor, red };
    });
    expect.soft(style.line, 'struck through').toContain('line-through');
    expect.soft({ color: style.color, stroke: style.stroke }, 'in red').toEqual({ color: style.red, stroke: style.red });
    await expect.soft(chip, 'why, on hover').toHaveAttribute('title', self.why);
    await expect.soft(region.locator('.in-frame .unsupported-note'), 'and in one line under the words').toHaveText(self.why);
    await expect.soft(region.locator('.slot-control[data-accept="image"]'), 'it offers no reference').toHaveCount(0);
    await expect.soft(region.locator('.in-frame .backdrop'), 'and the picture it can\'t use is no reference behind its words').toHaveCount(0);
    // Nor does `<` offer one: its words take a Text box's words, and no picture, video or sound (cl-1084).
    const wordsOffers = await offered(region);
    expect.soft(Object.entries(wordsOffers).filter(([, kind]) => kind !== 'text'), `${wordsId}'s \`<\` offers no picture: ${JSON.stringify(wordsOffers)}`).toEqual([]);
    expect.soft(Object.keys(wordsOffers), 'and does offer the Text box').toContain(names.text);
    await look('words-unsupported', writing, () => page.getByRole('region', { name: 'Focused asset' }).locator('.in-frame'));
    await leave(page);

    /** `prefix`'s Image, which reads pictures in its prompt, in the last one's place: the picture its words start from is its reference. */
    const uses = async (prefix) => {
      await openOperation('image');
      const reading = implementations('image').and(palette.locator(`[data-tool^="${prefix}/"]`)).first();
      await reading.scrollIntoViewIfNeeded();
      const readerId = await reading.getAttribute('data-tool');
      const tool = tools.find((one) => one.id === readerId);
      expect(tool && reads(tool.fields), `${tool?.id}: a tool that reads a picture`).toBe(true);
      await carry(page, reading, await onIt());
      await expect.poll(async () => (await project.asset(PHOTO)).tool, { message: `${tool.id}, a tool that reads a picture, goes on it` }).toBe(tool.id);
      await expect.configure({ soft: true }).poll(async () => JSON.stringify((await project.asset(PHOTO)).fields?.instructions ?? null), { message: `${tool.id}: its prompt starts from the picture` }).toMatch(/<self\.\d+>/);
      await expect.configure({ soft: true }).poll(async () => (await project.asset(PHOTO)).unused ?? [], { message: `${tool.id}: nothing it can't use` }).toEqual([]);
      expect.soft((await project.assets()).length, `${tool.id}: and makes no box`).toBe(count);
      region = await writing();
      await expect.soft(region.locator('.in-frame .chip[data-mention^="self."]').first(), `${tool.id}: the picture, used`).not.toHaveClass(/\bunsupported\b/);
      await expect.soft(region.locator('.in-frame .unsupported-note'), `${tool.id}: with nothing to say`).toHaveCount(0);
      await expect.soft(region.locator('.in-frame .backdrop'), `${tool.id}: the picture its reference, behind its words`).toHaveCount(1);
      const offers = await offered(region);
      expect.soft(Object.values(offers), `${tool.id}'s \`<\` offers pictures: ${JSON.stringify(offers)}`).toContain('image');
      await leave(page);
      return tool;
    };
    // SDXL's, since sdxl 0.15 (se-175: IP-Adapter references), and then Qwen Image 2's.
    await uses('sdxl');
    reader = await uses('qwen-image-2');
  });

  await test.step("the tool line's X takes the tool off, and Undo puts it back", async () => {
    // Paul, 11-16: "there should be an X next to it, not a trash can. You are not deleting anything; you are closing it."
    const region = await focus(page, PHOTO);
    const off = region.getByRole('button', { name: 'Remove image tool', exact: true });
    await expect(off).toBeVisible();
    await expect.soft(off, 'it says what it does').toHaveAttribute('title', `Take ${named(reader)} off`);
    await expect.soft(off.locator('svg'), 'an X, not a trash can').toHaveAttribute('data-icon', 'close');
    const versions = (await project.asset(PHOTO)).versions;
    await off.click();
    await expect.poll(async () => (await project.asset(PHOTO)).tool || null, { message: 'the tool is off' }).toBeNull();
    const note = page.locator('.notes .note').filter({ hasText: `Took ${named(reader)} off ${PHOTO}. Its images stay.` });
    await expect(note, 'a note says so, with Undo').toHaveCount(1);
    expect.soft((await project.asset(PHOTO)).versions, 'its picture stays').toBe(versions);
    await note.getByRole('button', { name: 'Undo', exact: true }).click();
    await expect.poll(async () => (await project.asset(PHOTO)).tool, { message: 'Undo puts it back' }).toBe(reader.id);
    await leave(page);
  });

  await test.step('MiniMax H3 Edit on the dropped video: its tool line says it once, as the package writes it, on paper and dark', async () => {
    // cl-1084: the tool line said "Minimax H3 · MiniMax H3 Edit".
    const target = names.video;
    await fit(page);
    await openOperation('video');
    const edit = implementations('video').and(palette.locator('[data-tool="minimax-h3/edit"]'));
    await edit.scrollIntoViewIfNeeded();
    await carry(page, edit, await canvasPoint(page, target));
    await expect.poll(async () => (await project.asset(target)).tool, { message: 'MiniMax H3 Edit goes on the video' }).toBe('minimax-h3/edit');
    await expect.configure({ soft: true }).poll(async () => (await canvasCard(page, target))?.rendered?.labels.tool?.fullText, { message: 'its tool line on the canvas' }).toBe('MiniMax H3 Edit');
    const line = () => page.getByRole('region', { name: 'Focused asset' }).locator('.tool-line').first();
    const focused = async () => {
      await focus(page, target);
      await expect(line()).toBeVisible();
    };
    await focused();
    await expect.soft(line().locator('.image-tool-name'), 'and in Focus').toHaveText('MiniMax H3 Edit');
    // The tool line's first row, where cl-1084's prefix was: since cl-1281 the rows beneath it list the slots' default LoRAs,
    // named as their authors name them ("alissonerdx / Minimax H3 VFX Edit"), which are not the tool's name.
    const named = (await line().innerText()).split('\n')[0];
    expect.soft(named, 'its tool line names no tool twice, nor cases it other than as its package writes it').not.toMatch(/Minimax H3|MiniMax H3 · MiniMax H3/);
    await look('tool-line', focused, line);
    await leave(page);
  });

  await test.step("Qwen Image 1 and Qwen Image 2.1, each called by its model, in Tools, on a box's tool line and in cozy explain; Qwen Image 1's megapixels (cl-1182)", async () => {
    await fit(page);
    const row = (operation, id) => implementations(operation).and(palette.locator(`[data-tool="${id}"]`));
    await openOperation('upscale_image');
    await expect.soft(row('upscale_image', 'qwen-image-2/upscale'), "Qwen Image 2.1's Upscale").toHaveText('Qwen Image 2.1');
    await openOperation('image');
    await expect.soft(row('image', 'qwen-image/image'), 'Qwen-Image-2512 and Qwen-Image-Edit-2511: Qwen Image 1').toHaveText('Qwen Image 1');
    await expect.soft(row('image', 'qwen-image-2/image'), 'Qwen-Image-2.1: Qwen Image 2.1').toHaveText('Qwen Image 2.1');
    // The Hub tool box, Qwen Image 2.1's: its tool line, then Qwen Image 1 in its place.
    const line = async () => (await canvasCard(page, names.tool))?.rendered?.labels.tool?.fullText;
    await expect.configure({ soft: true }).poll(line, { message: "Qwen Image 2.1's tool line" }).toBe('Qwen Image 2.1 · Image');
    const one = row('image', 'qwen-image/image');
    await one.scrollIntoViewIfNeeded();
    await carry(page, one, await canvasPoint(page, names.tool));
    await expect.poll(async () => (await project.asset(names.tool)).tool, { message: 'Qwen Image 1 goes on the box' }).toBe('qwen-image/image');
    await expect.configure({ soft: true }).poll(line, { message: "Qwen Image 1's tool line" }).toBe('Qwen Image 1 · Image');
    const region = page.getByRole('region', { name: 'Focused asset' });
    const focused = async () => {
      await focus(page, names.tool);
      await expect(region.locator('.tool-line').first()).toBeVisible();
    };
    await focused();
    await expect.soft(region.locator('.tool-line .image-tool-name').first(), 'and in Focus').toHaveText('Qwen Image 1 · Image');
    const megapixels = region.getByRole('radiogroup', { name: 'Megapixels', exact: true });
    await expect.soft(megapixels.getByRole('radio'), "Qwen Image 1's sizes").toHaveText(['1 MP', '1.7 MP', '2 MP']);
    await expect.soft(megapixels.getByRole('radio', { name: '1.7 MP', exact: true }), 'its native 1328², by default').toHaveAttribute('aria-checked', 'true');
    await look('qwen-image-1', focused, () => region);
    await leave(page);
    await look('tools-qwen', async () => { await expect(palette.locator('.tool-groups.stale')).toHaveCount(0); await openOperation('image'); }, () => palette, [[1440, 900], [1920, 1080]]);
    // `cozy explain` names each tool as the page does.
    const said = await cli('explain', 'image');
    expect.soft(said.code, `cozy explain image: ${said.stderr.slice(-500)}`).toBe(0);
    for (const [id, name] of [['qwen-image/image', 'Qwen Image 1 · Image'], ['qwen-image-2/image', 'Qwen Image 2.1 · Image'], ['qwen-image-2/upscale', 'Qwen Image 2.1 · Upscale']]) {
      expect.soft(said.stdout, `cozy explain image: ${id} as ${name}`).toMatch(new RegExp(`^${id}\\s+${name}, makes image$`, 'm'));
    }
  });
});
