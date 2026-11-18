// steady-face (cl-1197; Paul, 11-18: "when I push like 'run this' and the weights are already resident, it'll do something like
// 'downloading weights... installing...' but those are just phases it's skipping"): a box's face says a phase only once it has lasted
// (RunFace's STEADY_MS, 400 ms, as a spinner is held back). Every frame the board draws a box's face is kept, and three renders are judged:
//   cold   a fresh copy of weighted.py, never installed here: whatever of its install and BiRefNet's download its face shows (its machine
//          may do both ahead, as the box is added: 44239475329's run began with every byte here and its face showed neither), no face
//          gives way to another within 400 ms;
//   warm   the same again: no install or download on any frame, and nothing flashes by;
//   blink  three stages in a row that each last 200 ms (project/blink.py) never show, whatever the other two's phases took that day,
//          and the 3 s of work after them does: a phase that lasts is still said.
// An end shows at once. Each render judged must have made a run: a plain `cozy render`, which makes a new take of a box whose tool
// takes a seed and that names none (weighted.py and blink.py both; cl-1192 took `--new` away). The timelines, each run's events and a
// picture of each are kept: $OUT/web/steady-face/.
const { copyFileSync, mkdirSync, readFileSync, writeFileSync } = require('fs');
const { join } = require('path');
const { test, expect, canvasVisible, fixtureTool } = require('./fixtures');
const { fit } = require('./hands');

const BOX = 'steady';
const SKIPPABLE = ['install', 'download'];
const ENDS = ['result', 'failed', 'canceling'];
/** RunFace's STEADY_MS. */
const STEADY_MS = 400;

/** Each face that showed and gave way to another sooner than STEADY_MS: what flashed by. A face is its phase and words (its count
 *  moving is the same face). It's read once a frame, and a runner's frame is up to 100 ms, so how long it showed is known to a frame:
 *  its longest, from the frame before it was first seen (`since`) to the frame the next was, is what's judged, with no allowance
 *  (44242245904 read a face held its 400 ms as 352 ms, against an allowance of 20). An end shows at once, over whatever was there, and
 *  the last before the box's face goes stays as long as the render did. */
function flashed(faces) {
  const shown = [];
  for (const one of faces) {
    const last = shown.at(-1);
    if (last && last.phase === one.phase && last.state === one.state) continue;
    shown.push(one);
  }
  return shown.flatMap((one, i) => {
    const next = shown[i + 1];
    if (!one.phase || !next?.phase || ENDS.includes(one.phase) || ENDS.includes(next.phase) || next.at - one.since >= STEADY_MS) return [];
    return [{ phase: one.phase, state: one.state, ms: next.at - one.since }];
  });
}

const BLINK = 'blink';

/** What the board's face of `name` says, frame by frame, from now until `stop` is called: each change, with its time (`at`) and the
 *  time of the frame before, when it wasn't yet so (`since`). */
const record = (page, name) => page.evaluate((name) => {
  const seen = (window.__faces = []);
  let last = '';
  let before = Math.round(performance.now());
  const tick = () => {
    const at = Math.round(performance.now());
    const progress = document.querySelector('.canvas-root')?.canvasInspection?.card(name)?.rendered?.progress;
    const now = progress ? `${progress.phase}|${progress.state}|${progress.count ?? ''}` : 'none';
    if (now !== last) {
      seen.push({ at, since: before, phase: progress?.phase ?? null, state: progress?.state ?? null, count: progress?.count ?? null });
      last = now;
    }
    before = at;
    window.__facesId = requestAnimationFrame(tick);
  };
  tick();
}, name);
const stop = (page) => page.evaluate(() => {
  cancelAnimationFrame(window.__facesId);
  return window.__faces;
});

test('steady-face', async ({ page, project, cli }) => {
  await cli.json('machine', 'use', 'local');
  const shots = join(process.env.OUT, 'web', 'steady-face');
  mkdirSync(shots, { recursive: true });
  // A fresh copy: a package this machine has never installed.
  const dir = test.info().outputPath('steady');
  mkdirSync(dir, { recursive: true });
  const tool = join(dir, 'weighted.py');
  copyFileSync(join(__dirname, '..', 'project', 'weighted.py'), tool);
  writeFileSync(tool, `${readFileSync(tool, 'utf8')}\n# steady-face ${Date.now()}\n`);
  await project.add({ name: BOX, tool: `${tool}/weighted`, kind: 'image', fields: { instructions: 'steady', seconds: 3 }, at: { x: 0, y: 0 } });
  await expect.poll(() => canvasVisible(page, BOX)).toBe(true);
  await fit(page);

  /** One render of `box`: what its face said, frame by frame, its run's events, and a picture once its face says one of `when`. */
  const render = async (label, when, more = [], box = BOX) => {
    await record(page, box);
    const said = cli('--json', 'render', box, '--on', 'local', ...more);
    // A picture while it shows one of `when`, if it does.
    const pictured = expect.poll(() => page.evaluate((when) => when.includes(window.__faces.at(-1)?.phase), when), { timeout: 120_000, intervals: [50] })
      .toBe(true).then(() => page.screenshot({ path: join(shots, `${label}-${when.join('-')}.png`) }), () => null);
    const done = await said;
    // With --json a refusal is its envelope on stdout.
    expect(done.code, `${label}: cozy render: ${(done.stderr || done.stdout).slice(-400)}`).toBe(0);
    await pictured;
    const faces = await stop(page);
    const run = done.stdout.split('\n').filter((line) => line.startsWith('{')).map((line) => JSON.parse(line)).find((one) => one.type === 'take')?.take?.run;
    // A render answered from its box's take makes no run and shows no face: nothing of it would be judged.
    expect.soft(run, `${label}: its render made a run: ${done.stdout.slice(-300)}`).toBeTruthy();
    const events = run ? (await cli('--json', 'run', 'watch', String(run), '--full')).stdout : '';
    writeFileSync(join(shots, `${label}-faces.json`), JSON.stringify(faces, null, 2));
    writeFileSync(join(shots, `${label}-run-${run}.ndjson`), events);
    test.info().annotations.push({ type: label, description: JSON.stringify(faces.map((one) => `${one.phase}: ${one.state ?? ''}${one.count ? ` ${one.count}` : ''}`)) });
    return faces;
  };

  await test.step('cold: whatever of its install and download shows, nothing flashes by', async () => {
    // Pictured at its install or download if its face says one, else at its own work.
    const faces = await render('cold', [...SKIPPABLE, 'work']);
    test.info().annotations.push({ type: 'cold showed', description: JSON.stringify([...new Set(faces.map((one) => one.phase).filter((phase) => SKIPPABLE.includes(phase)))]) });
    expect.soft(flashed(faces), `nothing flashed by: ${JSON.stringify(faces)}`).toEqual([]);
  });

  await test.step('warm: straight to its own work, no install or download on any frame', async () => {
    const faces = await render('warm', ['work']);
    expect.soft(faces.filter((one) => SKIPPABLE.includes(one.phase)), `the warm render's faces: ${JSON.stringify(faces)}`).toEqual([]);
    expect.soft(flashed(faces), `nothing flashed by: ${JSON.stringify(faces)}`).toEqual([]);
    expect.soft(faces.some((one) => one.phase === 'work'), 'and its own work shows').toBe(true);
  });

  await test.step('three stages of 200 ms each never show; the work after them does', async () => {
    await project.add({ name: BLINK, tool: fixtureTool('blink.py/blink'), kind: 'image', fields: { instructions: 'blink', blink: 0.2, seconds: 3 }, at: { x: 420, y: 0 } });
    await expect.poll(() => canvasVisible(page, BLINK)).toBe(true);
    await fit(page);
    // Once, so its environment is here: the render judged is warm, its 200 ms stages the only short ones.
    await render('blink-first', ['work'], [], BLINK);
    const faces = await render('blink', ['work'], [], BLINK);
    expect.soft(faces.filter((one) => /blink/i.test(one.state ?? '')), `its blinks never show: ${JSON.stringify(faces)}`).toEqual([]);
    expect.soft(faces.some((one) => /drawing/i.test(one.state ?? '')), `its own work shows: ${JSON.stringify(faces)}`).toBe(true);
    expect.soft(flashed(faces), `nothing flashed by: ${JSON.stringify(faces)}`).toEqual([]);
  });
});
