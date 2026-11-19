// shared-install (cl-1218), the coordinator's ask after shared-download (cl-1205), on Paul's ruling that packages and models are the
// machine's and projects isolate only files and runs. One package installing for two projects on one machine is one install, listed once:
// an encrypted project's render builds e2e-slow-install's environment on this computer (its wheel comes slowly), and this cell's plain
// project, shown by the page, renders a box of the same tool meanwhile. While it builds: read with no project, the machine lists one
// install of the package, counting every job waiting on it and naming none, and so does `cozy machine show` from outside both; each
// project's run is one of that install's jobs, the one that builds it saying the install's own stage and one that waits naming it
// (`waiting_on`) with no stage of its own (whichever job the environment's lock went to builds: a run, or a guess, cl-1104); the plain
// project's page lists the install once, for its own run and the rest counted, its run's row a link to it and its box "Waiting for
// install", leading there. Nothing of the encrypted project's (its box, its run's name or id) is in the install's listing, the CLI's
// line or the plain project's page. Once both wait, the wheel is never asked for again: one build, as TensorD's install turns keep it
// (#3777, #3784). A later render, the package here, lists no install and waits on none. Screenshots of the canvas, the tab's list and
// the drawer at 1440×900, paper and dark, are taken as soon as the plain project's install run waits, before anything is judged, so a
// red run keeps them too.
// On the CPU e2e alone (web-shared-install, with the project-encryption driver): isolated.sh publishes e2e-slow-install for it, and
// wheel.py sends its 96 MiB wheel at RATE.
const { execFile, spawn } = require('child_process');
const { appendFileSync, mkdirSync, openSync, readFileSync, rmSync } = require('fs');
const { randomBytes } = require('crypto');
const { join } = require('path');
const { test, expect, themed, canvasIdle } = require('./fixtures');

const MACHINE = 'local';
const PACKAGE = 'e2e-slow-install';
const TOOL = `${PACKAGE}/still`;
const WHEEL = 'http://127.0.0.1:47104/e2e_slow_wheel-0.0.0-py3-none-any.whl';
/** Bytes a second: its 96 MiB take about three minutes. */
const RATE = 600_000;
const MINE = 'harbor';
const THEIRS = 'ledger_of_ash';
const AGAIN = 'harbor_again';
const softly = expect.configure({ soft: true });
const escape = (text) => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

/** An activity id as the page anchors its row (activity.ts `anchorOf`). */
const anchorOf = (id) => id.replace(/[^\w-]+/g, '-');
/** A package as the page says it (activity.ts `packageWords`): the Hub's own without its `_/`. */
const packageWords = (subject) => subject.replace(/^_\//, '').replace(/^local\//, '');
/** Whether `subject` is the package, under whichever org its Hub publishes it. */
const ours = (subject) => subject === PACKAGE || subject.endsWith(`/${PACKAGE}`);

/** `cozy <words>` in `dir`, as a person's shell runs it there: {code, stdout, stderr}. */
const cozyIn = (dir, ...words) => new Promise((done) => {
  if (process.env.ARGV_LOG) appendFileSync(process.env.ARGV_LOG, `${words.map((word) => `${word}\x1f`).join('')}\x1e`);
  execFile(process.env.COZY, words, { cwd: dir, maxBuffer: 1 << 26, timeout: 600_000 }, (error, stdout, stderr) => done({ code: error ? error.code ?? 1 : 0, stdout, stderr }));
});

/** The box's run line on the canvas: its words, its count and whether its words lead to what it waits on (presentation `leads`). */
const face = (page, name) => page.evaluate((box) => {
  const shown = document.querySelector('.canvas-root')?.canvasInspection?.card(box)?.rendered;
  return shown?.progress ? { words: (shown.progress.state ?? '').replace(/\s+/g, ' '), count: shown.progress.count ?? null, leads: Boolean(shown.progress.leads) } : null;
}, name);

test('shared-install', { tag: '@vault' }, async ({ page, project, api, cli, removeAfter }) => {
  test.skip(!(process.platform === 'linux' && process.env.GITHUB_ACTIONS === 'true'), "it installs a package on this computer's machine, cold, on a fresh GitHub runner");
  test.setTimeout(1_200_000);
  const shoot = async (label, region) => {
    for (const theme of ['paper', 'dark']) {
      await page.evaluate((dark) => {
        if (dark) document.documentElement.dataset.theme = 'dark';
        else delete document.documentElement.dataset.theme;
        return new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      }, theme === 'dark');
      await themed(page, theme);
      await page.screenshot({ path: test.info().outputPath(`${label}-1440x900-${theme}.png`) });
      if (region) await region.screenshot({ path: test.info().outputPath(`${label}-1440x900-${theme}-region.png`) });
    }
    await page.evaluate(() => { delete document.documentElement.dataset.theme; });
  };
  const outside = join(process.env.OUT, 'web');
  const machine = async () => api.get(`/v1/machines/${encodeURIComponent(MACHINE)}`);
  const installs = async () => ((await machine())?.status?.activity ?? []).filter((op) => op.kind === 'install' && ours(op.subject));
  const tab = () => page.locator(`nav.mtabs .mtab[data-machine="${MACHINE}"]`);
  const list = () => page.getByRole('dialog', { name: /is doing$/ });

  // The encrypted project, open: its box, its render, its run.
  const folder = join(outside, 'projects', `web-shared-install-vault-${randomBytes(4).toString('hex')}`);
  removeAfter(folder);
  const vault = await api.post('/v1/projects', { path: folder, passphrase: randomBytes(18).toString('base64url') });
  const vaultAt = (rest = '') => `/v1/projects/${encodeURIComponent(vault.id)}${rest}`;
  const runsOf = async (at) => (await api.get(at('/runs?limit=100'))).runs;
  /** The project's run that builds the package's environment or waits on it: one of the package whose progress or wait says install. */
  const installRun = async (at) => (await runsOf(at)).find((run) => run.target.includes(PACKAGE)
    && (run.progress?.waiting_on?.length || /install/i.test(`${run.waiting ?? ''} ${run.progress?.stage ?? ''}`)));

  // Its wheel, sent slowly, and each time it is asked for.
  const served = join(outside, 'shared-install-wheel');
  mkdirSync(served, { recursive: true });
  const log = join(outside, 'shared-install-wheel.log');
  const out = openSync(log, 'w');
  const server = spawn('python3', [join(__dirname, '..', 'project', 'slowinstall', 'wheel.py'), served, String(RATE)], { stdio: ['ignore', out, out] });
  const asked = () => (readFileSync(log, 'utf8').match(/"GET \/e2e_slow_wheel/g) ?? []).length;
  try {
    await expect.poll(() => fetch(WHEEL, { method: 'HEAD' }).then((answer) => answer.ok, () => false), { message: "the wheel's server answers", timeout: 60_000 }).toBe(true);
    await page.setViewportSize({ width: 1440, height: 900 });

    await api.post(vaultAt('/assets'), { name: THEIRS, tool: TOOL, exact: true, kind: 'image', fields: { instructions: 'A ledger, its pages burnt at the edges.' }, at: { x: 0, y: 0 } });
    const theirs = cozyIn(folder, '--json', 'render', THEIRS, '--on', MACHINE);
    await expect.poll(asked, { message: `${MACHINE} builds ${PACKAGE}'s environment for the encrypted project: its wheel asked for`, timeout: 600_000, intervals: [250] }).toBeGreaterThan(0);

    // This project's box of the same tool, rendered while that environment builds.
    await project.add({ name: MINE, tool: TOOL, kind: 'image', fields: { instructions: 'A harbor at first light.' }, at: { x: 0, y: 0 } });
    await page.goto(project.url());
    await expect(page.locator('.stage')).toBeVisible();
    await canvasIdle(page);
    const rendering = cli('--json', 'render', MINE, '--on', MACHINE);

    let mine;
    await test.step('as the package installs: the canvas, the machine tab and the drawer, paper and dark', async () => {
      // As its run says it, whichever way: the install it waits on, or (before cl-1218) words of whose install it waits for.
      await expect.poll(async () => {
        mine = await installRun(project.at.bind(project));
        return Boolean(mine);
      }, { message: `this project's install of ${PACKAGE} waits on ${MACHINE}`, timeout: 300_000, intervals: [250] }).toBe(true);
      await canvasIdle(page);
      await shoot('canvas');
      await tab().click();
      await expect(list()).toBeVisible();
      await shoot('machine-tab', list());
      await page.keyboard.press('Escape');
      await page.goto(project.url(`machines/${encodeURIComponent(MACHINE)}`));
      await expect(page.locator('ul.queue')).toBeVisible();
      // Shot once its run says what it waits on: a page just loaded lists the machine's install before its project's runs on it.
      await page.locator('ul.queue [data-queue="run"] a.said.waits').first().waitFor({ timeout: 15_000 }).catch(() => {});
      await shoot('drawer', page.locator('ul.queue'));
      await page.goto(project.url());
      await expect(page.locator('.stage')).toBeVisible();
      await canvasIdle(page);
    });
    // How often the wheel was asked for by now: a guess's build a run's install ended (cl-1123), and the one build both wait on.
    const askedThen = asked();

    // What is the encrypted project's: its box, and its run by name and id.
    const their = await installRun(vaultAt);
    expect(their, "the encrypted project's install run").toBeTruthy();
    const markers = { 'its box': new RegExp(escape(THEIRS)), "its run's id": new RegExp(escape(their.id)), "its run's name": new RegExp(`\\b${escape(their.name)}\\b`) };
    const leaks = (text) => Object.entries(markers).filter(([, re]) => re.test(text)).map(([name]) => name);

    let item;
    await test.step('read with no project, the machine lists one install of the package, counting every job and naming none', async () => {
      let listed = [];
      await softly.poll(async () => {
        listed = await installs();
        return listed.length;
      }, { message: `one install of ${PACKAGE} on ${MACHINE}, to a reader of no project`, timeout: 30_000, intervals: [250] }).toBe(1);
      item = listed[0];
      test.info().annotations.push({ type: 'install', description: JSON.stringify(listed) });
      softly(item?.names ?? [], 'it names no run to a reader of no project').toEqual([]);
      softly(item?.unnamed ?? 0, 'it counts both projects\' jobs').toBeGreaterThanOrEqual(2);
      softly(leaks(JSON.stringify(listed)), "it says nothing of the encrypted project's").toEqual([]);
    });

    await test.step('`cozy machine show` from outside both projects lists it once, for its jobs counted', async () => {
      const shown = await cozyIn(outside, 'machine', 'show', MACHINE);
      // Its own line begins with what it is; a run's begins with the run.
      const lines = shown.stdout.split('\n').filter((line) => line.trim().startsWith('install ') && line.includes(PACKAGE));
      test.info().annotations.push({ type: 'machine show', description: JSON.stringify(lines) });
      // Not judged here: its runs' lines, which name every open project's runs (cl-811), an encrypted one's too until it locks (cl-1130).
      test.info().annotations.push({ type: 'machine show, whole', description: JSON.stringify({ names: leaks(shown.stdout), code: shown.code }) });
      softly(lines.length, `one install line: ${shown.stdout.slice(0, 2000)}`).toBe(1);
      softly(lines.join('\n'), 'for its jobs, counted').toMatch(/for \d+ jobs/);
      softly(leaks(lines.join('\n')), "it says nothing of the encrypted project's").toEqual([]);
    });

    await test.step("each project's run is one of that install's jobs: the one that builds it says its stage, one that waits names it and says no stage of its own", async () => {
      const id = item?.id ?? `install ${PACKAGE}`;
      // On the install: it names it, or its own stage is the install's (it builds it).
      const on = async (at, run) => {
        const [now, listed] = [(await runsOf(at)).find((one) => one.id === run.id), (await installs())[0]];
        return { waits: (now?.progress?.waiting_on ?? []).includes(id), builds: Boolean(listed?.stage) && listed.stage === now?.progress?.stage };
      };
      const either = ({ waits, builds }) => waits || builds;
      await softly.poll(async () => either(await on(project.at.bind(project), mine)), { message: `${mine.name} is on the install`, timeout: 30_000, intervals: [250] }).toBe(true);
      await softly.poll(async () => either(await on(vaultAt, their)), { message: "the encrypted project's run is on the install", timeout: 30_000, intervals: [250] }).toBe(true);
      const [here, there] = [(await runsOf(project.at.bind(project))).find((run) => run.id === mine.id), (await runsOf(vaultAt)).find((run) => run.id === their.id)];
      mine = here ?? mine;
      test.info().annotations.push({ type: `run ${mine.name}`, description: JSON.stringify({ progress: here?.progress ?? null, waiting: here?.waiting ?? null }) });
      test.info().annotations.push({ type: "the encrypted project's run", description: JSON.stringify({ stage: there?.progress?.stage ?? null, waits: Boolean(there?.progress?.waiting_on?.length) }) });
      const waiting = [here, there].filter((run) => run?.progress?.waiting_on?.length);
      softly(waiting.length, 'at most one job builds it: a run of the two waits').toBeGreaterThanOrEqual(1);
      for (const run of waiting) {
        softly(run.progress.waiting_on, 'it names the one install').toEqual([id]);
        softly(run.progress.stage, 'it says it waits').toMatch(new RegExp(`waiting for ${escape(PACKAGE)} to install$`));
        softly(run.progress.done_fraction ?? null, 'it says no share of its own').toBeNull();
        softly(run.waiting ?? '', 'its wait names no run').not.toMatch(/\brun\b/i);
      }
      softly(leaks(JSON.stringify(here ?? {})), "this project's run says nothing of the encrypted project's").toEqual([]);
    });

    await test.step('its box says it waits on the install, and its words lead there', async () => {
      await softly.poll(() => face(page, MINE), { message: `${MINE} waits on the install`, timeout: 30_000 }).toEqual({ words: 'Waiting for install', count: null, leads: true });
    });

    await test.step("the machine's tab lists the install once, for this project's run and the rest counted, and its run waiting on it, a link to it", async () => {
      await tab().click();
      await expect(list()).toBeVisible();
      await softly(list().locator('[data-doing="install"]'), 'one install row').toHaveCount(1);
      const row = list().locator('[data-doing="install"] .said');
      await softly(row, 'it names this project\'s run').toContainText(mine?.name ?? 'no run');
      await softly(row, 'and counts the other project\'s').toContainText(/and \d+ jobs? more/);
      const waits = list().locator('[data-doing="run"] a.said.waits');
      await softly(waits, 'its run waits on it').toHaveCount(1);
      await softly(waits.first(), 'and says so').toHaveText(`Waiting for ${packageWords(item?.subject ?? PACKAGE)} to install`);
      if (await waits.count()) {
        await waits.first().click();
        await softly(list().locator('[data-doing="install"].lit'), 'its link lights the install').toHaveCount(1);
      }
      softly(leaks(await list().innerText()), "the list says nothing of the encrypted project's").toEqual([]);
      await page.keyboard.press('Escape');
    });

    await test.step("the machine's drawer the same, at the install's row a box's words lead to", async () => {
      await page.goto(project.url(`machines/${encodeURIComponent(MACHINE)}#${anchorOf(item?.id ?? `install ${PACKAGE}`)}`));
      const queue = page.locator('ul.queue');
      await expect(queue).toBeVisible();
      await softly(queue.locator('[data-queue="install"]'), 'one install row').toHaveCount(1);
      await softly(queue.locator('[data-queue="install"] .said'), 'for this project\'s run and the rest counted').toContainText(/and \d+ jobs? more/);
      await softly(queue.locator('[data-queue="run"] a.said.waits'), 'its run waits on it').toHaveCount(1);
      softly(leaks(await page.locator('body').innerText()), "the page says nothing of the encrypted project's").toEqual([]);
      await page.goto(project.url());
      await expect(page.locator('.stage')).toBeVisible();
    });

    await test.step('built once, both projects render', async () => {
      const [rendered, other] = [await rendering, await theirs];
      softly(rendered.code, `cozy render: ${rendered.stderr.slice(-400)}`).toBe(0);
      softly(other.code, `the encrypted project's cozy render: ${other.stderr.slice(-400)}`).toBe(0);
      await expect.poll(async () => [(await project.asset(MINE))?.versions ?? 0, (await api.get(vaultAt())).assets.find((row) => row.name === THEIRS)?.versions ?? 0], { message: 'both takes', timeout: 300_000 })
        .toEqual([1, 1]);
      test.info().annotations.push({ type: 'wheel', description: `asked for ${askedThen} times as both waited, ${asked()} by the end` });
      softly(asked(), 'no build of it began once both waited: its wheel was never asked for again').toBe(askedThen);
    });

    await test.step('a later render, the package here: no install listed, none waited on', async () => {
      await project.add({ name: AGAIN, tool: TOOL, kind: 'image', fields: { instructions: 'The harbor at noon.' }, at: { x: 380, y: 0 } });
      const before = new Set((await runsOf(project.at.bind(project))).map((run) => run.id));
      const again = cli('--json', 'render', AGAIN, '--on', MACHINE);
      const stages = new Set();
      let listed = 0, waited = 0;
      await expect.poll(async () => {
        for (const run of (await runsOf(project.at.bind(project))).filter((one) => !before.has(one.id))) {
          if (run.progress?.stage) stages.add(run.progress.stage);
          if (run.progress?.waiting_on?.length) waited += 1;
        }
        listed += (await installs()).length;
        return (await project.asset(AGAIN))?.versions ?? 0;
      }, { message: 'the later render', timeout: 300_000, intervals: [250] }).toBe(1);
      const said = await again;
      softly(said.code, `cozy render again: ${said.stderr.slice(-400)}`).toBe(0);
      test.info().annotations.push({ type: 'later render', description: JSON.stringify({ stages: [...stages], listed, waited, asked: asked() }) });
      softly(listed, `no install of ${PACKAGE} listed on ${MACHINE}`).toBe(0);
      softly(waited, 'it waited on no install').toBe(0);
      softly([...stages].filter((stage) => /install/i.test(stage)), 'no install stage').toEqual([]);
    });
  } finally {
    server.kill();
    rmSync(served, { recursive: true, force: true });
    // Locked, its folder can go.
    await new Promise((done) => execFile(process.env.COZY, ['--json', 'vault', 'lock'], { cwd: folder, timeout: 120_000 }, () => done()));
  }
});
