// llm-reads (xs-204). Paul, 11-16: "yeah; we shoul dhave a general purpose reference system where you can reference other nodes like
// <Answer> or something. That's hwat we do with iamges". A mention reads a box's state the same way whatever it is: an LLM box's answer
// goes into the prompt that mentions it as words, in place, as a Text box's words do, never as a file. On project/llm.py (local, CPU, it
// answers with what it was asked): `answer` is an LLM box and `reply`'s prompt says "<answer> …". Render on reply makes the answer first,
// as a render makes what it reads first; reply's run is sent the answer's words in place, with no reference; a new answer puts reply out
// of date, as a new picture would; `<answer.1>` reads version 1 whatever comes after. The page's `<` offers LLM boxes in any prompt, as
// words (SDXL's, which takes no picture), and reply's prompt shows its mention as a chip with no picture under it. The `<` list and
// reply's prompt are kept at 1440×900 and 1920×1080, on paper and dark.
const { mkdirSync } = require('fs');
const { join } = require('path');
const { test, expect, themed, canvasIdle, canvasVisible, fixtureTool, focus, png } = require('./fixtures');
const { fit, leave } = require('./hands');

const LLM = fixtureTool('llm.py/ask');
/** SDXL's Image, which the isolated Hub holds for every web job (setup.js SEED): its words take words, no file. */
const SDXL = 'sdxl/image';
const RUN_MS = 240_000;

test('llm-reads', async ({ page, project, api }) => {
  test.setTimeout(360_000);
  await project.add({ name: 'answer', tool: LLM, kind: 'text', fields: { instructions: 'Say hello.' }, at: { x: 0, y: 0 } });
  await project.add({ name: 'reply', tool: LLM, kind: 'text', fields: { instructions: '<answer> Then say goodbye.' }, at: { x: 720, y: 0 } });
  await project.import(png(120), { name: 'coast', at: { x: 0, y: 360 } });
  await project.add({ name: 'words', tool: SDXL, kind: 'image', at: { x: 720, y: 360 } });
  await expect.poll(async () => (await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}`)).tools.some((tool) => tool.local && tool.id === LLM),
    { message: 'the project lists its local LLM', timeout: 180_000 }).toBe(true);
  await page.reload();
  await expect(page.locator('.stage')).toBeVisible();
  for (const name of ['answer', 'reply', 'coast', 'words']) await expect.poll(() => canvasVisible(page, name)).toBe(true);
  await fit(page);

  const shots = join(process.env.OUT, 'web', 'llm-reads');
  mkdirSync(shots, { recursive: true });
  /** The page at 1440×900 and 1920×1080, on paper and dark, each a reload as the page's own switch keeps it, `again` putting back what's
   *  looked at (Paul, 11-12: look at your own UI). Then as it was. */
  const look = async (what, again) => {
    const [was, size] = [(await project.state()).appearance?.theme ?? null, page.viewportSize()];
    const theme = (chosen) => project.look(chosen);
    for (const [width, height] of [[1440, 900], [1920, 1080]]) for (const one of ['paper', 'dark']) {
      await theme(one);
      await page.setViewportSize({ width, height });
      await page.reload();
      await expect(page.locator('.stage')).toBeVisible();
      await themed(page, one);
      await fit(page);
      await again();
      await page.screenshot({ path: join(shots, `${what}-${width}x${height}-${one}.png`) });
      await leave(page);
    }
    await theme(was);
    await page.setViewportSize(size);
    await page.reload();
    await expect(page.locator('.stage')).toBeVisible();
    await fit(page);
  };
  const versions = async (name) => (await project.asset(name))?.versions ?? 0;
  /** What run `run` was sent: its recorded input. */
  const sent = async (run) => (await api.get(`${project.at(`/runs/${run}`)}?detail=full`)).request?.input ?? null;
  const render = async (name) => {
    const region = await focus(page, name);
    await region.getByRole('button', { name: 'Render', exact: true }).click();
    await leave(page);
  };

  let first;
  await test.step('Render on reply makes the answer first, and reply is sent its words in place', async () => {
    await render('reply');
    await expect.poll(() => versions('answer'), { message: 'the answer it reads is made first', timeout: RUN_MS }).toBe(1);
    await expect.poll(() => versions('reply'), { message: 'then reply', timeout: RUN_MS }).toBe(1);
    first = (await project.asset('answer')).prefixes[0].main?.text ?? '';
    expect(first, 'the answer is words').toContain('ANSWER to: Say hello.');
    const reply = await project.asset('reply');
    const input = await sent(reply.prefixes[0].run);
    expect(input?.instructions?.text, `reply is sent the answer's words in place: ${JSON.stringify(input)}`).toBe(`${first} Then say goodbye.`);
    expect.soft(input?.instructions?.references ?? [], 'and no file').toEqual([]);
    expect.soft(reply.prefixes[0].main?.text ?? '', 'the package read them').toContain(`ANSWER to: ${first} Then say goodbye. | shown: nothing`);
    expect.soft(reply.standing, 'reply stands on it').toBe('current');
  });

  await test.step('a new answer puts reply out of date, as a new picture would', async () => {
    const was = (await project.asset('reply')).reads?.find((read) => read.name === 'answer');
    expect(was?.key, 'reply reads answer, keyed by what it sends').toBeTruthy();
    await render('answer');
    await expect.poll(() => versions('answer'), { message: 'a second answer', timeout: RUN_MS }).toBe(2);
    const answer = await project.asset('answer');
    expect.soft(answer.prefixes[1].main?.text, 'a new answer').not.toBe(first);
    await expect.poll(async () => (await project.asset('reply'))?.standing, { message: 'reply goes out of date' }).toBe('out_of_date');
    const now = (await project.asset('reply')).reads?.find((read) => read.name === 'answer');
    expect.soft(now?.key, 'its read moved').not.toBe(was.key);
    expect.soft((await project.state()).stale?.map((stale) => stale.name) ?? [], 'the project says so').toContain('reply');
  });

  await test.step('<answer.1> reads version 1 whatever comes after', async () => {
    await project.add({ name: 'fixed', tool: LLM, kind: 'text', fields: { instructions: '<answer.1> Again.' }, at: { x: 1440, y: 0 } });
    await fit(page);
    await expect.poll(() => canvasVisible(page, 'fixed')).toBe(true);
    await render('fixed');
    await expect.poll(() => versions('fixed'), { message: 'fixed renders', timeout: RUN_MS }).toBe(1);
    const fixed = await project.asset('fixed');
    expect.soft(await sent(fixed.prefixes[0].run), 'version 1\'s words, not the newest').toMatchObject({ instructions: { text: `${first} Again.`, references: [] } });
    expect.soft(fixed.reads?.find((read) => read.name === 'answer')?.version, 'a read of version 1').toBe(1);
    expect.soft(fixed.standing, 'which nothing moves').toBe('current');
    expect.soft(await versions('answer'), 'and it made no answer').toBe(2);
  });

  await test.step("the page's `<` offers LLM boxes in any prompt, as words", async () => {
    const offered = page.locator('.name-menu[role="listbox"]');
    const ask = async () => {
      const region = await focus(page, 'words');
      const typed = region.locator('.mention-text[data-field="instructions"]').first();
      await expect(typed).toBeVisible();
      if (!(await typed.evaluate((el) => el === document.activeElement))) await typed.click();
      await page.keyboard.press('ControlOrMeta+a');
      await page.keyboard.type('Like <');
      await expect(offered.getByRole('option'), "SDXL's words: the LLM boxes, and not coast, a picture").toHaveText(['answer', 'fixed', 'reply']);
      await canvasIdle(page);
    };
    await ask();
    await leave(page);
    await look('offered', ask);
  });

  await test.step("reply's prompt: its mention a chip, with no picture under it", async () => {
    const shown = async () => {
      const region = await focus(page, 'reply');
      // A box of words shows its answer in the frame, and its prompt under it.
      const typed = region.locator('.mention-text[data-field="instructions"]').first();
      await expect(typed.locator('.chip[data-mention="answer"]'), 'a chip').toBeVisible();
      await expect.soft(typed.locator('.chip[data-mention="answer"]'), 'that its tool uses').not.toHaveClass(/\bunsupported\b/);
      await expect.soft(region.locator('.mention-thumb'), 'words have no picture').toHaveCount(0);
      await canvasIdle(page);
    };
    await shown();
    await leave(page);
    await look('reply-prompt', shown);
  });
});
