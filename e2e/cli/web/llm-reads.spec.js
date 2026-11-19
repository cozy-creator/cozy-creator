// llm-reads (xs-204). Paul, 11-16: "yeah; we shoul dhave a general purpose reference system where you can reference other nodes like
// <Answer> or something. That's hwat we do with iamges". A mention reads a box's state the same way whatever it is: an LLM box's answer
// goes into the prompt that mentions it as words, in place, as a Text box's words do, never as a file. On project/llm.py (local, CPU, it
// answers with what it was asked): `answer` is an LLM box and `reply`'s prompt says "<answer> …". Render on answer, then on reply (a
// render makes only what it names, cl-1192); reply's run is sent the answer's words in place, with no reference; a new answer moves what
// reply reads and nothing says reply is out of date, as with a new picture; `<answer.1>` reads version 1 whatever comes after. The page's
// `<` offers LLM boxes in any prompt, as words (SDXL's, beside the picture it reads since sdxl 0.15, se-175), and reply's prompt shows its
// mention as a chip with no picture under it. The `<` list and reply's prompt are kept at 1440×900 and 1920×1080, on paper and dark. A take's note
// shows a picture of an image or a video only: an LLM's words land with none and no `/thumbs` asked of them (#4031 asked one of each, and
// its 400s were console errors), a still lands with its own, and `/thumbs` refuses a text file at once. The Assets drawer's rows ask the
// same: a picture's row draws its thumbnail, and a box of words' asks none (it asked of every asset but a sound's).
const { randomUUID } = require('crypto');
const { mkdirSync } = require('fs');
const { join } = require('path');
const { test, expect, themed, canvasIdle, canvasVisible, fixtureTool, focus, png } = require('./fixtures');
const { fit, leave } = require('./hands');

const LLM = fixtureTool('llm.py/ask');
const STILL = `${fixtureTool('tool.py')}/still`;
/** SDXL's Image, which the isolated Hub holds for every web job (setup.js SEED): its words take words and pictures. */
const SDXL = 'sdxl/image';
const RUN_MS = 240_000;

test('llm-reads', async ({ page, project, api }) => {
  test.setTimeout(480_000);
  // Every thumbnail the page asks for, by its path.
  const thumbs = [];
  page.on('request', (request) => { const { pathname } = new URL(request.url()); if (pathname.includes('/thumbs/')) thumbs.push(pathname.toLowerCase()); });
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
  await test.step('Render on answer, then on reply: reply is sent its words in place', async () => {
    await render('answer');
    await expect.poll(() => versions('answer'), { message: 'the answer', timeout: RUN_MS }).toBe(1);
    await render('reply');
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

  await test.step('a new answer moves what reply reads, and nothing says reply is out of date, as with a new picture', async () => {
    const was = (await project.asset('reply')).reads?.find((read) => read.name === 'answer');
    expect(was?.key, 'reply reads answer, keyed by what it sends').toBeTruthy();
    await render('answer');
    await expect.poll(() => versions('answer'), { message: 'a second answer', timeout: RUN_MS }).toBe(2);
    const answer = await project.asset('answer');
    expect.soft(answer.prefixes[1].main?.text, 'a new answer').not.toBe(first);
    await expect.poll(async () => (await project.asset('reply'))?.reads?.find((read) => read.name === 'answer')?.key, { message: 'its read moved' }).not.toBe(was.key);
    const reply = await project.asset('reply');
    expect.soft({ standing: reply.standing, version: reply.version, changed: reply.changed }, 'reply stands on its take, unmarked').toEqual({ standing: 'current', version: 1, changed: undefined });
    expect.soft((await project.state()).stale, 'the project says nothing is out of date').toBeUndefined();
    expect.soft(await versions('reply'), 'and nothing rendered reply').toBe(1);
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
      await expect(offered.getByRole('option'), "SDXL's words: the LLM boxes, and coast, a picture it reads").toHaveText(['answer', 'coast', 'fixed', 'reply']);
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

  await test.step("a take's note: words show no picture and ask for none, a still shows its own, and /thumbs refuses words at once", async () => {
    const ready = (name) => page.locator('[data-note]').filter({ hasText: new RegExp(`of ${name} is ready`) });
    const renderHere = (name) => api.call('POST', project.at('/runs'), { names: [name], on: ['local'] }, { headers: { 'Idempotency-Key': randomUUID() } });
    await renderHere('answer');
    await expect(ready('answer'), "answer's take is said").toBeVisible({ timeout: RUN_MS });
    await expect.soft(ready('answer').locator('.note-picture'), 'with no picture').toHaveCount(0);
    const words = (await Promise.all(['answer', 'reply', 'fixed'].map((name) => project.asset(name))))
      .flatMap((asset) => asset.prefixes.map((take) => take.main?.sha256)).filter(Boolean).map((sha) => sha.replace(/^sha256:/, '').toLowerCase());
    expect(words.length, 'the LLM takes are files').toBeGreaterThanOrEqual(5);
    expect.soft(thumbs.filter((at) => words.some((hex) => at.includes(hex))), 'no thumbnail asked of words').toEqual([]);

    await project.add({ name: 'still', tool: STILL, kind: 'image', fields: { instructions: 'a still' }, at: { x: 1440, y: 360 } });
    await renderHere('still');
    await expect(ready('still'), "still's take is said").toBeVisible({ timeout: RUN_MS });
    await expect.soft(ready('still').locator('img.note-picture'), 'with its picture').toBeVisible({ timeout: 60_000 });
    const picture = (await project.asset('still')).prefixes.at(-1)?.main?.sha256?.replace(/^sha256:/, '').toLowerCase();
    expect.soft(thumbs.some((at) => picture && at.includes(picture)), "its thumbnail was asked").toBe(true);

    const asked = await api.raw('GET', project.at(`/thumbs/${words[0]}`));
    const said = await asked.json().catch(() => null);
    expect.soft({ status: asked.status, code: said?.error?.code }, `a text file's thumbnail is refused at once: ${JSON.stringify(said)}`).toEqual({ status: 400, code: 'thumb_unsupported' });
    console.log(`llm-reads thumbs: ${asked.status} ${said?.error?.message}`);
  });

  await test.step("the Assets drawer: a picture's row draws its thumbnail, and a box of words' asks none", async () => {
    const menu = page.locator('details.main-menu');
    const drawer = page.locator('aside.drawer');
    const before = thumbs.length;
    if ((await menu.getAttribute('open')) === null) await menu.locator('summary').click();
    await menu.locator('nav').getByRole('link', { name: 'Assets', exact: true }).click();
    for (const name of ['answer', 'reply', 'fixed', 'coast', 'still']) await expect(drawer.locator(`.assetrow[data-asset="${name}"]`), `${name} is listed`).toBeVisible();
    for (const name of ['coast', 'still']) {
      const row = drawer.locator(`.assetrow[data-asset="${name}"] .asset-thumb img`);
      await expect.poll(() => row.evaluate((img) => img.complete && img.naturalWidth > 0).catch(() => false), { message: `${name}'s row draws its thumbnail`, timeout: 60_000 }).toBe(true);
    }
    for (const name of ['answer', 'reply', 'fixed']) await expect.soft(drawer.locator(`.assetrow[data-asset="${name}"] .asset-thumb img`), `${name}'s row has no picture`).toHaveCount(0);
    const shown = (await Promise.all(['answer', 'reply', 'fixed'].map((name) => project.asset(name)))).map((asset) => asset.sha256?.replace(/^sha256:/, '').toLowerCase()).filter(Boolean);
    expect(shown.length, 'each box of words shows a file').toBe(3);
    expect.soft(thumbs.slice(before).filter((at) => shown.some((hex) => at.includes(hex))), "no thumbnail asked of a box of words' row").toEqual([]);
  });
});
