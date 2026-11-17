// llm-stream (xs-209). Paul, 11-17: "we can type text into boxes and get a reply from it (ideally streaming the results), and also we can
// get captions for images (ideally streaming)"; "all llm-output should be streaming where possible". The real Qwen3.5 0.8B, on this computer's
// CPU through the qwen3 package's Write tool (qwen3.5's before it), writes a long answer on a Text box: while its run goes the box at rest
// shows its words growing, each look a longer start of the last, and Focus shows them growing too; a reload mid-way shows the words so far.
// Its version lands with the words it streamed. Kept mid-stream at rest and in Focus at 1440×900 and 1920×1080, on paper and dark.
const { randomUUID } = require('crypto');
const { mkdirSync } = require('fs');
const { join } = require('path');
const { test, expect, themed, canvasIdle, canvasVisible, focus } = require('./fixtures');
const { fit, leave } = require('./hands');

const MODEL = 'qwen/qwen3.5-0.8b@0.1.0';
const PROMPT = 'Write a long story, about a thousand words, of a lighthouse keeper and a storm. Use plain sentences.';
const NAME = 'story';

/** The words a box shows at rest, as the canvas drew them. */
const restWords = (page, name) => page.evaluate((name) => document.querySelector('.canvas-root')?.canvasInspection?.card(name)?.rendered?.words ?? null, name);
/** Words as drawn, less a cut's ellipsis and the space at their ends: what any later look must start with. */
const plain = (words) => (words ?? '').replace(/…$/, '').trim();

test('llm-stream', async ({ page, project, api }) => {
  test.setTimeout(900_000);
  // The Write tool of the package the checkout holds, published at this Hub's root.
  let tool;
  for (const name of ['qwen3/write', 'qwen3.5/write']) {
    const found = await api.raw('GET', `/v1/targets?target=${encodeURIComponent(name)}&project=${encodeURIComponent(project.id)}`);
    if (found.ok) { tool = name; break; }
  }
  expect(tool, 'a Qwen Write tool is published here').toBeTruthy();
  await project.add({ name: NAME, tool, kind: 'text', fields: { instructions: PROMPT, max_tokens: 1400, seed: 7, models: { model: { ref: MODEL } } }, at: { x: 0, y: 0 } });
  await page.reload();
  await expect(page.locator('.stage')).toBeVisible();
  await expect.poll(() => canvasVisible(page, NAME)).toBe(true);
  await fit(page);

  const shots = join(process.env.OUT, 'web', 'llm-stream');
  mkdirSync(shots, { recursive: true });
  await api.call('POST', project.at('/runs'), { names: [NAME], on: ['local'] }, { headers: { 'Idempotency-Key': randomUUID() } });
  let number;
  await expect.poll(async () => (number = (await project.asset(NAME))?.making?.[0]) ?? null, { message: 'its render is a run', timeout: 60_000 }).not.toBeNull();
  const run = () => api.get(project.at(`/runs/${number}`));
  const going = async () => ['queued', 'in_progress'].includes((await run()).status);
  /** Its words, once they are an answer's: not the prompt it shows before any came. */
  const answer = (words) => (words && plain(words) && !PROMPT.startsWith(plain(words)) ? plain(words) : null);

  const looks = [];
  await test.step('at rest, its words grow while its run goes', async () => {
    const began = Date.now();
    while (looks.length < 4 && Date.now() - began < 600_000) {
      const words = answer(await restWords(page, NAME));
      const still = await going();
      if (words && still && words !== looks.at(-1)) looks.push(words);
      if (!still) break;
      await page.waitForTimeout(400);
    }
    expect(looks.length, `words seen growing on the box while its run went: ${JSON.stringify(looks.map((one) => one.length))}`).toBeGreaterThanOrEqual(3);
    for (const [i, words] of looks.entries()) {
      if (i) expect.soft(words.startsWith(looks[i - 1]), `look ${i + 1} goes on from look ${i}: ${JSON.stringify(looks[i - 1].slice(-40))} → ${JSON.stringify(words.slice(0, 80))}`).toBe(true);
    }
  });

  /** The page at 1440×900 and 1920×1080, on paper and dark, each a reload, mid-stream: kept for a person to look at (Paul, 11-12). */
  const look = async (what, again) => {
    const size = page.viewportSize();
    for (const [width, height] of [[1440, 900], [1920, 1080]]) for (const one of ['paper', 'dark']) {
      await project.look(one);
      await page.setViewportSize({ width, height });
      await page.reload();
      await expect(page.locator('.stage')).toBeVisible();
      await themed(page, one);
      await fit(page);
      const region = await again();
      // A reload mid-way shows the words so far, before any more come.
      await expect.configure({ soft: true }).poll(async () => answer(await restWords(page, NAME)) ?? (region ? answer(await region.locator('.answer').textContent()) : null), { message: `${what}: its words so far, after a reload`, timeout: 30_000 }).not.toBeNull();
      await canvasIdle(page);
      if (!(await going())) expect.soft(false, `${what} ${width}x${height} ${one}: the run ended before its look mid-stream`).toBe(true);
      await page.screenshot({ path: join(shots, `${what}-${width}x${height}-${one}.png`) });
      if (region) await leave(page);
    }
    await project.look(null);
    await page.setViewportSize(size);
  };

  await test.step('kept mid-stream at rest', async () => look('rest', async () => null));

  await test.step('in Focus, its words grow too', async () => {
    const region = await focus(page, NAME);
    const writing = region.locator('.answer[data-writing]');
    await expect(writing, 'Focus shows the words as they come').toBeVisible({ timeout: 30_000 });
    const first = plain(await writing.textContent());
    await expect.poll(async () => plain(await region.locator('.answer').textContent()).length, { message: 'and they grow', timeout: 60_000 }).toBeGreaterThan(first.length);
    expect.soft(plain(await region.locator('.answer').textContent()).startsWith(first), 'going on from what was there').toBe(true);
    await leave(page);
    await look('focus', async () => focus(page, NAME));
  });

  await test.step('its version lands with the words it streamed', async () => {
    await expect.poll(async () => (await project.asset(NAME))?.versions ?? 0, { message: 'version 1 lands', timeout: 900_000 }).toBe(1);
    const [version] = (await project.asset(NAME)).prefixes;
    const text = version.main?.text ?? '';
    expect(version.main?.field, 'a version of words').toBe('text');
    for (const words of looks) expect.soft(text.startsWith(words), `what streamed is how it starts: ${JSON.stringify(words.slice(-40))}`).toBe(true);
    await canvasIdle(page);
    await expect.configure({ soft: true }).poll(async () => plain(await restWords(page, NAME)), { message: 'the box at rest shows its version' }).toBe(text.trim());
  });
});
