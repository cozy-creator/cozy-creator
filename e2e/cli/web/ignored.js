// The Runtime's own rule (cl-944, Paul 11-12: "They won't throw an error, but the runtime will just ignore that input modality that the
// model cannot use"), for what reaches a function past Calcifer's binder: an API caller's raw prompt, sent as `cozy run … field:=<json>`.
// A picture, a video and a sound in a prompt of pictures: the picture is kept, the video and the sound are left out, each its name in the
// text and one `ignored` row; a picture where only words go: its name, one row. Nothing is refused, and `cozy run watch` says each once.
//   node ignored.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER; $COZY: the cozy under test.
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser. Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { spawnSync } = require('child_process');
const { mkdtempSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { projectFromPage, calcifer, signIn, MP4, STEP_MS, lastLook, launch, lookAt, noStall, png, until, upload, wav } = require('./page');

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

const cozy = (folder, words) => {
  const ran = spawnSync(process.env.COZY, words, { cwd: folder, encoding: 'utf8', timeout: STEP_MS * 5 });
  return { code: ran.status, out: ran.stdout ?? '', err: ran.stderr ?? '' };
};

(async () => {
  const base = calcifer();
  const browser = await launch(playwright);
  const page = await (await browser.newContext()).newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  await signIn(page);
  await page.waitForSelector('.top');
  const held = await page.evaluate(() => JSON.parse(sessionStorage.getItem('cozy.session') ?? 'null'));
  const api = (path, init = {}) => page.evaluate(async ({ path, init, token }) => {
    const answer = await fetch(path, { ...init, headers: { ...(token ? { Authorization: `Bearer ${token}`, 'cozy-project-storage': '3' } : {}), 'Content-Type': 'application/json' } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  }, { path, init, token: held?.token });

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-ignored-${Date.now()}`);
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const at = `/v1/projects/${id}`;
  const project = (await api(at)).body.path;
  seen.project = { id, folder: project };
  const files = mkdtempSync(join(tmpdir(), 'web-ignored-'));
  writeFileSync(join(files, 'pic.png'), png(80));
  writeFileSync(join(files, 'clip.mp4'), MP4);
  writeFileSync(join(files, 'hum.wav'), wav());
  for (const [name, file, x] of [['pic', 'pic.png', 100], ['clip', 'clip.mp4', 460], ['hum', 'hum.wav', 820]]) {
    const answer = await api(`${at}/imports`, { method: 'POST', body: JSON.stringify({ source: await upload(held?.token, join(files, file), id), name, exact: true, at: { x, y: 80 } }) });
    if (answer.status >= 300) throw new Error(`import ${name}: ${JSON.stringify(answer)}`);
  }
  const assets = await until('three files', async () => (await api(at)).body?.assets ?? [], (now) => ['pic', 'clip', 'hum'].every((name) => now.find((asset) => asset.name === name)?.sha256));
  const file = (name) => { const sha = assets.find((asset) => asset.name === name).sha256; return sha.startsWith('sha256:') ? sha : `sha256:${sha}`; };

  const raw = join(__dirname, '..', 'project', 'raw.py');
  const newest = async () => Math.max(0, ...((await api(`${at}/runs?limit=10`)).body?.runs ?? []).map((run) => run.number ?? 0));
  const run = async (fn, field, prompt) => {
    const after = await newest();
    const ran = cozy(project, ['--json', 'run', `${raw}/${fn}`, `${field}:=${JSON.stringify(prompt)}`, '--on', 'local']);
    const ends = (one) => one && ['completed', 'failed', 'canceled'].includes(one.status);
    const found = await until(`${fn} ended`, async () => (await api(`${at}/runs?limit=10`)).body?.runs?.find((one) => one.target?.endsWith(`/${fn}`) && one.number > after), ends, STEP_MS * 5).catch(() => null);
    const record = found ? (await api(`${at}/runs/${encodeURIComponent(found.id)}`)).body : null;
    return { code: ran.code, status: record?.status, result: record?.result, ignored: (record?.adjustments ?? []).filter((row) => row.kind === 'ignored'), number: record?.number, reason: record?.reason?.message, err: ran.err.slice(-600) };
  };

  // A picture, a video and a sound into a prompt of pictures.
  const pictures = await run('pictures', 'instructions', { text: '<A> by <V> with <H>', references: [{ key: 'A', type: 'image', file: file('pic') }, { key: 'V', type: 'video', file: file('clip') }, { key: 'H', type: 'audio', file: file('hum') }] });
  seen.pictures = pictures;
  check('a_picture_prompt_runs', pictures.code === 0 && pictures.status === 'completed', pictures);
  check('it_keeps_the_picture', JSON.stringify(pictures.result?.kept) === '["A"]', pictures.result);
  check('the_video_and_sound_are_their_names', pictures.result?.text === '<A> by V with H', pictures.result);
  const row = (key) => pictures.ignored.find((one) => one.requested === key);
  check('one_row_each', pictures.ignored.length === 2 && /takes no videos/.test(row('V')?.reason ?? '') && /takes no audio/.test(row('H')?.reason ?? '') && row('V')?.field === 'instructions.references.1', pictures.ignored);

  // A picture where only words go.
  const words = await run('words', 'prompt', { text: 'make <A> red', references: [{ key: 'A', type: 'image', file: file('pic') }] });
  seen.words = words;
  check('words_run', words.code === 0 && words.status === 'completed', words);
  check('words_take_its_name', words.result?.text === 'make A red', words.result);
  check('words_say_it_once', words.ignored.length === 1 && words.ignored[0].requested === 'A' && /takes no images/.test(words.ignored[0].reason), words.ignored);

  // `cozy run watch`: each once, as what was left out, never a failure.
  const watch = pictures.number ? cozy(project, ['run', 'watch', String(pictures.number)]) : { code: -1, out: '' };
  const lines = watch.out.split('\n').filter((line) => /^\s*ignored instructions\.references\.[12]: [VH] \(/.test(line));
  seen.watch = { code: watch.code, lines, out: watch.out.slice(-800) };
  check('watch_says_each_once', watch.code === 0 && lines.length === 2, seen.watch);

  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch((why) => died(why.stack ?? why));
