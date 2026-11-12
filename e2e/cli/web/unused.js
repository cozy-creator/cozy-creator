// A mention its tool can't use is struck through, saying why, and never sent (cl-944, Paul 11-12: "It should cross out that reference, to
// clearly indicate that it's not using its input"), in a real browser on a real Calcifer with the Hub's real MOSS Image, Qwen-Image-2 and H3.
// A project with a picture, a video and a sound, and a picture box with a version:
//   MOSS Image (words only): <pic>, <clip> and <photo.1> are struck through, each saying why;
//   Qwen-Image-2 (pictures): <pic> and <photo.1> are not; <clip:0-5> and <hum> are;
//   H3's Shot (pictures, videos and sounds): nothing is.
// Calcifer says each with the box (`unused`) and once in the project's notes, which `cozy status` shows; a run of a stand-in picture tool
// leaves a video it can't use out of its prompt and `cozy run watch` says so once, as a note. Nothing is refused and nothing pops up.
//   node unused.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER; $COZY: the cozy under test.
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots. Prints what it saw as JSON and exits 1
// if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { spawnSync } = require('child_process');
const { mkdtempSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { projectFromPage, calcifer, signIn, MP4, STEP_MS, boxOf, fitCanvas, lastLook, launch, lookAt, noStall, openBox, png, settled, until, upload, wav } = require('./page');

const failures = [];
const seen = {};
noStall(seen, died);
let during = 'the start';
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  during = name;
  if (!ok) failures.push(name);
}
async function died(why) {
  failures.push(`${during}: ${why}`);
  await lastLook();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(1);
}

/** The cozy under test in `folder`: its exit code, stdout and stderr. */
const cozy = (folder, words) => {
  const ran = spawnSync(process.env.COZY, words, { cwd: folder, encoding: 'utf8', timeout: STEP_MS * 4 });
  return { code: ran.status, out: ran.stdout ?? '', err: ran.stderr ?? '' };
};

(async () => {
  const base = calcifer();
  const browser = await launch(playwright);
  const context = await browser.newContext({ viewport: { width: 1600, height: 1100 } });
  const page = await context.newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
  page.on('console', (m) => { if (m.type() === 'error' && !/status of 4\d\d|net::ERR_/.test(m.text())) errors.push(`console: ${m.text()}`); });
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());

  await signIn(page);
  await page.waitForSelector('.top');
  const held = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session') ?? 'null'));
  const api = (path, init = {}) => page.evaluate(async ({ path, init, token }) => {
    const answer = await fetch(path, { ...init, headers: { ...(token ? { Authorization: `Bearer ${token}` } : {}), 'Content-Type': 'application/json' } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  }, { path, init, token: held?.token });
  const post = async (path, body) => {
    const answer = await api(path, { method: 'POST', body: JSON.stringify(body) });
    if (answer.status >= 300) throw new Error(`${path}: ${JSON.stringify(answer)}`);
    return answer.body;
  };

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-unused-${Date.now()}`);
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const at = `/v1/projects/${id}`;
  const project = (await api(at)).body.path;
  seen.project = { id, folder: project };
  const files = mkdtempSync(join(tmpdir(), 'web-unused-'));
  writeFileSync(join(files, 'pic.png'), png(80));
  writeFileSync(join(files, 'photo.png'), png(150));
  writeFileSync(join(files, 'clip.mp4'), MP4);
  writeFileSync(join(files, 'hum.wav'), wav());

  // The Hub's own tools: MOSS Image takes words only, Qwen-Image-2 pictures, H3's Shot pictures, videos and sounds.
  const listed = async (q) => (await api(`/v1/tools?project=${id}&q=${q}`)).body?.tools ?? [];
  const moss = (await listed('moss')).find((tool) => tool.package === 'moss-image' && tool.function === 'image');
  const qwen = (await listed('qwen')).find((tool) => tool.package === 'qwen-image-2' && tool.function === 'image');
  const h3 = (await listed('minimax')).find((tool) => tool.id === 'minimax-h3/generate') ?? (await listed('minimax')).find((tool) => tool.package === 'minimax-h3' && tool.makes === 'video');
  if (!moss || !qwen || !h3) throw new Error(`the Hub lists no MOSS Image, Qwen-Image-2 or H3 Shot: ${JSON.stringify({ moss: moss?.id, qwen: qwen?.id, h3: h3?.id })}`);
  seen.tools = { moss: moss.id, qwen: qwen.id, h3: h3.id };

  for (const [name, file, x] of [['pic', 'pic.png', 100], ['clip', 'clip.mp4', 460], ['hum', 'hum.wav', 820]]) {
    await post(`${at}/imports`, { source: await upload(held?.token, join(files, file)), name, exact: true, at: { x, y: 80 } });
  }
  await post(`${at}/assets`, { tool: qwen.id, kind: 'image', name: 'photo', exact: true, fields: { instructions: 'a harbour' }, at: { x: 1180, y: 80 } });
  await post(`${at}/assets/photo/versions`, { source: await upload(held?.token, join(files, 'photo.png')) });
  const written = {
    moss: { tool: moss.id, kind: 'image', fields: { instructions: '<pic> is in a pose, beside <clip>, as in <photo.1>' }, x: 100 },
    qwen: { tool: qwen.id, kind: 'image', fields: { instructions: '<pic> by the sea as in <photo.1>, with <clip:0-5> and <hum>' }, x: 560 },
    shot: { tool: h3.id, kind: 'video', fields: { summary: '<clip> then <pic>, and <hum> plays', beats: [{ description: '<pic> waves.', duration: 5 }] }, x: 1020 },
  };
  for (const [name, box] of Object.entries(written)) {
    await post(`${at}/assets`, { tool: box.tool, kind: box.kind, name, exact: true, fields: box.fields, at: { x: box.x, y: 520 } });
  }

  // Calcifer's word, once it has read each package: what each box leaves out, where, and why.
  const unusedOf = async () => Object.fromEntries(((await api(at)).body?.assets ?? []).map((asset) => [asset.name, asset.unused ?? []]));
  const read = await until('each box read by its package', unusedOf, (now) => now.moss?.length === 3 && now.qwen?.length === 2, STEP_MS * 2).catch(unusedOf);
  seen.unused = read;
  const one = (name, mention) => read[name]?.find((unused) => unused.mention === mention);
  check('moss_leaves_out_a_picture', /^Moss Image can't use images$/.test(one('moss', 'pic')?.why ?? '') && one('moss', 'pic')?.at === 'instructions', read.moss);
  check('moss_leaves_out_a_video', /can't use videos$/.test(one('moss', 'clip')?.why ?? ''), read.moss);
  check('moss_leaves_out_a_version', /can't use images$/.test(one('moss', 'photo.1')?.why ?? ''), read.moss);
  check('qwen_uses_pictures', !one('qwen', 'pic') && !one('qwen', 'photo.1'), read.qwen);
  check('qwen_leaves_out_a_range_of_a_video', /^Qwen Image 2 can't use videos$/.test(one('qwen', 'clip:0-5')?.why ?? ''), read.qwen);
  check('qwen_leaves_out_a_sound', /can't use audio$/.test(one('qwen', 'hum')?.why ?? ''), read.qwen);
  check('h3_uses_all_three', (read.shot ?? []).length === 0, read.shot);
  const notes = (await api(at)).body?.notes ?? [];
  seen.notes = notes;
  check('the_project_notes_each_once', notes.length === 5 && notes.every((note) => note.message.includes('is left out:')), notes);

  // The call its binder would send: what's left out is its name in the text, and no reference.
  const call = async (name) => (await api(`${at}/assets/${name}?call=true`)).body;
  const mossCall = await call('moss');
  seen.moss_call = { call: mossCall?.call, said: mossCall?.call_warnings };
  check('moss_sends_only_words', typeof mossCall?.call?.instructions === 'string' && !mossCall.call.instructions.includes('<'), seen.moss_call);
  const qwenCall = await call('qwen');
  seen.qwen_call = { call: qwenCall?.call?.instructions, said: qwenCall?.call_warnings };
  const keys = (qwenCall?.call?.instructions?.references ?? []).map((reference) => reference.key);
  check('qwen_sends_only_pictures', JSON.stringify(keys) === '["pic","photo.1"]' && !/<clip|<hum/.test(qwenCall?.call?.instructions?.text ?? '<'), seen.qwen_call);

  // The page: each crossed out where it's written, at rest and open, its title why; nothing else.
  for (const name of ['moss', 'qwen', 'shot']) await boxOf(page, name).waitFor();
  await fitCanvas(page);
  await settled(page);
  const chips = (name) => boxOf(page, name).evaluate((node) => [...node.querySelectorAll('.card .words .chip')].map((chip) => ({ text: chip.textContent, unused: chip.classList.contains('unused'), struck: getComputedStyle(chip).textDecorationLine.includes('line-through'), title: chip.title })));
  const rest = {};
  for (const name of ['moss', 'qwen', 'shot']) rest[name] = await until(`${name}'s words`, () => chips(name), (now) => now.length > 0 && (name === 'shot' || now.some((chip) => chip.unused))).catch(() => chips(name));
  seen.at_rest = rest;
  check('at_rest_moss_crosses_out_all_three', rest.moss.length === 3 && rest.moss.every((chip) => chip.unused && chip.struck && /can't use/.test(chip.title)), rest.moss);
  check('at_rest_qwen_crosses_out_the_video_and_sound_only', JSON.stringify(rest.qwen.map((chip) => chip.unused)) === '[false,false,true,true]', rest.qwen);
  check('at_rest_h3_crosses_out_nothing', rest.shot.length >= 3 && rest.shot.every((chip) => !chip.unused && !chip.struck), rest.shot);
  await shot('at-rest');

  await openBox(page, 'moss');
  const editor = page.locator('.card.here .mention-text[data-field=instructions]');
  await editor.waitFor();
  const open = await until('moss open, crossed out', () => editor.evaluate((node) => [...node.querySelectorAll('.chip')].map((chip) => ({ mention: chip.dataset.mention, unused: chip.classList.contains('unused'), title: chip.title }))), (now) => now.length === 3 && now.every((chip) => chip.unused));
  seen.open = open;
  check('open_moss_crosses_out_each_saying_why', open.every((chip) => /^Moss Image can't use (images|videos)$/.test(chip.title)), open);
  check('nothing_pops_up', (await page.locator('[role=dialog], [role=alertdialog], .modal').count()) === 0);
  await shot('moss-open');

  // Typed in: a sound in MOSS's words is crossed out once Calcifer reads it; nothing is refused.
  await editor.click();
  await page.keyboard.press('End');
  await page.keyboard.type(' and <hum> ');
  const typed = await until('a typed sound crossed out', () => editor.evaluate((node) => [...node.querySelectorAll('.chip')].map((chip) => ({ mention: chip.dataset.mention, unused: chip.classList.contains('unused') }))), (now) => now.some((chip) => chip.mention === 'hum' && chip.unused));
  seen.typed = typed;
  check('typed_mention_crossed_out', typed.some((chip) => chip.mention === 'hum' && chip.unused));
  await page.keyboard.press('Escape');

  // `cozy status`: each once, as a note; the project stays ready for what it is.
  const status = cozy(project, ['--json', 'status']);
  const said = (() => { try { return JSON.parse(status.out); } catch { return null; } })();
  seen.status = { code: status.code, notes: said?.notes, health: said?.health, err: status.err.slice(-400) };
  check('status_notes_each_once', status.code === 0 && Array.isArray(said?.notes) && said.notes.filter((line) => line.startsWith('moss: <hum> is left out:')).length === 1 && said.notes.some((line) => line.startsWith('qwen: <clip:0-5> is left out:')), seen.status);
  check('status_notes_are_not_warnings', !(said?.warnings ?? []).some((warning) => /is left out/.test(warning.message ?? '')), said?.warnings);

  // A run of a stand-in picture tool: the video is its name in the prompt, and `cozy run watch` says it once, as a note.
  // Each run is read back from Calcifer once it ends, as the page reads it.
  const newest = async () => Math.max(0, ...((await api('/v1/runs?limit=10')).body?.runs ?? []).map((run) => run.number ?? 0));
  const ended = async (target, after) => {
    const ends = (run) => run && ['completed', 'failed', 'canceled'].includes(run.status);
    const found = await until(`a run of ${target} ended`, async () => (await api('/v1/runs?limit=10')).body?.runs?.find((run) => run.target?.includes(target) && run.number > after), ends, STEP_MS * 3).catch(() => null);
    return found ? (await api(`/v1/runs/${encodeURIComponent(found.id)}`)).body : null;
  };
  const still = join(__dirname, '..', 'project', 'tool.py') + '/still';
  const before = await newest();
  const ran = cozy(project, ['--json', 'run', still, 'instructions=<pic> beside <clip>', '--on', 'local']);
  const record = await ended('still', before);
  seen.run = { code: ran.code, status: record?.status, prompt: record?.result?.prompt, warnings: record?.warnings, err: ran.err.slice(-400) };
  check('a_run_sends_what_it_can_use', ran.code === 0 && record?.result?.prompt === '<pic> beside clip', seen.run);
  check('a_run_keeps_the_note', (record?.warnings ?? []).filter((warning) => warning.code === 'mention_unused' && warning.message.includes('<clip> is left out:')).length === 1, record?.warnings);
  const watch = record?.number ? cozy(project, ['run', 'watch', String(record.number)]) : { code: -1, out: '', err: 'no run' };
  const lines = `${watch.out}\n${watch.err}`.split('\n').filter((line) => line.includes('<clip> is left out:'));
  seen.watch = { code: watch.code, lines, out: watch.out.slice(-600) };
  check('watch_says_it_once_as_a_note', watch.code === 0 && lines.length === 1 && /note: /.test(lines[0]) && !/warning: /.test(lines[0]), seen.watch);

  // The Runtime's own rule, for what reaches a function past Calcifer's binder (an API caller's raw prompt): a video and a sound in a
  // picture prompt, and a picture where only words go, are left out with an `ignored` row, their mentions their names; never refused.
  const assets = (await api(at)).body?.assets ?? [];
  const file = (name) => { const sha = assets.find((asset) => asset.name === name)?.sha256 ?? ''; return sha.startsWith('sha256:') ? sha : `sha256:${sha}`; };
  const raw = async (target, field, prompt) => {
    const after = await newest();
    const ran = cozy(project, ['--json', 'run', target, `${field}:=${JSON.stringify(prompt)}`, '--on', 'local']);
    const record = await ended(target.split('/').at(-1), after);
    return { code: ran.code, status: record?.status, result: record?.result, ignored: (record?.adjustments ?? []).filter((row) => row.kind === 'ignored'), name: record ? String(record.number) : null, failure: record?.failure ?? record?.error, err: ran.err.slice(-400) };
  };
  const pictures = await raw(still, 'instructions', { text: '<V> and <H> at dusk', references: [{ key: 'V', type: 'video', file: file('clip') }, { key: 'H', type: 'audio', file: file('hum') }] });
  seen.runtime_pictures = pictures;
  check('runtime_leaves_a_video_and_a_sound_out_of_a_picture_prompt', pictures.code === 0 && pictures.result?.prompt === 'V and H at dusk', pictures);
  check('runtime_notes_each_once', pictures.ignored.length === 2 && /takes no videos/.test(pictures.ignored.find((row) => row.requested === 'V')?.reason ?? '') && /takes no audio/.test(pictures.ignored.find((row) => row.requested === 'H')?.reason ?? ''), pictures.ignored);
  const words = await raw(join(__dirname, '..', 'project', 'tool.py') + '/words', 'prompt', { text: 'make <A> red', references: [{ key: 'A', type: 'image', file: file('pic') }] });
  seen.runtime_words = words;
  check('runtime_leaves_a_picture_out_of_words', words.code === 0 && words.result?.text === 'make A red' && words.ignored.length === 1 && /takes no images/.test(words.ignored[0].reason), words);
  const shown = pictures.name ? cozy(project, ['run', 'watch', pictures.name]) : { code: -1, out: '' };
  const rows = shown.out.split('\n').filter((line) => /ignored instructions\.references\.\d: [VH] \(/.test(line));
  seen.runtime_watch = { code: shown.code, rows, out: shown.out.slice(-600) };
  check('runtime_watch_says_each_once', shown.code === 0 && rows.length === 2, seen.runtime_watch);

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch((why) => died(why.stack ?? why));
