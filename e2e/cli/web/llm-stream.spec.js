// llm-stream (xs-209). Paul, 11-17: "we can type text into boxes and get a reply from it (ideally streaming the results), and also we can
// get captions for images (ideally streaming)"; "all llm-output should be streaming where possible". The real Qwen3.5 0.8B, on this computer's
// CPU through the qwen3 package's Write tool (qwen3.5's before it), writes a long answer on a Text box: while its run goes the box at rest
// shows its words growing, each look a longer start of the last, and Focus shows them growing too; a reload mid-way shows the words so far.
// Its version lands with the words it streamed. Kept mid-stream at rest and in Focus at 1440×900 and 1920×1080, on paper and dark.
const { randomUUID } = require('crypto');
const { mkdirSync, writeFileSync } = require('fs');
const { join } = require('path');
const { test, expect, themed, canvasIdle, canvasVisible, focus } = require('./fixtures');
const { fit, leave } = require('./hands');

const MODEL = 'huihui-ai/huihui-qwen3.5-0.8b-abliterated@0.1.0';
const PROMPT = 'Write a long story in twelve chapters, each about two hundred words, of a lighthouse keeper through one winter of storms. Use plain sentences.';
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
  await project.add({ name: NAME, tool, kind: 'text', fields: { instructions: PROMPT, max_tokens: 2400, seed: 7, models: { model: { ref: MODEL } } }, at: { x: 0, y: 0 } });
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

// The GPU check (release-gate/sessions/llm-stream.sh): a real model on a rental, STREAM_TOOL (`qwen3/write`, its default the abliterated
// Qwen3.8-27B) on STREAM_MACHINE: a long story cold, then warm; then a short question thinking at `low`, `medium` and `xhigh` effort
// (its chat template's `reasoning_effort`), whose box says it's thinking and shows no words until its answer begins, each effort thinking
// longer than the one before. Each render's time from its ask to its first word on the box at rest, the words a second the box shows
// against the tokens a second its run counts over the same span, and a thought's tokens, in OUT/web/llm-stream-gpu.
test('llm-stream: a real model on a GPU', { tag: '@gpu' }, async ({ page, project, api }) => {
  const machine = process.env.STREAM_MACHINE, tool = process.env.STREAM_TOOL;
  test.skip(!machine || !tool, 'release-gate/sessions/llm-stream.sh sets STREAM_MACHINE and STREAM_TOOL');
  test.setTimeout(3_600_000);
  const story = 'Write a long story, at least eight hundred words, of a lighthouse keeper through one winter of storms.';
  const model = process.env.STREAM_MODEL ? { models: { model: { ref: process.env.STREAM_MODEL } } } : {};
  await project.add({ name: NAME, tool, kind: 'text', fields: { instructions: story, max_tokens: 1500, ...model }, at: { x: 0, y: 0 } });
  await page.reload();
  await expect(page.locator('.stage')).toBeVisible();
  await expect.poll(() => canvasVisible(page, NAME)).toBe(true);
  await fit(page);
  const out = join(process.env.OUT, 'web', 'llm-stream-gpu');
  mkdirSync(out, { recursive: true });
  const count = (words) => (words.match(/\S+/g) ?? []).length;
  const renders = [];
  let before = story;
  const QUESTION = 'Why is the sea salty, and why are rivers not? Answer in three sentences.';
  for (const [n, effort] of [[1], [2], [3, 'low'], [4, 'medium'], [5, 'xhigh']]) {
    const thinking = Boolean(effort);
    if (thinking) await api.patch(project.at(`/assets/${NAME}`), { fields: { instructions: QUESTION, thinking: true, reasoning_effort: effort, max_tokens: 8000 } });
    const asked = Date.now();
    await api.call('POST', project.at('/runs'), { names: [NAME], on: [machine] }, { headers: { 'Idempotency-Key': randomUUID() } });
    const shown = [], counted = [];
    let number, first;
    for (;;) {
      const asset = await project.asset(NAME);
      if ((asset?.versions ?? 0) >= n) break;
      number = number ?? asset?.making?.[0];
      const at = Date.now() - asked;
      const words = plain(await restWords(page, NAME));
      // Its words, once they're no longer what the box showed before its ask (its prompt, then the last version).
      if (words && words !== plain(before)) {
        first = first ?? at;
        shown.push([at, count(words), words]);
      }
      if (number !== undefined) {
        const run = await api.get(project.at(`/runs/${number}`));
        if (run.progress?.unit === 'tokens' && run.status === 'in_progress') counted.push([at, run.progress.completed, run.progress.stage, Boolean(first)]);
      }
      expect(Date.now() - asked, `render ${n} lands within 30 min`).toBeLessThan(1_800_000);
      await page.waitForTimeout(250);
    }
    const version = (await project.asset(NAME)).prefixes.at(-1);
    const run = await api.get(project.at(`/runs/${number}`));
    // Over the span words showed and tokens were counted, both.
    const from = Math.max(first ?? Infinity, counted[0]?.[0] ?? Infinity);
    const span = (rows) => rows.filter(([at]) => at >= from);
    const rate = (rows) => (rows.length > 1 ? ((rows.at(-1)[1] - rows[0][1]) * 1000) / (rows.at(-1)[0] - rows[0][0]) : null);
    renders.push({
      render: n, effort: effort ?? null, thought_tokens: thinking ? (counted.filter(([, , stage]) => /thinking/i.test(stage)).at(-1)?.[1] ?? 0) : null,
      first_words: shown[0]?.[2].slice(0, 120) ?? null, run: number, machine: run.machine, gpu: run.gpu, executed_ms: run.execution_ms, landed_ms: Date.now() - asked,
      first_word_ms: first ?? null, words_per_s: rate(span(shown)), tokens_per_s: rate(span(counted)),
      words: count(version.main?.text ?? ''), tokens: counted.at(-1)?.[1] ?? null, stages: [...new Set(counted.map(([, , stage]) => stage))],
      shown: shown.length, text: (version.main?.text ?? '').slice(0, 300),
    });
    expect.soft(first, `render ${n}: words showed on the box before its version landed`).toBeTruthy();
    // What the box showed as it came is how its answer starts, each time: nothing of a thought, nothing that went away.
    const text = plain(version.main?.text ?? '');
    const stray = shown.find(([, , words]) => !text.startsWith(words));
    expect.soft(stray?.[2].slice(0, 200) ?? null, `render ${n}: every look at its words is how its answer starts`).toBeNull();
    if (thinking) {
      // While it thought, its stage said so (its first word may come a progress report, 0.25 s, before the stage moves on).
      expect.soft(counted.some(([, , stage]) => /thinking/i.test(stage)), `render ${n} (${effort}): its stage said it was thinking`).toBe(true);
    }
    before = version.main?.text ?? '';
    await page.screenshot({ path: join(out, `landed-${n}.png`) });
  }
  writeFileSync(join(out, 'numbers.json'), JSON.stringify({ tool, model: process.env.STREAM_MODEL ?? 'default', renders }, null, 2));
  // Each effort thinks longer than the one before (se-145: low asks for brief thinking, xhigh to think carefully).
  const thoughts = renders.filter((one) => one.effort).map((one) => one.thought_tokens);
  expect.soft(thoughts[0] < thoughts[2], `low thinks less than xhigh: ${JSON.stringify(thoughts)} tokens`).toBe(true);
});
