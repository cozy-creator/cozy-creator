// Calcifer closes a project nothing reads (cl-823): twenty projects of 1,000 assets each, opened and closed in one Calcifer by the CLI and the page.
// A project is open while a call on it, its stream, a page showing it or its render reads it, and the sign is its project.db: Calcifer has the
// file open only then. Each closes once the CLI's call has answered, and once the page shows another; Calcifer's heap is back within a tenth of
// where it was after two projects went the same way; a reopen is as quick as its first open; a render keeps its project open while it goes,
// waiting for its machine too, and it closes once that has landed.
//   node closes.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $COZY: the CLI; $ALONE: on a shared computer, so it runs nothing; $HOLDER:
// Calcifer's lock file, which names its process; $ARGV_LOG: where each cozy call is logged for coverage.
// Prints what it saw as JSON and exits 1 if anything failed.
const { chromium } = require(process.env.PLAYWRIGHT);
const { execFileSync } = require('child_process');
const { createHash } = require('crypto');
const { copyFileSync, linkSync, mkdirSync, readFileSync, rmSync, writeFileSync } = require('fs');
const { homedir } = require('os');
const { join } = require('path');
const { projectUrl, projectAction, newProject, calcifer, signIn, STEP_MS, lastLook, lookAt, png, noStall, until, projectHeld } = require('./page');
const { prepareCurrentSeed, seedCurrentProject } = require('./current-seed');
// Reuse one native producer, prepared before cold/start memory and timing samples.
prepareCurrentSeed();

const failures = [];
const seen = {};
noStall(seen, died);
let during = 'the start';
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  during = name;
  if (!ok) failures.push(name);
}

const PROJECTS = 20;
const ASSETS = 1000;
const PICTURES = 200;
/** How long a project that nothing reads has to close: its connection closes beside whatever let go of it. */
const CLOSE_MS = 10_000;
/** How long Calcifer is left alone before its memory is read: what a thread let go of, its allocator gives back. */
const SETTLE_MS = 15_000;
const BARS = {
  // Calcifer's heap back within a tenth of where it stood after two projects, twenty projects later: what a project costs open goes when it
  // closes. Without closing, twenty hold ~700 MB on top.
  memory_times_start: 1.1,
  // A reopen reads project.db as its first open did, the middle of twenty each, a runner's jitter aside.
  reopen_times_open: 1.25,
  reopen_slack_s: 0.05,
};
const COZY = process.env.COZY;
const TOOL = join(__dirname, '..', 'project', 'tool.py');
const WAITS = join(__dirname, '..', 'project', 'waits.py');
const HOLDER = JSON.parse(readFileSync(process.env.HOLDER, 'utf8'));
const sha = (bytes) => createHash('sha256').update(bytes).digest('hex');
const seconds = (ms) => +(ms / 1000).toFixed(3);
const median = (values) => [...values].sort((a, b) => a - b)[values.length >> 1];

/** Current native storage with the same pictures, two versions per shot and grid as web-scale. */
function generated(folder) {
  mkdirSync(join(folder, 'files'), { recursive: true });
  const seeds = { square: [16, 16], wide: [32, 18] };
  const seedOf = (shape) => join(folder, `seed-${shape}.png`);
  for (const [shape, [width, height]] of Object.entries(seeds)) writeFileSync(seedOf(shape), png(90, width, height));
  const measured = {};
  const file = (hex, shape) => {
    const at = join(folder, 'files', `${hex}.png`);
    try {
      linkSync(seedOf(shape), at);
    } catch (error) {
      // NTFS allows 1023 links to a file.
      if (error.code !== 'EEXIST') copyFileSync(seedOf(shape), at);
    }
    const [width, height] = seeds[shape];
    measured[`sha256:${hex}`] = { width, height };
  };
  const hash = (word) => sha(Buffer.from(`${folder} ${word}`));
  // Each box in order, its fields as stored: [[field, value], …].
  const boxes = [];
  const assets = {};
  const words = 'The colossal blue whale rises through a storm, lit from below, the sea torn white around it, and the small boat holds. '.repeat(3);
  for (let i = 0; i < PICTURES; i++) {
    const hex = hash(`picture ${i}`);
    file(hex, 'square');
    boxes.push({ name: `pic_${i}`, fields: [['approved', hex.slice(0, 12)]] });
  }
  for (let i = 0; i < ASSETS - PICTURES; i++) {
    const name = `shot_${i}`;
    boxes.push({ name, fields: [['tool', `${TOOL}/still`], ['instructions', `${words}<pic_${i % PICTURES}> and <pic_${(i * 7) % PICTURES}>.`], ['aspect_ratio', '16:9']] });
    const versions = [1, 2].map((number) => {
      const hex = hash(`${name} ${number}`);
      file(hex, 'wide');
      return { number, inputs: 'not yet', files: { image: `sha256:${hex}` }, seed: i * number, run: 1000 + i * 2 + number, execution_ms: 1234 };
    });
    assets[`shot ${i}`] = { name, kind: 'image', versions, numbered: 2 };
  }
  const side = Math.ceil(Math.sqrt(ASSETS));
  const cards = {};
  for (let i = 0; i < ASSETS - PICTURES; i++) cards[`shot_${i}`] = { x: (i % side) * 300, y: Math.floor(i / side) * 320, w: 260 };
  for (let i = 0; i < PICTURES; i++) cards[`pic_${i}`] = { x: ((ASSETS - PICTURES + i) % side) * 300, y: Math.floor((ASSETS - PICTURES + i) / side) * 320, w: 260 };
  const seeded = seedCurrentProject(folder, { boxes, assets, makes: { [`${TOOL}/still`]: 'image' }, measured, cards,
    expected_assets: ASSETS, expected_versions: (ASSETS - PICTURES) * 2 });
  for (const shape of Object.keys(seeds)) rmSync(seedOf(shape));
  return seeded;
}

/** Calcifer's resident memory, in MiB, as its OS says. */
function calciferMb() {
  if (process.platform === 'win32') {
    const line = execFileSync('tasklist', ['/FI', `PID eq ${HOLDER.pid}`, '/FO', 'CSV', '/NH'], { encoding: 'utf8' });
    return Math.round(Number(line.split('","')[4].replace(/[^0-9]/g, '')) / 1024);
  }
  return Math.round(Number(execFileSync('ps', ['-o', 'rss=', '-p', String(HOLDER.pid)], { encoding: 'utf8' }).trim()) / 1024);
}

/** Calcifer's memory in MiB. Its heap, what its status API reports: what a project open costs, and closing gives back. And as
 *  its OS says, resident, and on Linux what of that is its own (anonymous) and files mapped in (its code, read in as it's first used), and
 *  how many threads it has. Resident memory also counts what its allocator keeps of what was freed: on the same code, a minute after the
 *  last close, it stood 1.02–1.15× where it was after warm-up (cl-826), so it's kept for the record and judges nothing. */
async function memoryNow(api) {
  const calcifer = await api('/v1/status');
  const heap = Math.round((calcifer?.heap_bytes ?? NaN) / 2 ** 20 * 10) / 10;
  if (process.platform !== 'linux') return { heap, rss: calciferMb() };
  const status = readFileSync(`/proc/${HOLDER.pid}/status`, 'utf8');
  const field = (name) => Number(status.match(new RegExp(`^${name}:\\s+(\\d+)`, 'm'))?.[1] ?? NaN);
  const mb = (name) => Math.round(field(name) / 1024);
  return { heap, rss: mb('VmRSS'), anon: mb('RssAnon'), file: mb('RssFile'), threads: field('Threads') };
}

/** Whether Calcifer has the project in `folder` open: its project.db among the files it holds. Null where this can't be read (Windows). */
const held = (folder) => projectHeld(folder, HOLDER.pid);

(async () => {
  const base = calcifer();
  const api = (path, init = {}) => fetch(`${base}${path}`, { ...init, headers: { Authorization: `Bearer ${HOLDER.token}`, 'cozy-project-storage': '3', ...init.headers } }).then((answer) => answer.json());
  const status = async (folder) => {
    const answer = await projectAction(folder, 'status');
    return { state: answer.value, ms: answer.ms };
  };
  const readable = process.platform === 'linux' || process.platform === 'darwin';
  /** The project in `folder` closes within CLOSE_MS of `after`, and how long that took, else null. */
  const closes = async (folder) => {
    if (!readable) return 0;
    const from = Date.now();
    return until('closed', () => held(folder), (open) => open === false, CLOSE_MS).then(() => Date.now() - from, () => null);
  };
  const stamp = Date.now();
  const folders = Array.from({ length: PROJECTS }, (_, i) => join(homedir(), '.cozy', 'projects', `web-closes-${stamp}-${i}`));
  const ids = [];
  seen.project = { folders, ids };
  const times = { import_s: [], open_s: [], reopen_s: [], closed_ms: { cli: [], page: [] } };
  const memory = { trend: [], trend_rss: [] };
  seen.times = times;
  seen.memory_mb = memory;
  seen.bars = BARS;
  seen.fds_readable = readable;

  const browser = await chromium.launch({ executablePath: process.env.CHROME, headless: true });
  const context = await browser.newContext({ viewport: { width: 1600, height: 1000 } });
  const page = await context.newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
  // The page on what shows no project: the runs. What it held of earlier cells' projects goes.
  await signIn(page);
  await page.waitForSelector('.top');
  await page.goto(`${base}/runs`);
  await page.waitForTimeout(SETTLE_MS);
  memory.cold = await memoryNow(api);
  const notClosed = { cli: [], page: [] };
  const notHeld = [];

  /** Each project in `list` as the twenty go: imported and opened by the CLI, shown by the page, which then shows the next (so two are open a
   *  moment), and read again by the CLI. With `counted`, its times and what it holds after each are kept. */
  async function pass(list, counted) {
    /** Project `i`, which the page has stopped showing: closed, then read again by the CLI, which opens it as its first read did, and closed. */
    const reopened = async (i) => {
      const left = await closes(list[i]);
      if (counted) times.closed_ms.page.push(left);
      if (left === null) notClosed.page.push(list[i]);
      const again = await status(list[i]);
      if (counted) times.reopen_s.push(seconds(again.ms));
      if ((await closes(list[i])) === null) notClosed.cli.push(list[i]);
    };
    for (let i = 0; i < list.length; i++) {
      const folder = list[i];
      seen.looks = (seen.looks ?? 0) + 1;
      seen.native_seed = generated(folder);
      seen.native_seed_projects = (seen.native_seed_projects ?? 0) + 1;
      // Both reads open current project.db; each closes once it has answered.
      const imported = await status(folder);
      ids.push(imported.state.id);
      const afterImport = await closes(folder);
      const opened = await status(folder);
      const afterOpen = await closes(folder);
      if (counted) {
        times.import_s.push(seconds(imported.ms));
        times.open_s.push(seconds(opened.ms));
        times.closed_ms.cli.push(afterImport, afterOpen);
      }
      if (afterImport === null || afterOpen === null) notClosed.cli.push(folder);
      // The page shows it, and asks nothing more of it: it's open while it does. The one it showed before closes.
      await page.goto(await projectUrl(imported.state.id));
      await page.waitForSelector('.nm:text-is("shot_0")', { timeout: 120_000 });
      await page.waitForTimeout(1000);
      if (readable && !held(folder)) notHeld.push(folder);
      if (i > 0) {
        await reopened(i - 1);
        // Each with one project shown, the one before closed: what it holds after each, which levels off or grows with them.
        if (counted) {
          const now = await memoryNow(api);
          memory.trend.push(now.heap);
          memory.trend_rss.push(now.anon ?? now.rss);
        }
      }
    }
    await page.goto(`${base}/runs`);
    await reopened(list.length - 1);
    await page.waitForTimeout(SETTLE_MS);
  }

  // Two projects through all of it first, as the twenty go: what Calcifer takes on at its first use of all this (its code read in, its
  // allocator's own for two projects open at a time) is no project's. Its memory from there is the start.
  const warm = [0, 1].map((i) => join(homedir(), '.cozy', 'projects', `web-closes-${stamp}-warm-${i}`));
  folders.push(...warm);
  await pass(warm, false);
  memory.start = await memoryNow(api);
  await pass(folders.slice(0, PROJECTS), true);

  check('each_is_open_while_the_page_shows_it', notHeld.length === 0, { not_held: notHeld });
  check('each_closes_once_the_cli_has_its_answer', notClosed.cli.length === 0, { open_still: notClosed.cli });
  check('each_closes_once_the_page_shows_another', notClosed.page.length === 0, { open_still: notClosed.page });
  const [open, reopen] = [median(times.open_s), median(times.reopen_s)];
  seen.open_median_s = open;
  seen.reopen_median_s = reopen;
  check('a_reopen_is_as_quick_as_the_first_open', reopen <= open * BARS.reopen_times_open + BARS.reopen_slack_s, { open_s: open, reopen_s: reopen });
  memory.end = await memoryNow(api);
  // And left alone a minute more: what Calcifer lets go of late.
  await page.waitForTimeout(60_000);
  memory.later = await memoryNow(api);
  memory.later_times_start = { heap: +(memory.later.heap / memory.start.heap).toFixed(3), rss: +(memory.later.rss / memory.start.rss).toFixed(3) };
  check('calcifer_gives_back_what_it_closed', memory.later.heap <= memory.start.heap * BARS.memory_times_start, memory);

  // A render going keeps its project open, with no call, stream or page on it: one waiting for its machine, behind another's run there,
  // too. Each closes once its run has landed.
  const usable = (await api('/v1/status')).local_usable;
  seen.render = { local_usable: usable };
  if (usable && !process.env.ALONE) {
    const first = join(homedir(), '.cozy', 'projects', `web-closes-${stamp}-first`);
    const second = join(homedir(), '.cozy', 'projects', `web-closes-${stamp}-second`);
    folders.push(first, second);
    const made = [];
    for (const [folder, wait] of [[first, 20], [second, 1]]) {
      await newProject(folder);
      await projectAction(folder, 'add', `${WAITS}/wait`, 'waits', `instructions=waits ${wait}`, `seconds:=${wait}`);
      made.push((await status(folder)).state.id);
    }
    ids.push(...made);
    await closes(first);
    await closes(second);
    const busy = async () => {
      const { projects } = await api('/v1/projects?limit=1000');
      return made.map((id) => projects.find((row) => row.id === id)?.busy === true);
    };
    const looks = [];
    const observe = async () => {
      // Held, read first: a render that was going then and isn't by the list's read had closed nothing yet.
      const holding = [held(first), held(second)];
      const [going, runs] = await Promise.all([busy(), api(`/v1/projects/${made[1]}/runs`)]);
      const run = runs.runs?.[0];
      const look = { going, held: holding, second_run: run ? `${run.status}/${run.machine_state ?? ''}` : null };
      looks.push(look);
      seen.looks = looks.length;
      return look;
    };
    let update, finished = false;
    try {
      await projectAction(first, 'render', 'waits', '--on', 'local', '--queue');
      const running = await until('the first run executing', async () => (await api(`/v1/projects/${made[0]}/runs`)).runs?.[0],
        (run) => run?.status === 'in_progress' && run.machine_state === 'running', 5 * STEP_MS);
      // The lifetime suite's update drain holds new roots regardless of CPU parallelism.
      const started = await api('/v1/machines/local/update', {
        method: 'POST', headers: { 'Content-Type': 'application/json', 'Idempotency-Key': `closes-update-${stamp}` },
        body: JSON.stringify({ gpu_memory_limit: 0 }),
      });
      update = started.run;
      if (!update?.id) throw new Error(`queue update was not accepted: ${JSON.stringify(started)}`);
      const waitingFor = `waiting for run ${running.number} to end`;
      const drain = await until('the update holding new runs', async () => {
        const [machine, updating, firstRun] = await Promise.all([
          api('/v1/machines/local'), api(`/v1/runs/${update.id}`), api(`/v1/projects/${made[0]}/runs/${running.id}`),
        ]);
        const state = { machine_update: machine.status?.update, update_waiting: updating.waiting, first_status: firstRun.status };
        seen.render.drain = state;
        if (firstRun.status !== 'in_progress') throw new Error(`the first run ended before the queue gate: ${JSON.stringify(state)}`);
        return state;
      }, (state) => state.machine_update === waitingFor && state.update_waiting?.includes(waitingFor));
      seen.render.queue_update = { first_run: running.number, update_run: update.number, ...drain };
      await projectAction(second, 'render', 'waits', '--on', 'local', '--queue');
      const queued = await until('a queued run with its project held open', async () => {
        const look = await observe();
        const updating = await api(`/v1/runs/${update.id}`);
        if (!updating.waiting?.includes(waitingFor)) throw new Error(`the queue gate ended before its witness: ${JSON.stringify({ look, update_status: updating.status })}`);
        return look;
      }, (look) => look.going[1] && look.second_run?.split('/').includes('queued') && look.held[1] !== false);
      seen.render.queued = queued;

      const ending = Date.now() + 5 * STEP_MS;
      for (;;) {
        const look = await observe();
        if (!look.going.some(Boolean) || Date.now() > ending) break;
        await new Promise((done) => setTimeout(done, 250));
      }
      seen.render.looks = looks.length;
      seen.render.second_runs = [...new Set(looks.map((look) => look.second_run))];
      const goingLooks = looks.filter((look) => look.going.some(Boolean));
      check('a_render_going_keeps_its_project_open', goingLooks.length > 0 && goingLooks.every((look) => look.going.every((going, at) => !going || look.held[at] !== false)), looks.filter((look) => look.going.some((going, at) => going && look.held[at] === false)).slice(0, 4));
      const waiting = (look) => look.second_run?.split('/').includes('queued');
      check('one_waiting_for_its_machine_keeps_it_open', looks.some((look) => look.going[1] && waiting(look) && look.held[1] !== false), seen.render.second_runs);
      check('each_render_lands', !looks.at(-1).going.some(Boolean), looks.at(-1));
      const updated = await until('the queue update completed', () => api(`/v1/runs/${update.id}`),
        (run) => ['completed', 'failed', 'canceled'].includes(run.status), 5 * STEP_MS);
      check('queue_update_completed', updated.status === 'completed', { status: updated.status, reason: updated.reason });
      await until('local ready after its queue update', () => api('/v1/machines/local'), (machine) => machine.state === 'ready', 5 * STEP_MS);
      const [firstClosed, secondClosed] = [await closes(first), await closes(second)];
      check('each_closes_once_its_render_has_landed', firstClosed !== null && secondClosed !== null, { first: firstClosed, second: secondClosed });
      finished = !looks.at(-1).going.some(Boolean);
    } finally {
      if (!finished) {
        if (update?.id) await api(`/v1/runs/${update.id}/cancel`, { method: 'POST' }).catch(() => {});
        for (const id of made) {
          await api(`/v1/projects/${id}/cancel`, {
            method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ names: ['all'] }),
          }).catch(() => {});
        }
        await until('fixture renders settled after failure', busy, (going) => !going.some(Boolean), 5 * STEP_MS).catch(() => {});
        if (update?.id) {
          await until('fixture update settled after failure', () => api(`/v1/runs/${update.id}`),
            (run) => ['completed', 'failed', 'canceled'].includes(run.status), 5 * STEP_MS).catch(() => {});
        }
      }
    }
  }

  check('no_page_errors', errors.length === 0, errors.slice(0, 8));
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  dropAll(folders);
  process.exit(failures.length ? 1 : 0);
})().catch(died);

/** Each project it made, its folder gone. */
function dropAll(folders) {
  for (const folder of folders) rmSync(folder, { recursive: true, force: true });
}

async function died(error) {
  console.error(error);
  await lastLook();
  console.log(JSON.stringify({ failures, seen, during }, null, 2));
  if (seen.project) dropAll(seen.project.folders);
  process.exit(2);
}
