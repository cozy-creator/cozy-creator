// llm (xs-204). Paul, 11-16: "ideally I want LLM tools as well. Like I can create a text box, and then type text into it. Then I can attach
// an LLM and it can generate more text. Or I can attach an LLM (ideally VLLM) to an image node, or a video node, and ask it to produce a
// caption, and it'll produce text". On an LLM in the shape of cozy_runtime.vlm's box (project/llm.py: a prompt that reads pictures and videos,
// a seed, an answer in words), local, so it runs here: Tools › LLM lists it. Let go on a Text box with words, it goes on it and the words are
// its prompt; each render is a new version, its answer on the box at rest and in Focus; Undo leaves the plain Text box with its words. Let go
// on a picture, it makes a new text box right of it whose prompt starts with the picture's mention, opened with the caret after it: what's
// typed is asked, and its answer read the picture. On a video the same, and Undo takes the new box away. A tool that makes video from a
// picture (project/clip.py) let go on a picture makes a new video box reading it, one step that Undo takes away. On a sound, which it can't
// read, the LLM changes nothing and says why. The LLM row, the Text box with its answer and the new box beside the picture are kept at
// 1440×900 and 1920×1080, on paper and dark.
const { mkdirSync, readFileSync, writeFileSync } = require('fs');
const { join } = require('path');
const { test, expect, canvasIdle, canvasNames, canvasPoint, canvasText, canvasVisible, fixtureTool, focus, MP4, png, wav } = require('./fixtures');
const { carry, rowsOf } = require('./geometry');
const { fit, leave, toolRow } = require('./hands');

const LLM = fixtureTool('llm.py/ask');
const CLIP = fixtureTool('clip.py/clip');
const WORDS = 'Write one line about the sea.';
const RUN_MS = 240_000;

test('llm', async ({ page, project, api }) => {
  // Three renders here and a reload for each of twelve shots, beside a lane's other cells.
  test.setTimeout(300_000);
  // Two rows a box's room apart: what's let go on, then what the drops make right of it; the two boxes that name the local tools (which
  // is what lists them) under them.
  await project.import(png(120), { name: 'coast', at: { x: 0, y: 0 } });
  await project.add({ name: 'note', fields: { text: WORDS }, at: { x: 720, y: 0 } });
  await project.import(MP4, { name: 'take', at: { x: 0, y: 330 } });
  await project.import(wav(), { name: 'sea', at: { x: 720, y: 330 } });
  await project.add({ name: 'asker', tool: LLM, kind: 'text', fields: { instructions: 'Say hello.' }, at: { x: 0, y: 660 } });
  await project.add({ name: 'mover', tool: CLIP, kind: 'video', at: { x: 720, y: 660 } });
  const listed = async () => (await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}`)).tools;
  await expect.poll(async () => { const tools = await listed(); return [LLM, CLIP].filter((id) => !tools.some((tool) => tool.local && tool.id === id)); },
    { message: 'the project lists its local tools', timeout: 180_000 }).toEqual([]);
  await page.reload();
  await expect(page.locator('.stage')).toBeVisible();
  for (const name of ['coast', 'note', 'take', 'sea']) await expect.poll(() => canvasVisible(page, name)).toBe(true);
  await fit(page);

  const shots = join(process.env.OUT, 'web', 'llm');
  mkdirSync(shots, { recursive: true });
  /** The page at 1440×900 and 1920×1080, on paper and dark, each a reload as the page's own switch keeps it, `again` putting back what's
   *  looked at: kept for a person to look at (Paul, 11-12: look at your own UI). Then as it was. */
  const look = async (what, again) => {
    const [was, size] = [await page.evaluate(() => localStorage.getItem('cozy.theme')), page.viewportSize()];
    const theme = (chosen) => page.evaluate((one) => (one === null ? localStorage.removeItem('cozy.theme') : localStorage.setItem('cozy.theme', one)), chosen);
    for (const [width, height] of [[1440, 900], [1920, 1080]]) for (const one of ['paper', 'dark']) {
      await theme(one);
      await page.setViewportSize({ width, height });
      await page.reload();
      await expect(page.locator('.stage')).toBeVisible();
      await fit(page);
      await again();
      await canvasIdle(page);
      await page.screenshot({ path: join(shots, `${what}-${width}x${height}-${one}.png`) });
    }
    await theme(was);
    await page.setViewportSize(size);
    await page.reload();
    await expect(page.locator('.stage')).toBeVisible();
    await fit(page);
  };
  /** Undo, and the canvas back still from the box it had opened: a carry lets go where it was aimed. */
  const undo = async () => {
    await page.locator('.top .history').getByRole('button', { name: 'Undo', exact: true }).click();
    await canvasIdle(page);
  };
  const history = async () => (await api.get(project.at('/history'))).entries;
  /** The box a drop made: the one more there is. */
  const madeAfter = async (was, what) => {
    let made;
    await expect.poll(async () => (made = (await project.assets()).find((asset) => !was.includes(asset.name)))?.name ?? null, { message: `${what} makes a box` }).not.toBeNull();
    return made;
  };
  /** Its words, opened in place, the caret in them: the field it reads in, focused. */
  const wordsOf = (name) => page.locator(`.card.here[data-asset=${JSON.stringify(name)}] .in-frame .mention-text[data-field="instructions"]`);

  await test.step('Tools › LLM lists it, and what makes text is under LLM', async () => {
    const row = await toolRow(page, 'LLM', LLM);
    await expect.soft(row).toHaveAttribute('data-tool-operation', 'llm');
    const text = (await listed()).filter((tool) => tool.makes === 'text');
    // One tool is that row, called LLM; several, each by its package's title (Qwen3.6, Qwen3.5), with no page of its own for any.
    const labels = await page.locator('[data-implementation][data-tool-operation="llm"]').evaluateAll((all) => all.map((one) => [one.dataset.tool, one.textContent]));
    expect.soft(labels.map(([id]) => id).sort(), 'every tool that makes text, under LLM').toEqual(text.map((tool) => tool.id).sort());
    if (text.length > 1) for (const [id, label] of labels) expect.soft(label, `${id}: its package's title`).toBe(text.find((tool) => tool.id === id).package_title);
    await look('llm-row', async () => { await toolRow(page, 'LLM', LLM); });
  });

  await test.step('let go on a Text box with words, it goes on it: its words are the prompt', async () => {
    const count = (await project.assets()).length;
    await carry(page, await toolRow(page, 'LLM', LLM), await canvasPoint(page, 'note'));
    await expect.poll(async () => (await project.asset('note'))?.tool ?? null, { message: 'the LLM goes on note' }).toBe(LLM);
    const note = await project.asset('note');
    expect.soft(note.kind, 'still a box of words').toBe('text');
    expect.soft(note.fields?.instructions, 'its words are the prompt').toBe(WORDS);
    expect.soft(note.fields?.text ?? null, 'and no longer its text').toBeNull();
    expect.soft((await project.assets()).length, 'and makes no box').toBe(count);
  });

  let answers;
  await test.step('each render is a new version: its answer on the box at rest and in Focus', async () => {
    const region = await focus(page, 'note');
    for (const n of [1, 2]) {
      await region.getByRole('button', { name: 'Render', exact: true }).click();
      await expect.poll(async () => (await project.asset('note'))?.versions ?? 0, { message: `version ${n} lands`, timeout: RUN_MS }).toBe(n);
    }
    const note = await project.asset('note');
    expect(note.prefixes.map((version) => version.main?.field), 'each a version of words').toEqual(['text', 'text']);
    answers = note.prefixes.map((version) => version.main?.text ?? '');
    for (const answer of answers) expect.soft(answer, 'asked its words').toContain(`ANSWER to: ${WORDS} | shown: nothing`);
    expect.soft(answers[0], 'each render its own').not.toBe(answers[1]);
    await region.getByRole('button', { name: 'version 1', exact: true }).click();
    await expect.soft(region.locator('.answer'), 'Focus shows the version picked').toHaveText(answers[0]);
    await region.getByRole('button', { name: 'version 2', exact: true }).click();
    await expect.soft(region.locator('.answer'), 'and the other').toHaveText(answers[1]);
    await page.screenshot({ path: join(shots, `focus-answer-${page.viewportSize().width}x${page.viewportSize().height}.png`) });
    await leave(page);
    await expect.configure({ soft: true }).poll(() => canvasText(page, 'note'), { message: 'its answer on the box at rest' }).toContain('ANSWER to:');
    await look('text-box-answer', async () => {});
  });

  await test.step('Undo leaves the plain Text box with its words', async () => {
    await undo();
    await expect.poll(async () => (await project.asset('note'))?.tool ?? null, { message: 'the LLM comes off' }).toBeNull();
    const note = await project.asset('note');
    expect.soft(note.kind).toBe('text');
    expect.soft(note.fields?.text, 'its words are its own again').toBe(WORDS);
    await expect.configure({ soft: true }).poll(() => canvasText(page, 'note'), { message: 'drawn as its words' }).toContain(WORDS);
  });

  let caption;
  await test.step('let go on a picture: a new text box right of it, its prompt the picture\'s mention, the caret after it', async () => {
    const was = (await project.assets()).map((asset) => asset.name);
    const steps = (await history()).length;
    await carry(page, await toolRow(page, 'LLM', LLM), await canvasPoint(page, 'coast'));
    const made = await madeAfter(was, 'the LLM on coast');
    caption = made.name;
    expect.soft(made, 'a box of words with the LLM on it').toMatchObject({ kind: 'text', tool: LLM });
    expect.soft(made.fields?.instructions, 'its prompt starts with the picture').toBe('<coast>');
    const coast = await project.asset('coast');
    expect.soft({ tool: coast.tool ?? null, versions: coast.versions }, 'the picture stays as it was').toEqual({ tool: null, versions: 1 });
    const rows = await rowsOf(project, ['coast', caption]);
    expect.soft(rows[caption].x > rows.coast.x && Math.abs(rows[caption].y - rows.coast.y) <= 1, `right of it: ${JSON.stringify(rows[caption])} from ${JSON.stringify(rows.coast)}`).toBe(true);
    const words = wordsOf(caption);
    await expect(words, 'opened on its words').toBeFocused();
    await expect.soft(words.locator('.chip[data-mention="coast"]'), 'the picture a chip').toBeVisible();
    await page.keyboard.type(' write a caption');
    await expect.poll(async () => (await project.asset(caption))?.fields?.instructions, { message: 'what\'s typed is after it' }).toBe('<coast> write a caption');
    expect.soft((await project.asset(caption)).reads?.map((read) => read.name) ?? [], 'linked as a mention is').toContain('coast');
    expect.soft((await history()).filter((entry) => !entry.undone).length - steps >= 1, 'its drop is a step of the history').toBe(true);
    await page.keyboard.press('ControlOrMeta+Enter');
    await expect.poll(async () => (await project.asset(caption))?.versions ?? 0, { message: 'its answer lands', timeout: RUN_MS }).toBe(1);
    const [version] = (await project.asset(caption)).prefixes;
    expect.soft(version.main?.text, 'asked what was typed, with the picture').toMatch(/^ANSWER to: <[^>]+> write a caption \| shown: an image \d+px wide \|/);
    // Esc leaves it (cl-939).
    await page.keyboard.press('Escape');
    await expect(page.locator(`.card.here[data-asset=${JSON.stringify(caption)}]`), 'left').toHaveCount(0);
    await canvasIdle(page);
    await expect.configure({ soft: true }).poll(() => canvasText(page, caption), { message: 'its answer on the box at rest' }).toContain('ANSWER to:');
    await look('beside-picture', async () => {});
  });

  await test.step('let go on a video: a new box reading it, one step, which Undo takes away', async () => {
    const was = (await project.assets()).map((asset) => asset.name);
    await carry(page, await toolRow(page, 'LLM', LLM), await canvasPoint(page, 'take'));
    const made = await madeAfter(was, 'the LLM on take');
    expect.soft(made, 'a box of words with the LLM on it').toMatchObject({ kind: 'text', tool: LLM });
    expect.soft(made.fields?.instructions, 'its prompt starts with the video').toBe('<take>');
    await expect(wordsOf(made.name), 'opened on its words').toBeFocused();
    await undo();
    await expect.poll(async () => (await project.assets()).some((asset) => asset.name === made.name), { message: 'Undo takes it away' }).toBe(false);
    await expect.configure({ soft: true }).poll(async () => (await canvasNames(page)).includes(made.name), { message: 'from the canvas too' }).toBe(false);
    expect.soft((await project.asset('take')).tool ?? null, 'the video stays as it was').toBeNull();
  });

  await test.step('a tool that makes video from a picture, let go on one: a new video box reading it, which Undo takes away', async () => {
    const was = (await project.assets()).map((asset) => asset.name);
    await carry(page, await toolRow(page, 'Video', CLIP), await canvasPoint(page, 'coast'));
    const made = await madeAfter(was, 'Clip on coast');
    expect.soft(made, 'a video box with Clip on it').toMatchObject({ kind: 'video', tool: CLIP });
    expect.soft(made.fields?.instructions, 'its prompt starts with the picture').toBe('<coast>');
    expect.soft((await project.asset('coast')).tool ?? null, 'the picture stays as it was').toBeNull();
    await undo();
    await expect.poll(async () => (await project.assets()).some((asset) => asset.name === made.name), { message: 'Undo takes it away' }).toBe(false);
  });

  await test.step('on a sound, which it can\'t read, the LLM changes nothing and says why', async () => {
    const count = (await project.assets()).length;
    const before = await project.asset('sea');
    await canvasIdle(page);
    await carry(page, await toolRow(page, 'LLM', LLM), await canvasPoint(page, 'sea'));
    await expect(page.locator('.notes .note .line').filter({ hasText: "can't use audio, so it can't go on sea" }), 'it says why').toHaveCount(1);
    const after = await project.asset('sea');
    expect.soft({ tool: after.tool ?? null, kind: after.kind, fields: after.fields }, 'the sound stays as it was').toEqual({ tool: before.tool ?? null, kind: before.kind, fields: before.fields });
    expect.soft((await project.assets()).length, 'and no box is made').toBe(count);
  });
});

// xs-204's GPU proof, release-gate/sessions/llm-tools.sh: the same hands on a real model, its renders on a rental. LLM_MACHINE is where the
// project's renders go (`local` in the session's $0 pre-flight); LLM_TOOL the tool (`qwen3.6/write`, the dev home's checkout's; a local
// fixture's path in the pre-flight); LLM_PICTURE and LLM_VIDEO the files, boxes named `photo` and `clip` so their names say nothing of
// them; LLM_SEES_PICTURE and LLM_SEES_VIDEO what an answer that saw each says (case-insensitive patterns). A Text box with words and the
// LLM on it rendered twice: two versions of words, different, each whole in Focus and the newest on the box at rest. The LLM let go on
// the picture and on the video: a new box of words right of it, `<name>` its prompt, the caret after it; " write a caption" typed and
// rendered: its run's input carries the file and its answer describes it. Answers, runs and machines in $OUT/web/llm-gpu/answers.json,
// with the page at 1440×900 and 1920×1080, on paper and dark.
test('llm: real models on a GPU', { tag: '@gpu' }, async ({ page, project, api, cli }) => {
  const machine = process.env.LLM_MACHINE, want = process.env.LLM_TOOL;
  test.skip(!machine || !want, 'release-gate/sessions/llm-tools.sh sets LLM_MACHINE and LLM_TOOL');
  test.setTimeout(3_600_000);
  // A render waits on its machine's install and download the first time: a hang, not slowness, fails it (the session's watch).
  const RENDER_MS = 30 * 60_000;
  const sees = { picture: new RegExp(process.env.LLM_SEES_PICTURE ?? '.', 'i'), video: new RegExp(process.env.LLM_SEES_VIDEO ?? '.', 'i') };
  const shots = join(process.env.OUT, 'web', 'llm-gpu');
  mkdirSync(shots, { recursive: true });
  const kept = { machine, tool: want, boxes: {} };
  const keep = () => writeFileSync(join(shots, 'answers.json'), JSON.stringify(kept, null, 2));
  const listed = async () => (await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}`)).tools;
  /** A version's run as the CLI reads it: where it ran, and whether its input carries `digest`. */
  const runOf = async (run, digest) => {
    const said = await cli('--json', 'run', 'watch', String(run), '--once', '--full');
    let record = {};
    try { record = JSON.parse(said.stdout); } catch { record = JSON.parse(said.stdout.trim().split('\n').filter((line) => line.startsWith('{')).at(-1) ?? '{}'); }
    const input = JSON.stringify(record.request?.input ?? null);
    return { run, machine: record.machine ?? null, gpu: record.gpu ?? null, status: record.status ?? null, execution_ms: record.execution_ms ?? null, carries: digest ? input.includes(digest) : null, input: input.slice(0, 600) };
  };
  /** `name`'s version `n` landed; a run that ended without it fails at once, not at the wait's end: what was making it, and the runs. */
  const lands = async (name, n) => {
    const start = Date.now();
    let seen = false, idle = null;
    for (;;) {
      const asset = await project.asset(name);
      if ((asset?.versions ?? 0) >= n) return;
      const making = asset?.making ?? [];
      if (making.length) [seen, idle] = [true, null];
      else idle ??= Date.now();
      if ((idle && Date.now() - idle > (seen ? 45_000 : 180_000)) || Date.now() - start > RENDER_MS) {
        const runs = await cli('--json', 'run', 'ls', '--limit', '3');
        throw new Error(`${name}'s version ${n} never landed (${Math.round((Date.now() - start) / 1000)} s; making ${JSON.stringify(making)}, seen ${seen}): ${runs.stdout.slice(0, 1500)}`);
      }
      await page.waitForTimeout(2_000);
    }
  };
  const shoot = async (what) => {
    for (const [width, height] of [[1440, 900], [1920, 1080]]) for (const theme of ['paper', 'dark']) {
      await page.setViewportSize({ width, height });
      await page.evaluate((dark) => { if (dark) document.documentElement.dataset.theme = 'dark'; else delete document.documentElement.dataset.theme; }, theme === 'dark');
      await fit(page);
      await page.screenshot({ path: join(shots, `${what}-${width}x${height}-${theme}.png`) });
    }
    await page.evaluate(() => { delete document.documentElement.dataset.theme; });
    await page.setViewportSize({ width: 1600, height: 1000 });
    await fit(page);
  };

  await api.call('PUT', project.at('/machines'), { machines: [machine] });
  await project.import(readFileSync(process.env.LLM_PICTURE), { name: 'photo', at: { x: 0, y: 0 } });
  await project.add({ name: 'note', fields: { text: 'Write one sentence about the sea at night.' }, at: { x: 720, y: 0 } });
  await project.import(readFileSync(process.env.LLM_VIDEO), { name: 'clip', at: { x: 0, y: 330 } });
  // A local tool is listed once a box names it (the pre-flight's fixture); a checkout's as any first-party tool is.
  if (want.startsWith('/')) await project.add({ name: 'asker', tool: want, kind: 'text', fields: { instructions: 'Say hello.' }, at: { x: 720, y: 330 } });
  let tool;
  await expect.poll(async () => (tool = (await listed()).find((one) => one.makes === 'text' && (one.id === want || one.id.endsWith(`/${want}`))))?.id ?? null,
    { message: `Tools lists ${want}, which makes text`, timeout: 180_000 }).not.toBeNull();
  kept.tool = { id: tool.id, package_title: tool.package_title, label: tool.label, source: tool.source ?? null, release: tool.release };
  keep();
  await page.reload();
  await expect(page.locator('.stage')).toBeVisible();
  for (const name of ['photo', 'note', 'clip']) await expect.poll(() => canvasVisible(page, name)).toBe(true);
  await fit(page);

  await test.step(`${tool.package_title} let go on a Text box with words, rendered twice: two versions of words`, async () => {
    await carry(page, await toolRow(page, 'LLM', tool.id), await canvasPoint(page, 'note'));
    await expect.poll(async () => (await project.asset('note'))?.tool ?? null, { message: 'the LLM goes on note' }).toBe(tool.id);
    const region = await focus(page, 'note');
    for (const n of [1, 2]) {
      await region.getByRole('button', { name: 'Render', exact: true }).click();
      await lands('note', n);
    }
    const note = await project.asset('note');
    kept.boxes.note = [];
    for (const version of note.prefixes) kept.boxes.note.push({ version: version.number, field: version.main?.field ?? null, text: version.main?.text ?? '', ...(await runOf(version.run)) });
    keep();
    const answers = kept.boxes.note.map((version) => version.text);
    expect(kept.boxes.note.map((version) => version.field), 'each a version of words').toEqual(['text', 'text']);
    for (const answer of answers) expect(answer.trim().length, 'an answer in words').toBeGreaterThan(0);
    expect.soft(answers[0], 'each render its own').not.toBe(answers[1]);
    for (const version of kept.boxes.note) expect.soft(version.machine, `version ${version.version} ran on ${machine}`).toBe(machine);
    for (const [at, answer] of answers.entries()) {
      await region.getByRole('button', { name: `version ${at + 1}`, exact: true }).click();
      await expect.soft(region.locator('.answer'), `version ${at + 1} whole in Focus`).toHaveText(answer);
      await page.screenshot({ path: join(shots, `focus-version-${at + 1}.png`) });
    }
    await leave(page);
    const start = answers[1].trim().split(/\s+/).slice(0, 3).join(' ');
    await expect.configure({ soft: true }).poll(async () => (await canvasText(page, 'note'))?.replace(/\s+/g, ' ') ?? '', { message: 'its answer on the box at rest' }).toContain(start);
    await shoot('text-box');
  });

  for (const [kind, name] of [['picture', 'photo'], ['video', 'clip']]) {
    await test.step(`${tool.package_title} let go on the ${kind}: a new box of words beside it, "write a caption" rendered describes it`, async () => {
      const was = (await project.assets()).map((asset) => asset.name);
      await carry(page, await toolRow(page, 'LLM', tool.id), await canvasPoint(page, name));
      let made;
      await expect.poll(async () => (made = (await project.assets()).find((asset) => !was.includes(asset.name)))?.name ?? null, { message: `the LLM on ${name} makes a box` }).not.toBeNull();
      expect.soft(made, 'a box of words with the LLM on it').toMatchObject({ kind: 'text', tool: tool.id });
      expect.soft(made.fields?.instructions, `its prompt starts with the ${kind}`).toBe(`<${name}>`);
      const rows = await rowsOf(project, [name, made.name]);
      expect.soft(rows[made.name].x > rows[name].x && Math.abs(rows[made.name].y - rows[name].y) <= 1, `right of it: ${JSON.stringify(rows[made.name])} from ${JSON.stringify(rows[name])}`).toBe(true);
      const words = page.locator(`.card.here[data-asset=${JSON.stringify(made.name)}] .in-frame .mention-text[data-field="instructions"]`);
      await expect(words, 'opened on its words').toBeFocused();
      await page.keyboard.type(' write a caption');
      await expect.poll(async () => (await project.asset(made.name))?.fields?.instructions, { message: "what's typed is after it" }).toBe(`<${name}> write a caption`);
      await page.keyboard.press('ControlOrMeta+Enter');
      await lands(made.name, 1);
      const [version] = (await project.asset(made.name)).prefixes;
      const digest = String((await project.asset(name)).sha256 ?? '').replace(/^sha256:/, '');
      kept.boxes[made.name] = [{ reads: name, kind, version: version.number, field: version.main?.field ?? null, text: version.main?.text ?? '', ...(await runOf(version.run, digest)) }];
      keep();
      const [one] = kept.boxes[made.name];
      expect(one.field, 'a version of words').toBe('text');
      expect.soft(one.carries, `its run's input carries the ${kind} (${digest})`).toBe(true);
      expect.soft(one.machine, `it ran on ${machine}`).toBe(machine);
      expect.soft(one.text, `its answer describes the ${kind}`).toMatch(sees[kind]);
      await page.keyboard.press('Escape');
      await canvasIdle(page);
      await shoot(`beside-${kind}`);
    });
  }
});
