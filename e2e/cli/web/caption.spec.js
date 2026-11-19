// caption (se-151). Paul, 11-18: "we can introduce a caption tool generally. I'm guessing what we'd do is (1) add the caption tool to an image,
// (2) run it, it produces a caption, (3) then add the upscale tool, and use that caption as one of its inputs. That way they're composable. I
// think the caption tool goes under the LLM section generally". The qwen3 package's Caption on the real Qwen3.5 0.8B, on this computer's CPU:
// Tools › LLM lists it beside Write, each by its package's title and its own label; let go on a picture it makes a box of words reading it;
// Render writes a caption there, its run sent the picture by mention. Then Qwen Image 2.1's Upscale goes on the picture, and `<caption_1>`
// typed in its prompt is the caption's words: what Calcifer's binder sends for that box (`?call=true`, as Render sends it; Upscale itself
// can't run on a CPU), and the caption's own call still carries the picture through that loop of boxes. The LLM row, the caption beside
// its picture and Upscale's prompt are kept at 1440×900 and 1920×1080, on paper and dark.
const { readFileSync, mkdirSync } = require('fs');
const { join } = require('path');
const { test, expect, themed, canvasIdle, canvasPoint, canvasText, canvasVisible, focus } = require('./fixtures');
const { carry } = require('./geometry');
const { fit, leave, toolRow } = require('./hands');

const CAPTION = 'qwen3/caption';
const UPSCALE = 'qwen-image-2/upscale';
const MODEL = 'huihui-ai/huihui-qwen3.5-0.8b-abliterated@0.1.0';
// A caption is at most 120 words; 160 tokens holds most of one at the slowest runner's 1.3 tokens/s (llm-stream.spec.js) in about two minutes.
const TOKENS = 160;
const RUN_MS = 600_000;

test('caption', async ({ page, project, api }) => {
  test.setTimeout(900_000);
  await project.import(readFileSync(process.env.PICTURE), { name: 'dog', at: { x: 0, y: 0 } });
  const listed = async () => (await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}`)).tools;
  await expect.poll(async () => { const tools = await listed(); return [CAPTION, UPSCALE].filter((id) => !tools.some((tool) => tool.id === id)); },
    { message: 'the Hub lists Caption and Upscale', timeout: 180_000 }).toEqual([]);
  await page.reload();
  await expect(page.locator('.stage')).toBeVisible();
  await expect.poll(() => canvasVisible(page, 'dog')).toBe(true);
  await fit(page);

  const shots = join(process.env.OUT, 'web', 'caption');
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
  /** What run `run` was sent: its recorded input. */
  const sent = async (run) => (await api.get(`${project.at(`/runs/${run}`)}?detail=full`)).request?.input ?? null;

  await test.step('Tools › LLM lists Caption beside Write, each by its package and its own label', async () => {
    await toolRow(page, 'LLM', CAPTION);
    const rows = page.locator('[data-implementation][data-tool-operation="llm"]');
    const labels = Object.fromEntries(await rows.evaluateAll((all) => all.map((one) => [one.dataset.tool, one.textContent])));
    expect.soft(labels[CAPTION], 'Caption').toBe('Qwen3 Caption');
    expect.soft(labels['qwen3/write'], 'Write').toBe('Qwen3 Write');
    await look('llm-row', async () => { await toolRow(page, 'LLM', CAPTION); });
  });

  let caption;
  await test.step('let go on the picture: a box of words beside it reading it; Render writes its caption', async () => {
    const was = (await project.assets()).map((asset) => asset.name);
    await carry(page, await toolRow(page, 'LLM', CAPTION), await canvasPoint(page, 'dog'));
    let made;
    await expect.poll(async () => (made = (await project.assets()).find((asset) => !was.includes(asset.name)))?.name ?? null, { message: 'Caption makes a box' }).not.toBeNull();
    caption = made.name;
    expect.soft(made, 'a box of words with Caption on it').toMatchObject({ name: 'caption_1', kind: 'text', tool: CAPTION });
    expect.soft(made.fields?.instructions, 'its words mention the picture').toBe('<dog>');
    expect.soft((await project.asset('dog')).tool ?? null, 'the picture stays as it was').toBeNull();
    await page.keyboard.press('Escape');
    await canvasIdle(page);
    // The CPU's checkpoint, and a caption's worth of tokens: what a person picks in its model menu and under Advanced.
    await api.patch(project.at(`/assets/${caption}`), { fields: { models: { model: { ref: MODEL } }, max_tokens: TOKENS } });
    await expect.poll(async () => (await project.asset(caption))?.fields?.max_tokens, { message: 'its model and length' }).toBe(TOKENS);
    const region = await focus(page, caption);
    await region.getByRole('button', { name: 'Render', exact: true }).click();
    await leave(page);
    await expect.poll(async () => (await project.asset(caption))?.versions ?? 0, { message: 'its caption lands', timeout: RUN_MS }).toBe(1);
    const [version] = (await project.asset(caption)).prefixes;
    const words = version.main?.text ?? '';
    console.log(`caption: ${words}`);
    expect(words.trim().split(/\s+/).length, `a caption in words: ${JSON.stringify(words)}`).toBeGreaterThanOrEqual(8);
    expect.soft(words, 'of the dog in the picture').toMatch(/\bdogs?\b|\bpupp(y|ies)\b/i);
    const input = await sent(version.run);
    expect.soft(input?.instructions?.text, `its run is sent its words: ${JSON.stringify(input)}`).toBe('<dog>');
    expect.soft((input?.instructions?.references ?? []).map((one) => [one.key, one.type]), 'with the picture they mention').toEqual([['dog', 'image']]);
    await expect.configure({ soft: true }).poll(() => canvasText(page, caption), { message: 'its caption on the box at rest' }).toContain(words.trim().split(/\s+/).slice(0, 3).join(' '));
    await look('caption-beside-picture', async () => {});
  });

  await test.step("Upscale on the picture, `<caption_1>` in its prompt: Calcifer sends the caption's words as its prompt", async () => {
    await carry(page, await toolRow(page, 'Upscale Image', UPSCALE), await canvasPoint(page, 'dog'));
    await expect.poll(async () => (await project.asset('dog'))?.tool ?? null, { message: 'Upscale goes on the picture' }).toBe(UPSCALE);
    expect.soft((await project.asset('dog')).fields?.image, 'reading its picture').toBe('<self.1>');
    /** Upscale's words opened as a person opens them: its next take. */
    const prompt = async () => {
      const region = await focus(page, 'dog');
      await region.getByRole('button', { name: 'Its next take' }).first().click();
      const typed = region.locator('.mention-text[data-field="prompt"]').first();
      await expect(typed).toBeVisible();
      return { region, typed };
    };
    const { typed } = await prompt();
    if (!(await typed.evaluate((el) => el === document.activeElement))) await typed.click();
    await page.keyboard.press('ControlOrMeta+a');
    await page.keyboard.type('<capt');
    const offered = page.locator('.name-menu[role="listbox"]');
    await expect(offered.getByRole('option'), 'the caption is offered, as words').toHaveText([caption]);
    await page.keyboard.press('Enter');
    await expect.poll(async () => (await project.asset('dog'))?.fields?.prompt ?? '', { message: 'its prompt mentions the caption' }).toMatch(new RegExp(`^<${caption}>\\s*$`));
    await expect.soft(typed.locator(`.chip[data-mention="${caption}"]`), 'a chip').toBeVisible();
    await expect.soft(typed.locator(`.chip[data-mention="${caption}"]`), 'that Upscale uses').not.toHaveClass(/\bunsupported\b/);
    await leave(page);
    const words = (await project.asset(caption)).prefixes[0].main.text;
    const read = await api.get(`${project.at('/assets/dog')}?call=true`);
    expect(read.call, `Calcifer makes Upscale's call: ${JSON.stringify(read.call_warnings ?? null)}`).toBeTruthy();
    expect(read.call.prompt?.text?.trim(), `its prompt is the caption's words: ${JSON.stringify(read.call)}`).toBe(words.trim());
    expect.soft(read.call.prompt?.references ?? [], 'sent as words, with no file').toEqual([]);
    expect.soft(read.call.image, 'and its picture').toBeTruthy();
    // Paul's flow is a loop of boxes: dog's Upscale reads caption_1, which reads dog. A caption made again still sees the picture: its call
    // carries dog's file, whatever Calcifer says of the loop (se-151 review).
    const dog = await project.asset('dog');
    const picture = String(dog.prefixes?.[0]?.main?.sha256 ?? dog.sha256 ?? '').replace(/^sha256:/, '');
    const again = await api.get(`${project.at(`/assets/${caption}`)}?call=true`);
    expect(again.call, `Calcifer makes the caption's call again: ${JSON.stringify(again.call_warnings ?? null)}`).toBeTruthy();
    expect((again.call.instructions?.references ?? []).map((one) => [one.key, one.type, String(one.file).replace(/^sha256:/, '')]),
      `the caption's call carries the picture: ${JSON.stringify(again.call.instructions)}`).toEqual([['dog', 'image', picture]]);
    await look('upscale-prompt', async () => { await prompt(); });
  });
});
