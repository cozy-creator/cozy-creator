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
const { execFile, execFileSync } = require('child_process');
const { createHash } = require('crypto');
const { appendFileSync, copyFileSync, linkSync, mkdirSync, readdirSync, readFileSync, readlinkSync, realpathSync, renameSync, rmSync, writeFileSync } = require('fs');
const { homedir } = require('os');
const { join } = require('path');
const { calcifer, signIn, STEP_MS, lastLook, lookAt, png, noStall, until } = require('./page');

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

/** A project as the files from before project.db, which its first open imports as it imports a person's: pictures approved as they are, and
 *  shots of them, each with two versions, the same as web-scale's. */
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
  const lines = ['version: 4', 'assets:'];
  const assets = {};
  const words = 'The colossal blue whale rises through a storm, lit from below, the sea torn white around it, and the small boat holds. '.repeat(3);
  for (let i = 0; i < PICTURES; i++) {
    const hex = hash(`picture ${i}`);
    file(hex, 'square');
    lines.push(`  pic_${i}: {approved: "${hex.slice(0, 12)}"}`);
  }
  for (let i = 0; i < ASSETS - PICTURES; i++) {
    const name = `shot_${i}`;
    lines.push(`  ${name}:`, `    tool: ${TOOL}/still`, `    instructions: ${words}<pic_${i % PICTURES}> and <pic_${(i * 7) % PICTURES}>.`, '    aspect_ratio: "16:9"');
    const versions = [1, 2].map((number) => {
      const hex = hash(`${name} ${number}`);
      file(hex, 'wide');
      return { number, inputs: 'not yet', files: { image: `sha256:${hex}` }, seed: i * number, run: 1000 + i * 2 + number, execution_ms: 1234 };
    });
    assets[`shot ${i}`] = { name, kind: 'image', versions, numbered: 2 };
  }
  replaceFile(join(folder, '.measured.json'), JSON.stringify(measured));
  replaceFile(join(folder, 'project.yaml'), `${lines.join('\n')}\n`);
  replaceFile(join(folder, 'project.lock'), JSON.stringify({ format: 3, assets, makes: { [`${TOOL}/still`]: 'image' } }));
  const side = Math.ceil(Math.sqrt(ASSETS));
  const rows = ['# Where cards sit. Only the page reads this. Nothing here is an input.', 'version: 1', 'cards:'];
  for (let i = 0; i < ASSETS - PICTURES; i++) rows.push(`  shot_${i}: {x: ${(i % side) * 300}, y: ${Math.floor(i / side) * 320}, w: 260}`);
  for (let i = 0; i < PICTURES; i++) rows.push(`  pic_${i}: {x: ${((ASSETS - PICTURES + i) % side) * 300}, y: ${Math.floor((ASSETS - PICTURES + i) / side) * 320}, w: 260}`);
  replaceFile(join(folder, 'project.canvas.yaml'), `${rows.join('\n')}\n`);
  for (const shape of Object.keys(seeds)) rmSync(seedOf(shape));
}

function replaceFile(at, text) {
  writeFileSync(`${at}.part`, text);
  renameSync(`${at}.part`, at);
}

/** Calcifer's resident memory, in MiB, as its OS says. */
function calciferMb() {
  if (process.platform === 'win32') {
    const line = execFileSync('tasklist', ['/FI', `PID eq ${HOLDER.pid}`, '/FO', 'CSV', '/NH'], { encoding: 'utf8' });
    return Math.round(Number(line.split('","')[4].replace(/[^0-9]/g, '')) / 1024);
  }
  return Math.round(Number(execFileSync('ps', ['-o', 'rss=', '-p', String(HOLDER.pid)], { encoding: 'utf8' }).trim()) / 1024);
}

/** Calcifer's memory in MiB. Its heap, what its code holds, as `cozy status` says: what a project open costs, and closing gives back. And as
 *  its OS says, resident, and on Linux what of that is its own (anonymous) and files mapped in (its code, read in as it's first used), and
 *  how many threads it has. Resident memory also counts what its allocator keeps of what was freed: on the same code, a minute after the
 *  last close, it stood 1.02–1.15× where it was after warm-up (cl-826), so it's kept for the record and judges nothing. */
async function memoryNow(cozy) {
  const { calcifer } = JSON.parse((await cozy('--json', 'status')).stdout);
  const heap = Math.round((calcifer?.heap_bytes ?? NaN) / 2 ** 20 * 10) / 10;
  if (process.platform !== 'linux') return { heap, rss: calciferMb() };
  const status = readFileSync(`/proc/${HOLDER.pid}/status`, 'utf8');
  const field = (name) => Number(status.match(new RegExp(`^${name}:\\s+(\\d+)`, 'm'))?.[1] ?? NaN);
  const mb = (name) => Math.round(field(name) / 1024);
  return { heap, rss: mb('VmRSS'), anon: mb('RssAnon'), file: mb('RssFile'), threads: field('Threads') };
}

/** Whether Calcifer has the project in `folder` open: its project.db among the files it holds. Null where this can't be read (Windows). */
function held(folder) {
  const db = join(realpathSync(folder), 'project.db');
  if (process.platform === 'linux') {
    const fds = `/proc/${HOLDER.pid}/fd`;
    return readdirSync(fds).some((fd) => {
      try {
        return readlinkSync(join(fds, fd)) === db;
      } catch {
        return false;
      }
    });
  }
  if (process.platform === 'darwin') {
    let said = '';
    try {
      said = execFileSync('lsof', ['-p', String(HOLDER.pid), '-Fn'], { encoding: 'utf8', maxBuffer: 1 << 26 });
    } catch (error) {
      said = error.stdout ?? '';
    }
    return said.split('\n').some((line) => line === `n${db}`);
  }
  return null;
}

(async () => {
  const base = calcifer();
  const api = (path, init = {}) => fetch(`${base}${path}`, { ...init, headers: { Authorization: `Bearer ${HOLDER.token}`, ...init.headers } }).then((answer) => answer.json());
  const cozy = (...words) => new Promise((done, fail) => {
    if (process.env.ARGV_LOG) appendFileSync(process.env.ARGV_LOG, `${words.map((word) => `${word}\x1f`).join('')}\x1e`);
    const from = Date.now();
    const env = { ...process.env };
    delete env.COZY_PROJECT;
    execFile(COZY, words, { env, encoding: 'utf8', maxBuffer: 1 << 28 }, (error, stdout, stderr) => {
      if (error) fail(new Error(`cozy ${words.join(' ').slice(0, 300)}: ${error.message}: ${stdout.slice(0, 500)}${stderr.slice(0, 500)}`));
      else done({ stdout, stderr, ms: Date.now() - from });
    });
  });
  const status = async (folder) => {
    const answer = await cozy('--json', 'project', '-p', folder, 'status');
    return { state: JSON.parse(answer.stdout), ms: answer.ms };
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
  await page.goto(`${base}/#/runs`);
  await page.waitForTimeout(SETTLE_MS);
  memory.cold = await memoryNow(cozy);
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
      generated(folder);
      // Its first read imports it; the read after is its first open of project.db. Each closes once it has answered.
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
      await page.goto(`${base}/#/projects/${imported.state.id}`);
      await page.waitForSelector('.nm:text-is("shot_0")', { timeout: 120_000 });
      await page.waitForTimeout(1000);
      if (readable && !held(folder)) notHeld.push(folder);
      if (i > 0) {
        await reopened(i - 1);
        // Each with one project shown, the one before closed: what it holds after each, which levels off or grows with them.
        if (counted) {
          const now = await memoryNow(cozy);
          memory.trend.push(now.heap);
          memory.trend_rss.push(now.anon ?? now.rss);
        }
      }
    }
    await page.goto(`${base}/#/runs`);
    await reopened(list.length - 1);
    await page.waitForTimeout(SETTLE_MS);
  }

  // Two projects through all of it first, as the twenty go: what Calcifer takes on at its first use of all this (its code read in, its
  // allocator's own for two projects open at a time) is no project's. Its memory from there is the start.
  const warm = [0, 1].map((i) => join(homedir(), '.cozy', 'projects', `web-closes-${stamp}-warm-${i}`));
  folders.push(...warm);
  await pass(warm, false);
  memory.start = await memoryNow(cozy);
  await pass(folders.slice(0, PROJECTS), true);

  check('each_is_open_while_the_page_shows_it', notHeld.length === 0, { not_held: notHeld });
  check('each_closes_once_the_cli_has_its_answer', notClosed.cli.length === 0, { open_still: notClosed.cli });
  check('each_closes_once_the_page_shows_another', notClosed.page.length === 0, { open_still: notClosed.page });
  const [open, reopen] = [median(times.open_s), median(times.reopen_s)];
  seen.open_median_s = open;
  seen.reopen_median_s = reopen;
  check('a_reopen_is_as_quick_as_the_first_open', reopen <= open * BARS.reopen_times_open + BARS.reopen_slack_s, { open_s: open, reopen_s: reopen });
  memory.end = await memoryNow(cozy);
  // And left alone a minute more: what Calcifer lets go of late.
  await page.waitForTimeout(60_000);
  memory.later = await memoryNow(cozy);
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
      await cozy('project', 'new', folder);
      await cozy('project', '-p', folder, 'add', `${WAITS}/wait`, 'waits', `instructions=waits ${wait}`, `seconds:=${wait}`);
      made.push((await status(folder)).state.id);
    }
    ids.push(...made);
    await closes(first);
    await closes(second);
    for (const folder of [first, second]) await cozy('project', '-p', folder, 'render', 'waits', '--on', 'local', '--queue');
    const busy = async () => {
      const { projects } = await api('/v1/projects?limit=1000');
      return made.map((id) => projects.find((row) => row.id === id)?.busy === true);
    };
    const looks = [];
    const ending = Date.now() + 5 * STEP_MS;
    for (;;) {
      // Held, read first: a render that was going then and isn't by the list's read had closed nothing yet.
      const holding = [held(first), held(second)];
      const [going, runs] = await Promise.all([busy(), api(`/v1/runs?project=${made[1]}`)]);
      const run = runs.runs?.[0];
      // Its run waits for the machine as Calcifer's queued, or as the machine's: one CPU slot on a runner of four cores, held by the first's.
      const look = { going, held: holding, second_run: run ? `${run.status}/${run.machine_state ?? ''}` : null };
      looks.push(look);
      seen.looks = looks.length;
      if (!going.some(Boolean) || Date.now() > ending) break;
      await new Promise((done) => setTimeout(done, 250));
    }
    seen.render.looks = looks.length;
    seen.render.second_runs = [...new Set(looks.map((look) => look.second_run))];
    const goingLooks = looks.filter((look) => look.going.some(Boolean));
    check('a_render_going_keeps_its_project_open', goingLooks.length > 0 && goingLooks.every((look) => look.going.every((going, at) => !going || look.held[at] !== false)), looks.filter((look) => look.going.some((going, at) => going && look.held[at] === false)).slice(0, 4));
    const waiting = (look) => look.second_run?.split('/').includes('queued');
    check('one_waiting_for_its_machine_keeps_it_open', looks.some((look) => look.going[1] && waiting(look) && look.held[1] !== false), seen.render.second_runs);
    check('each_render_lands', !looks.at(-1).going.some(Boolean), looks.at(-1));
    const [firstClosed, secondClosed] = [await closes(first), await closes(second)];
    check('each_closes_once_its_render_has_landed', firstClosed !== null && secondClosed !== null, { first: firstClosed, second: secondClosed });
  }

  check('no_page_errors', errors.length === 0, errors.slice(0, 8));
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  await forgetAll(ids, folders);
  process.exit(failures.length ? 1 : 0);
})().catch(died);

/** Each project it made, forgotten, and its folder gone. */
async function forgetAll(ids, folders) {
  const base = calcifer();
  for (const id of ids) await fetch(`${base}/v1/projects/${id}`, { method: 'DELETE', headers: { Authorization: `Bearer ${HOLDER.token}` } }).catch(() => {});
  for (const folder of folders) rmSync(folder, { recursive: true, force: true });
}

async function died(error) {
  console.error(error);
  await lastLook();
  console.log(JSON.stringify({ failures, seen, during }, null, 2));
  if (seen.project) await forgetAll(seen.project.ids, seen.project.folders);
  process.exit(2);
}
