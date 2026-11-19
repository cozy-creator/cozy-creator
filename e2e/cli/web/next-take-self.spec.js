// next-take-self (se-151). A box's next take starts from the take looked at, `<self.N>` (Paul 11-06), where its tool reads a box of its kind,
// as putting the tool on one starts it (readsFrom, Calcifer's with_tool): its first required slot of that kind, else its main text. Qwen
// Image reads its picture in its words: its next take from version 2 opens on `<self.2>` there. Qwen Image 2.1's Upscale reads it in `image`:
// its next take from the version approved (2), then from the one looked at (1), puts that one in `image` and leaves its prompt the person's
// own (Paul, 11-18: Upscale's prompt is where a Caption box goes, `<caption_1>`, and a `<self.N>` there sent the picture a second time).
// MiniMax H3's Upscale reads a take in `video`: picking take 2 moves it there. Each is what Calcifer's binder sends for the box
// (`?call=true`), never only what the page holds. The two next takes are kept at 1440×900 and 1920×1080, on paper and dark.
const { mkdirSync } = require('fs');
const { join } = require('path');
const { test, expect, themed, canvasIdle, canvasVisible, focus, png, DARK_OPEN, MP4 } = require('./fixtures');
const { fit, leave } = require('./hands');

const QWEN = 'qwen-image-2/image';
const UPSCALE = 'qwen-image-2/upscale';
const H3_UPSCALE = 'minimax-h3/upscale';

const hex = (sha) => String(sha ?? '').replace(/^sha256:/, '');
const tile = (region, noun, number) => region.getByRole('button', { name: `${noun} ${number}`, exact: true });

test('next-take-self', async ({ page, project, api }) => {
  // Two takes each, then its tool's fields as putting it on a box of its kind writes them: `<self.1>` where the tool reads it.
  const boxes = [['qwen', QWEN, 'image', [png(40), png(200)], { instructions: '<self.1>' }], ['up', UPSCALE, 'image', [png(80), png(160)], { image: '<self.1>' }],
    ['clip', H3_UPSCALE, 'video', [MP4, DARK_OPEN], { video: '<self.1>' }]];
  for (const [i, [name, tool, kind, files, fields]] of boxes.entries()) {
    await project.add({ name, tool, kind, at: { x: 400 * i, y: 0 } });
    for (const file of files) await api.post(project.at(`/assets/${name}/versions`), { source: await project.upload(file) });
    await api.patch(project.at(`/assets/${name}`), { fields });
  }
  const shaOf = async (name, number) => hex((await project.asset(name))?.prefixes?.find((version) => version.number === number)?.main?.sha256);
  await expect.poll(async () => Promise.all(['qwen', 'up', 'clip'].map(async (name) => (await project.asset(name))?.versions ?? 0))).toEqual([2, 2, 2]);
  const tools = (await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}`)).tools.map((tool) => tool.id);
  expect(tools, 'the Hub lists the three tools').toEqual(expect.arrayContaining([QWEN, UPSCALE, H3_UPSCALE]));
  await page.reload();
  await expect(page.locator('.stage')).toBeVisible();
  for (const name of ['qwen', 'up', 'clip']) await expect.poll(() => canvasVisible(page, name)).toBe(true);
  await fit(page);

  const shots = join(process.env.OUT, 'web', 'next-take-self');
  mkdirSync(shots, { recursive: true });
  /** The page at 1440×900 and 1920×1080, on paper and dark, each a reload as the page's own switch keeps it, `again` putting back what's
   *  looked at (Paul, 11-12: look at your own UI). Then as it was. */
  const look = async (what, again) => {
    const [was, size] = [(await project.state()).appearance?.theme ?? null, page.viewportSize()];
    for (const [width, height] of [[1440, 900], [1920, 1080]]) for (const one of ['paper', 'dark']) {
      await project.look(one);
      await page.setViewportSize({ width, height });
      await page.reload();
      await expect(page.locator('.stage')).toBeVisible();
      await themed(page, one);
      await fit(page);
      await again();
      await canvasIdle(page);
      await page.screenshot({ path: join(shots, `${what}-${width}x${height}-${one}.png`) });
      // Out of Focus before the next reload, which would open on it.
      if (await page.getByRole('region', { name: 'Focused asset' }).count()) await leave(page);
    }
    await project.look(was);
    await page.setViewportSize(size);
    await page.reload();
    await expect(page.locator('.stage')).toBeVisible();
    await fit(page);
  };
  /** What Calcifer's binder sends for the box now, as Render sends it. */
  const call = async (name) => {
    const read = await api.get(`${project.at(`/assets/${name}`)}?call=true`);
    expect(read.call, `${name}'s call: ${JSON.stringify(read.call_warnings ?? null)}`).toBeTruthy();
    return read.call;
  };
  /** Its next take from version `number`, as a person writes one: the version looked at, then its next take. */
  const nextFrom = async (name, number) => {
    const region = await focus(page, name);
    await tile(region, 'version', number).click();
    await region.getByRole('button', { name: 'Its next take', exact: true }).first().click();
    await expect(region.getByLabel('Take', { exact: true })).toHaveText('New take');
    return region;
  };

  await test.step("Qwen Image's next take from version 2 opens on <self.2> in its words", async () => {
    await nextFrom('qwen', 2);
    await expect.poll(async () => (await project.asset('qwen'))?.fields?.instructions?.trim(), { message: 'its words start from version 2' }).toBe('<self.2>');
    await leave(page);
    const sent = await call('qwen');
    // Calcifer spells a box's own take as the box: `<self.2>` goes as `<qwen.2>`.
    expect.soft((sent.instructions?.references ?? []).map((one) => [one.key, hex(one.file)]), `sent version 2 by mention: ${JSON.stringify(sent.instructions)}`)
      .toEqual([['qwen.2', await shaOf('qwen', 2)]]);
    await look('qwen-next-take', async () => { await nextFrom('qwen', 2); });
  });

  await test.step("Upscale's next take: the take in image, its prompt left the person's", async () => {
    // Version 2 approved, then its next take: from version 2.
    let region = await focus(page, 'up');
    await tile(region, 'version', 2).dblclick();
    await expect.poll(async () => (await project.asset('up'))?.approved ?? null, { message: 'version 2 approved' }).toBe(2);
    await region.getByRole('button', { name: 'Its next take', exact: true }).first().click();
    await expect.poll(async () => (await project.asset('up'))?.fields?.image, { message: 'image is the version approved' }).toBe('<self.2>');
    expect.soft((await project.asset('up')).fields?.prompt ?? '', "and its prompt holds nothing it wasn't given").toBe('');
    await expect.soft(region.locator('.mention-text[data-field="prompt"]').first(), 'its words in Focus: empty').toHaveText('');
    await leave(page);
    let sent = await call('up');
    expect.soft(hex(sent.image), 'Calcifer sends version 2').toBe(await shaOf('up', 2));
    expect.soft({ text: sent.prompt?.text ?? '', references: sent.prompt?.references ?? [] }, 'and no prompt').toEqual({ text: '', references: [] });
    // Version 1 looked at, then its next take: from version 1.
    region = await nextFrom('up', 1);
    await expect.poll(async () => (await project.asset('up'))?.fields?.image, { message: 'image is the version looked at' }).toBe('<self.1>');
    expect.soft((await project.asset('up')).fields?.prompt ?? '', 'its prompt still empty').toBe('');
    await leave(page);
    sent = await call('up');
    expect.soft(hex(sent.image), 'Calcifer sends version 1').toBe(await shaOf('up', 1));
    await look('upscale-next-take', async () => { await nextFrom('up', 1); });
  });

  await test.step("MiniMax H3's Upscale: picking take 2 moves its video there", async () => {
    const region = await focus(page, 'clip');
    await tile(region, 'take', 2).click();
    await expect.poll(async () => (await project.asset('clip'))?.fields?.video, { message: 'video is take 2' }).toBe('<self.2>');
    expect.soft((await project.asset('clip')).fields?.prompt ?? '', 'its prompt untouched').toBe('');
    await leave(page);
    const sent = await call('clip');
    expect.soft(hex(sent.video?.file), `Calcifer sends take 2: ${JSON.stringify(sent.video)}`).toBe(await shaOf('clip', 2));
  });
});
