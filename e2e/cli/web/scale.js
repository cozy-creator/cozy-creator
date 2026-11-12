// Thousands of assets (Paul expects them): a generated 5,000-asset project, a real Calcifer, a real Chrome. How long a project API read takes, opening it in the page, one edit to show, what's read for it, a box let go on the crowd by hand, a render landing, panning, memory.
// CI seeds the current native Automerge/SQLite baseline before timed reads: 4,000 shots with two versions each and 1,000 pictures, using the
// same synthetic media and layout. From its first open onward, only the page and Calcifer's current HTTP API change that project state.
// The bars are what a person would feel, in BARS, on the fast hosted runners: each time bar is held times this runner's
// measured speed against those (speed.js), and seen.bars has what each was held to. What each took is seen.scale.
//   node scale.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $COZY: the CLI; $SHOT: where to keep its screenshots; $ALONE: on a shared
// computer, so it runs nothing; $HOLDER: Calcifer's lock file, which names its process; $ARGV_LOG: where each cozy call is logged for coverage.
// Prints what it saw as JSON and exits 1 if anything failed.
const { chromium } = require(process.env.PLAYWRIGHT);
const { execFile, execFileSync } = require('child_process');
const { createHash } = require('crypto');
const { appendFileSync, closeSync, copyFileSync, fsyncSync, linkSync, mkdirSync, openSync, readFileSync, renameSync, rmSync, statSync, writeFileSync, writeSync } = require('fs');
const { homedir } = require('os');
const { join } = require('path');
const { projectUrl, projectAction, calcifer, signIn, STEP_MS, lastLook, lookAt, png, noStall, closeBox, openBox, renderOf, until, projectHeld } = require('./page');
const { readFootprint, closedProjectMemory } = require('./native-memory');
const { runnerSpeed } = require('./speed');
const { prepareCurrentSeed, seedCurrentProject } = require('./current-seed');
// Compilation is setup, before runner calibration, resident-memory or timed reads.
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

const ASSETS = 5000;
const HOLE = { col: 3, row: 1 };
const PAN_MS = 3000;
const REST_MS = 2000;
const PICTURES = 1000;
/** How long after its project is closed Calcifer's memory is read, on a fast runner. */
const CLOSED_MS = 10_000;
const COZY = process.env.COZY;
const TOOL = join(__dirname, '..', 'project', 'tool.py');
// What a person would feel, on a runner slower than their computer. A command you'd run without thinking; a project that opens as a page does;
// the second of the CLI's "live"; a pan that doesn't stutter; memory a laptop gives without noticing.
const BARS = {
  status_s: 3,
  status_again_s: 1,
  command_s: 2,
  open_boxes_s: 5,
  open_settled_s: 10,
  edit_shows_ms: 1500,
  // A box let go on others gives way before the next frame or two: from the pointer's release to its slide, with all 5,000 in the field.
  drop_ms: 100,
  // A person's write, flushed to disk and answered, the middle of nine. "Under 100-ms is pretty much fast enough for most operations" (Paul, 11-05).
  write_ms: 100,
  landing_shows_ms: 1500,
  // What a pan costs the page itself, per frame, against the same pan of a project of 36, the same boxes on screen, on the same runner: size adds
  // nothing. Frames aren't the bar: a runner draws with no GPU, and macOS's frames run 33 to 133 ms on a project of ten boxes. They're shown.
  pan_work_times_small: 1.5,
  pan_work_slack_ms: 2,
  // ms of the page's thread each second left alone. Read 0.1 to 0.2 on this project on all three OSes, 11.9 at the most on any: this is twice that,
  // and under what a loop of frames would spend, 1 to 3 ms sixty times a second.
  rest_ms_per_s: 25,
  fit_all_s: 10,
  calcifer_mb: 1024,
  // With ten asks owed, each read works out each one's text, which `rc/frieren` keeps apart from the lock: what a queue costs here, until that lands.
  asks_status_s: 5,
  calcifer_asks_mb: 2048,
  // Its project closed, what Calcifer kept of it and of working it: a quarter of what the open project may cost. glibc's arenas kept
  // ~1,500 MB of each run, and the next run's on top (cl-822).
  calcifer_kept_mb: 256,
  page_heap_mb: 512,
};

// The bars that are times: each is held times this runner's speed factor. The memory bars and the pan's ratio stay.
const TIMED = ['status_s', 'status_again_s', 'command_s', 'open_boxes_s', 'open_settled_s', 'edit_shows_ms', 'drop_ms', 'write_ms', 'landing_shows_ms', 'pan_work_slack_ms', 'rest_ms_per_s', 'fit_all_s', 'asks_status_s'];
// Measured first, before this script or Calcifer has anything of the project to do.
const SPEED = runnerSpeed();
const BAR = { ...BARS, ...Object.fromEntries(TIMED.map((name) => [name, +(BARS[name] * SPEED.factor).toFixed(3)])) };

const sha = (bytes) => createHash('sha256').update(bytes).digest('hex');

/** A fully populated current native project, with the same synthetic files and two versions per shot.
 * `inputs`, once known, makes them current. With no `takes`, shots have none: what's read for their keys. */
function generated(folder, inputs, assetCount = ASSETS, pictures = PICTURES, takes = true) {
  mkdirSync(join(folder, 'files'), { recursive: true });
  // A picture is square; a shot's take is the 16:9 it asks for, as a render makes it, so a box showing one is that shape.
  const seeds = { square: [16, 16], wide: [32, 18] };
  const seedOf = (shape) => join(folder, `seed-${shape}.png`);
  for (const [shape, [width, height]] of Object.entries(seeds)) writeFileSync(seedOf(shape), png(90, width, height));
  const hash = (word) => sha(Buffer.from(word));
  let file = (hex, shape) => {
    const at = join(folder, 'files', `${hex}.png`);
    try {
      linkSync(seedOf(shape), at);
    } catch (error) {
      // NTFS allows 1023 links to a file.
      if (error.code !== 'EEXIST') copyFileSync(seedOf(shape), at);
    }
  };
  // Each box in order, its fields as stored: [[field, value], …].
  const boxes = [];
  // Each measured as it landed, as a person's are.
  const measured = {};
  const made = file;
  file = (hex, shape = 'square') => {
    made(hex, shape);
    const [width, height] = seeds[shape];
    measured[`sha256:${hex}`] = { width, height };
  };
  const assets = {};
  const words = 'The colossal blue whale rises through a storm, lit from below, the sea torn white around it, and the small boat holds. '.repeat(3);
  for (let i = 0; i < pictures; i++) {
    const hex = hash(`picture ${i}`);
    file(hex);
    boxes.push({ name: `pic_${i}`, fields: [['approved', hex.slice(0, 12)]] });
  }
  // The suite's own weightless tool, whose fields a 0.22 box draws whatever the Hub has published.
  const tool = `${TOOL}/still`;
  for (let i = 0; i < assetCount - pictures; i++) {
    const name = `shot_${i}`;
    boxes.push({ name, fields: [['tool', tool], ['instructions', `${words}<pic_${i % pictures}> and <pic_${(i * 7) % pictures}>.`], ['aspect_ratio', '16:9']] });
    if (!takes) continue;
    const versions = [1, 2].map((number) => {
      const hex = hash(`${name} ${number}`);
      file(hex, 'wide');
      return { number, inputs: inputs?.[name] ?? 'not yet', files: { image: `sha256:${hex}` }, seed: i * number, run: 1000 + i * 2 + number, execution_ms: 1234 };
    });
    assets[`shot ${i}`] = { name, kind: 'image', versions, numbered: 2 };
  }
  // A grid, each where it'd sit if a person had placed it.
  const side = Math.ceil(Math.sqrt(assetCount));
  const cards = {};
  // A place left empty near the top: a grid has no other room, and a box that lands on another is moved off it.
  const hole = side * HOLE.row + HOLE.col;
  const card = (name, at) => {
    const place = at < hole ? at : at + 1;
    cards[name] = { x: (place % side) * 300, y: Math.floor(place / side) * 320, w: 260 };
  };
  for (let i = 0; i < assetCount - pictures; i++) card(`shot_${i}`, i);
  for (let i = 0; i < pictures; i++) card(`pic_${i}`, assetCount - pictures + i);
  const seeded = seedCurrentProject(folder, { boxes, assets, makes: { [tool]: 'image' }, measured, cards,
    expected_assets: assetCount, expected_versions: takes ? (assetCount - pictures) * 2 : 0 });
  for (const shape of Object.keys(seeds)) rmSync(seedOf(shape));
  return seeded;
}

/** A wait that ends, so a cell that can't see what it waits for says so. */
const within = (what, waiting, ms = STEP_MS) => Promise.race([waiting, new Promise((_, fail) => { setTimeout(() => fail(new Error(`${what}: not within ${ms} ms`)), ms).unref(); })]);

/** What a plain flush costs on the disk under `dir`, the middle of 20: 4 KB, and as many bytes as `like`. Nothing of cozy's is in it. */
function flushed(dir, like) {
  const at = join(dir, '.flush-probe');
  const flush = (bytes) => {
    const took = [];
    for (let i = 0; i < 20; i++) {
      const held = openSync(at, 'w');
      writeSync(held, Buffer.alloc(bytes, i));
      const began = performance.now();
      fsyncSync(held);
      took.push(performance.now() - began);
      closeSync(held);
    }
    rmSync(at);
    return summary(took).median_ms;
  };
  // As `durable::replace` goes: written beside it, flushed, renamed over it.
  const replace = (bytes) => {
    const took = [];
    for (let i = 0; i < 20; i++) {
      const began = performance.now();
      const held = openSync(`${at}.part`, 'w');
      writeSync(held, Buffer.alloc(bytes, i));
      fsyncSync(held);
      closeSync(held);
      renameSync(`${at}.part`, at);
      took.push(performance.now() - began);
    }
    rmSync(at);
    return summary(took).median_ms;
  };
  const like_bytes = statSync(like).size;
  return { flush_4k_ms: flush(4096), flush_like_ms: flush(like_bytes), replace_like_ms: replace(like_bytes), like_bytes };
}

function summary(frames) {
  const sorted = [...frames].sort((a, b) => a - b);
  return { median_ms: +sorted[sorted.length >> 1].toFixed(1), p95_ms: +sorted[Math.floor(sorted.length * 0.95)].toFixed(1), worst_ms: +sorted.at(-1).toFixed(1), frames: sorted.length };
}

/** Calcifer's resident memory, in MiB, as its OS says. */
function calciferMb() {
  const { pid } = JSON.parse(readFileSync(process.env.HOLDER, 'utf8'));
  if (process.platform === 'win32') {
    const line = execFileSync('tasklist', ['/FI', `PID eq ${pid}`, '/FO', 'CSV', '/NH'], { encoding: 'utf8' });
    return Math.round(Number(line.split('","')[4].replace(/[^0-9]/g, '')) / 1024);
  }
  return Math.round(Number(execFileSync('ps', ['-o', 'rss=', '-p', String(pid)], { encoding: 'utf8' }).trim()) / 1024);
}

/** What holds Calcifer's resident memory (Linux, /proc/<pid>/smaps), in MiB: by mapping kind, and its largest mappings. An anonymous
 *  mapping of 256 MiB or more is a mimalloc arena; [heap] and the smaller anonymous ones are glibc's (SQLite's pages among them),
 *  stacks and small maps. */
function residentByMapping() {
  const { pid } = JSON.parse(readFileSync(process.env.HOLDER, 'utf8'));
  let text;
  try { text = readFileSync(`/proc/${pid}/smaps`, 'utf8'); } catch { return null; }
  const maps = [];
  for (const line of text.split('\n')) {
    const head = line.match(/^([0-9a-f]+)-([0-9a-f]+) (\S+) \S+ \S+ \d+\s*(.*)$/);
    if (head) maps.push({ size_mib: (parseInt(head[2], 16) - parseInt(head[1], 16)) / 1048576, perms: head[3], name: head[4], rss_mib: 0 });
    const rss = line.match(/^Rss:\s+(\d+) kB/);
    if (rss && maps.length) maps.at(-1).rss_mib += Number(rss[1]) / 1024;
  }
  const kind = ({ size_mib, name }) => (name === '[heap]' ? 'heap' : name.startsWith('[') ? name : name ? 'file' : size_mib >= 256 ? 'arena' : 'anon');
  const kinds = {};
  for (const map of maps) kinds[kind(map)] = +((kinds[kind(map)] ?? 0) + map.rss_mib).toFixed(1);
  const largest = maps.filter((map) => map.rss_mib >= 1).sort((a, b) => b.rss_mib - a.rss_mib).slice(0, 12)
    .map((map) => ({ ...map, size_mib: +map.size_mib.toFixed(1), rss_mib: +map.rss_mib.toFixed(1) }));
  return { kinds, anon_mappings: maps.filter((map) => !map.name).length, largest };
}

/** Native ownership evidence beside the RSS bar, from this exact scale case. */
async function nativeMemory(base, folder) {
  const holder = JSON.parse(readFileSync(process.env.HOLDER, 'utf8'));
  const sample = { at_ms: Date.now(), heap_bytes: null, project_db_open: null };
  let physicalBefore;
  if (process.platform === 'darwin') {
    try { physicalBefore = readFootprint(holder.pid); }
    catch (error) { sample.physical_error = error.message; }
  }
  try {
    const reply = await fetch(`${base}/v1/status`, { headers: { Authorization: `Bearer ${holder.token}` } });
    const status = await reply.json();
    if (!reply.ok || !Number.isFinite(status.heap_bytes)) throw new Error(`status ${reply.status} carries no native heap counter`);
    sample.heap_bytes = status.heap_bytes;
  } catch (error) { sample.heap_error = error.message; }
  try { sample.project_db_open = projectHeld(folder, holder.pid); }
  catch (error) { sample.ownership_error = error.message; }
  if (process.platform === 'darwin') {
    try {
      const after = JSON.parse(readFileSync(process.env.HOLDER, 'utf8'));
      const pressure = readFootprint(after.pid);
      const current = JSON.parse(readFileSync(process.env.HOLDER, 'utf8'));
      sample.physical = { holder_pid: holder.pid, after_holder_pid: current.pid, before: physicalBefore, after: pressure };
    } catch (error) { sample.physical_error = error.message; }
  }
  sample.finished_at_ms = Date.now();
  return sample;
}

(async () => {
  const base = calcifer();
  // Generated once with no takes, to read each shot's real key, then with them where its first open finds them current.
  const keying = join(homedir(), '.cozy', 'projects', `web-scale-keys-${Date.now()}`);
  const folder = join(homedir(), '.cozy', 'projects', `web-scale-${Date.now()}`);
  mkdirSync(keying, { recursive: true });
  mkdirSync(folder, { recursive: true });
  const numbers = {};
  seen.scale = numbers;
  seen.runner = SPEED;
  seen.bars = BAR;
  seen.bars_on_a_fast_runner = BARS;

  const cozyAt = (at, ...words) => new Promise((done, fail) => {
    if (process.env.ARGV_LOG) appendFileSync(process.env.ARGV_LOG, `${words.map((word) => `${word}\x1f`).join('')}\x1e`);
    const from = Date.now();
    execFile(COZY, words, { cwd: at, encoding: 'utf8', maxBuffer: 1 << 28 }, (error, stdout, stderr) => {
      const at = Date.now();
      if (error) fail(new Error(`cozy ${words.join(' ').slice(0, 300)}: ${error.message}: ${stdout.slice(0, 500)}${stderr.slice(0, 500)}`));
      else done({ stdout, stderr, at, ms: at - from });
    });
  });
  const cozy = (...words) => cozyAt(folder, ...words);
  const project = (...words) => projectAction(folder, ...words);
  const status = async (at = folder) => {
    const answer = await projectAction(at, 'status');
    return { state: answer.value, ms: answer.ms };
  };
  const mb = (bytes) => +(bytes / 1048576).toFixed(1);
  const dbMb = () => mb(statSync(join(folder, 'project.db')).size);
  const seconds = (ms) => +(ms / 1000).toFixed(2);

  const startMb = calciferMb();
  numbers.calcifer_mappings = { before: residentByMapping() };
  numbers.calcifer_native = { before: await nativeMemory(base, folder) };
  const generating = Date.now();
  seen.native_seed = { keying: generated(keying, null, ASSETS, PICTURES, false) };
  numbers.generate_s = seconds(Date.now() - generating);

  // The first read opens current native storage; then each shot gets its real key, so they stand as a person's would. Where this
  // computer runs models its tool is read by this machine's Runtime, from a run of it there, as tools.js primes its fixture.
  let keyed = await status(keying);
  numbers.status_first_s = seconds(keyed.ms);
  const local = (await (await fetch(`${base}/v1/status`, { headers: { Authorization: `Bearer ${JSON.parse(readFileSync(process.env.HOLDER, 'utf8')).token}` } })).json()).local_usable;
  if (local) {
    await cozyAt(keying, '--json', 'run', `${TOOL}/still`, 'instructions=primed', '--on', 'local');
    keyed = await status(keying);
  }
  const inputs = Object.fromEntries(keyed.state.assets.filter((asset) => asset.name.startsWith('shot_')).map((asset) => [asset.name, asset.inputs]));
  // Its folder goes at the end, with the small one.
  seen.keyed = Object.keys(inputs).length;
  seen.native_seed.main = generated(folder, inputs);
  const first = await status();
  numbers.status_import_s = seconds(first.ms);
  check('all_are_there', first.state.assets.length === ASSETS, first.state.assets.length);
  check('its_storage_is_current_native', seen.native_seed.main.schema === seen.native_seed.main.current_schema && seen.native_seed.main.crdt_format === seen.native_seed.main.current_crdt_format && seen.native_seed.main.assets === ASSETS && seen.native_seed.main.versions === (ASSETS - PICTURES) * 2);
  seen.project = { id: first.state.id, folder };
  const id = first.state.id;
  const read = await status();
  numbers.status_s = seconds(read.ms);
  numbers.db_mb = dbMb();
  numbers.status_answer_mb = mb(JSON.stringify(read.state).length);
  const standing = read.state.assets.reduce((held, asset) => ({ ...held, [asset.standing]: (held[asset.standing] ?? 0) + 1 }), {});
  numbers.standing = standing;
  check('they_stand_as_a_persons_would', standing.current === ASSETS, standing);
  check('status_is_quick', numbers.status_s <= BAR.status_s, { s: numbers.status_s, bar: BAR.status_s });
  const again = await status();
  numbers.status_again_s = seconds(again.ms);
  check('status_again_is_quicker', numbers.status_again_s <= BAR.status_again_s, { s: numbers.status_again_s, bar: BAR.status_again_s });
  const memory = { before: startMb, read: calciferMb() };
  numbers.calcifer_memory_mb = memory;
  numbers.calcifer_native.read = await nativeMemory(base, folder);

  const browser = await chromium.launch({ executablePath: process.env.CHROME, headless: true, args: ['--enable-precise-memory-info'] });
  const context = await browser.newContext({ viewport: { width: 1800, height: 1200 } });
  const page = await context.newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
  page.on('console', (m) => { if (m.type() === 'error' && !/status of 4\d\d|net::ERR_/.test(m.text())) errors.push(`console: ${m.text()}`); });
  await page.addInitScript(() => {
    window.violations = [];
    document.addEventListener('securitypolicyviolation', (e) => window.violations.push(`${e.violatedDirective}: ${e.blockedURI}`));
    window.boxOf = (name) => document.querySelector(`.card[data-asset="${CSS.escape(name)}"]`)?.closest('.react-flow__node');
    window.placedAt = (name, x, y) => new Promise((done) => {
      const look = () => {
        const node = window.boxOf(name);
        const m = node && new DOMMatrix(getComputedStyle(node).transform);
        return m && Math.abs(m.e - x) < 0.5 && Math.abs(m.f - y) < 0.5 ? done(Date.now()) : requestAnimationFrame(look);
      };
      look();
    });
    window.wordsAre = (name, words) => new Promise((done) => {
      const held = () => [...document.querySelectorAll(`.card.here[data-asset="${name}"] .mention-text[data-field=instructions], .card.here[data-asset="${name}"] .held-line`)];
      const look = () => (held().some((line) => line.textContent.includes(words)) ? done(Date.now()) : requestAnimationFrame(look));
      look();
    });
    window.pictureOf = (name) => new Promise((done) => {
      const look = () => (window.boxOf(name)?.querySelector('.media img') ? done(Date.now()) : requestAnimationFrame(look));
      look();
    });
    window.takeFrames = (ms) => new Promise((done) => {
      const frames = [];
      let last = performance.now();
      const end = last + ms;
      const step = (now) => {
        frames.push(now - last);
        last = now;
        if (now < end) requestAnimationFrame(step);
        else done(frames.slice(1));
      };
      requestAnimationFrame(step);
    });
  });
  const reads = { whole: 0, heads: 0, assets: 0, whole_bytes: 0 };
  /** Each asset as the page last read it alone, and when. */
  const readAlone = new Map();
  /** What the page read and found gone. */
  const readGone = new Set();
  page.on('request', (request) => {
    const url = new URL(request.url());
    if (request.method() !== 'GET') return;
    const kind = url.pathname === `/v1/projects/${id}` ? (url.searchParams.get('assets') === 'false' ? 'heads' : 'whole')
      : url.pathname.startsWith(`/v1/projects/${id}/assets/`) ? 'assets'
      : url.pathname === `/v1/projects/${id}/canvas` ? 'canvas' : null;
    if (kind && kind !== 'canvas') reads[kind] += 1;
  });
  page.on('response', (answer) => {
    const request = answer.request();
    if (request.method() !== 'GET') return;
    const { pathname } = new URL(answer.url());
    if (pathname === `/v1/projects/${id}`) answer.body().then((body) => { reads.whole_bytes = Math.max(reads.whole_bytes, body.length); }, () => {});
    if (pathname.startsWith(`/v1/projects/${id}/assets/`)) answer.json().then(({ asset }) => {
      if (asset) readAlone.set(asset.name, { words: asset.fields?.instructions, at: Date.now() });
      else readGone.add(decodeURIComponent(pathname.split('/').at(-1)));
    }, () => {});
  });
  // By its title: a name is drawn breakable after its underscores (cl-958's <wbr>), which its text no longer matches whole.
  const box = (name) => page.locator('.react-flow__node').filter({ has: page.locator(`.nm[title="${name}"]`) });
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png`, timeout: 60_000 }).catch(() => {}) : Promise.resolve());

  await signIn(page);
  await page.waitForSelector('.top');
  const { token } = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session')));

  // The 95th percentile of half a second's frames. None in that half second is one longer than all of it: not short yet (macOS, 42022159182).
  const shortFrames = async () => {
    const frames = await page.evaluate(() => window.takeFrames(500));
    return frames.length ? summary(frames).p95_ms : Infinity;
  };
  /** Frames while the viewport goes 480 px left and back, every box on screen drawn first, and with no pan, each as a person's wheel does it. */
  const panning = (ms) => page.evaluate((length) => new Promise((done) => {
    const pane = document.querySelector('.react-flow__pane');
    const frames = [];
    let [last, sign, count] = [performance.now(), 1, 0];
    const end = last + length;
    const step = (now) => {
      frames.push(now - last);
      last = now;
      if (++count % 40 === 0) sign = -sign;
      pane.dispatchEvent(new WheelEvent('wheel', { deltaX: sign * 12, deltaY: 0, bubbles: true, cancelable: true }));
      if (now < end) requestAnimationFrame(step);
      else done(frames.slice(1));
    };
    requestAnimationFrame(step);
  }), ms);
  const drawn = () => until('every box on screen drawn', () => page.evaluate(() => {
    const images = [...document.querySelectorAll('.react-flow__node .media img')];
    return images.length > 0 && images.every((image) => image.complete);
  }), Boolean, 120_000);
  const cdp = await context.newCDPSession(page);
  await cdp.send('Performance.enable');
  /** The page's own thread, in ms: its time on a core, and by the clock in all and at each kind of work. What the compositor and a software rasterizer take isn't in this. */
  const worked = async () => Object.fromEntries((await cdp.send('Performance.getMetrics')).metrics.map(({ name, value }) => [name, value * 1000]));
  /** What the page's thread spends `ms` on, by function: what's asked of it isn't, as nothing is. */
  const profiled = async (ms) => {
    await cdp.send('Profiler.enable');
    await cdp.send('Profiler.start');
    await page.waitForTimeout(ms);
    const { profile } = await cdp.send('Profiler.stop');
    const spent = new Map();
    for (const [i, id] of profile.samples.entries()) spent.set(id, (spent.get(id) ?? 0) + profile.timeDeltas[i]);
    const named = profile.nodes.map(({ id, callFrame: { functionName, url, lineNumber } }) => ({ what: `${functionName || '(anonymous)'} ${url.split('/').at(-1)}:${lineNumber}`, ms: Math.round((spent.get(id) ?? 0) / 1000) }));
    return named.filter(({ what, ms: took }) => took > 0 && !what.startsWith('(idle)')).sort((a, b) => b.ms - a.ms).slice(0, 10);
  };
  const resting = async () => {
    await closeBox(page);
    await page.locator('.card.here').waitFor({ state: 'detached' });
    const state = await page.evaluate(() => ({ hash: location.pathname, opened: document.querySelectorAll('.card.here').length }));
    if (state.opened || state.hash.includes('/assets/')) throw new Error(`expected the real resting canvas: ${JSON.stringify(state)}`);
  };
  const measuredPan = async () => {
    await resting();
    await drawn();
    const still = summary(await page.evaluate(() => window.takeFrames(1000)));
    // Left alone, no frame asked for and nothing moved, as a canvas is on a laptop with no one at it.
    const before = await worked();
    await page.waitForTimeout(REST_MS);
    const after = await worked();
    const perSecond = (name) => +(((after[name] - before[name]) / REST_MS) * 1000).toFixed(1);
    const rest_ms_per_s = perSecond('TaskDuration');
    const from = await worked();
    const moving = summary(await panning(PAN_MS));
    const to = await worked();
    const perFrame = (name) => +((to[name] - from[name]) / moving.frames).toFixed(2);
    // Time on a core, which a busy runner can't stretch: by the clock macOS read 6 to 14 ms a frame four times, a minute after 1, where Linux and Windows
    // never passed 4. What goes on at rest goes on through a pan, and isn't the pan's: `work_ms_per_frame` is all of it, `added_ms_per_frame` what panning adds.
    const added = Math.max(0, to.ThreadTime - from.ThreadTime - perSecond('ThreadTime') * (PAN_MS / 1000));
    return {
      still,
      moving,
      rest_ms_per_s,
      rest_kinds: { script: perSecond('ScriptDuration'), layout: perSecond('LayoutDuration'), style: perSecond('RecalcStyleDuration') },
      work_ms_per_frame: perFrame('ThreadTime'),
      clock_ms_per_frame: { task: perFrame('TaskDuration'), script: perFrame('ScriptDuration'), layout: perFrame('LayoutDuration'), style: perFrame('RecalcStyleDuration') },
      added_ms_per_frame: +(added / moving.frames).toFixed(2),
      mounted: await page.locator('.react-flow__node').count(),
    };
  };

  // The same pan of a small project, for what this runner draws whatever a project holds.
  const smallFolder = join(homedir(), '.cozy', 'projects', `web-scale-small-${Date.now()}`);
  mkdirSync(smallFolder, { recursive: true });
  generated(smallFolder, null, 36, 8);
  const small = (await projectAction(smallFolder, 'status')).value;
  await page.goto(await projectUrl(small.id));
  await page.waitForSelector('.react-flow__node .nm');
  await page.evaluate(() => { window.notReloaded = true; });
  numbers.pan_small = await measuredPan();
  check('small_pan_keeps_document', await page.evaluate(() => window.notReloaded === true));

  // Opening it: boxes on screen, then the page settled, its frames short.
  const opening = Date.now();
  await page.goto(await projectUrl(id));
  await page.waitForSelector('.react-flow__node .nm[title="shot_0"]', { timeout: 120_000 });
  numbers.open_boxes_s = seconds(Date.now() - opening);
  check('it_opens_quickly', numbers.open_boxes_s <= BAR.open_boxes_s, { s: numbers.open_boxes_s, bar: BAR.open_boxes_s });
  // The deliberate project navigation created this document. Watch its interactions from here.
  await page.evaluate(() => { window.notReloaded = true; });
  await until('its frames short', shortFrames, (p95) => p95 < 100, 180_000);
  numbers.open_settled_s = seconds(Date.now() - opening);
  check('it_settles_quickly', numbers.open_settled_s <= BAR.open_settled_s, { s: numbers.open_settled_s, bar: BAR.open_settled_s });
  memory.open = calciferMb();
  numbers.calcifer_native.open = await nativeMemory(base, folder);
  numbers.mounted_boxes = await page.locator('.react-flow__node').count();
  numbers.pan_big = await measuredPan();
  const allowed = numbers.pan_small.added_ms_per_frame * BARS.pan_work_times_small + BAR.pan_work_slack_ms;
  check('size_adds_nothing_to_a_pan', numbers.pan_big.added_ms_per_frame <= allowed, { allowed, small: numbers.pan_small, big: numbers.pan_big });
  // Over the bar it says what's spending it, then and there.
  const quiet = numbers.pan_big.rest_ms_per_s <= BAR.rest_ms_per_s;
  if (!quiet) numbers.rest_profile = await profiled(REST_MS);
  check('a_page_at_rest_is_quiet', quiet, { bar: BAR.rest_ms_per_s, big: numbers.pan_big.rest_ms_per_s, kinds: numbers.pan_big.rest_kinds, small: numbers.pan_small.rest_ms_per_s, profile: numbers.rest_profile });
  numbers.page_heap_open_mb = await page.evaluate(() => Math.round(performance.memory.usedJSHeapSize / 1048576));
  numbers.whole_read_mb = +(reads.whole_bytes / 1048576).toFixed(1);
  await shot('open');

  // One edit, of a box on screen. It shows within a second or so, by a read of it alone, and the command itself is quick.
  const onScreen = await page.evaluate(() => [...document.querySelectorAll('.react-flow__node')]
    .map((node) => ({ name: node.querySelector('.nm')?.textContent, rect: node.getBoundingClientRect() }))
    .filter(({ name, rect }) => name?.startsWith('shot_') && rect.left > 240 && rect.top > 72 && rect.right < innerWidth && rect.bottom < innerHeight)
    .map(({ name }) => name));
  numbers.shots_on_screen = onScreen.length;
  check('a_shot_on_screen', onScreen.length > 0, await page.locator('.nm').allTextContents());
  const edited = onScreen[0];
  // A box of a picture says nothing at rest of its words, nor that they've changed (Paul, 11-05): open, its instructions are what the file says,
  // its tool's own field, or under More what it holds that its tool doesn't read.
  const openOn = async (name) => {
    await resting();
    await openBox(page, name);
    const here = page.locator(`.card.here[data-asset="${name}"]`);
    await here.waitFor();
    // On a take an image tool made, its words are its next take's, behind its +.
    await (await renderOf(page, name)).waitFor();
    await here.locator('.more-toggle').waitFor();
    if (!(await here.locator('.edit-more').count())) await here.locator('.more-toggle').click();
    await here.locator('.mention-text[data-field=instructions], .held-line:has-text("Instructions")').first().waitFor();
  };
  const says = (name, words) => within(`${name} says ${words}`, page.evaluate(([asked, said]) => window.wordsAre(asked, said), [name, words]));
  await openOn(edited);
  const before = { ...reads };
  const shownEdit = says(edited, 'ONE EDIT FROM THE CLI');
  const edit = await project('set', edited, 'instructions=ONE EDIT FROM THE CLI');
  numbers.edit_shows_ms = (await shownEdit) - edit.at;
  check('one_edit_shows_live', numbers.edit_shows_ms <= BAR.edit_shows_ms, { ms: numbers.edit_shows_ms, bar: BAR.edit_shows_ms });
  await page.waitForTimeout(600);
  numbers.edit_reads = { whole: reads.whole - before.whole, heads: reads.heads - before.heads, assets: reads.assets - before.assets };
  check('one_edit_reads_only_it', numbers.edit_reads.whole === 0 && numbers.edit_reads.assets >= 1 && numbers.edit_reads.assets <= 3, numbers.edit_reads);
  check('one_edit_read_is_right', readAlone.get(edited)?.words === 'ONE EDIT FROM THE CLI', readAlone.get(edited));
  // The middle of five: one alone on a Windows runner has taken 2.3 s, in a run whose edits averaged 0.8 s, and the others are what a person gets.
  const sets = [edit.ms];
  for (let turn = 1; turn < 5; turn++) sets.push((await project('set', edited, `instructions=EDIT ${turn} FROM THE CLI`)).ms);
  numbers.set_commands_s = sets.map(seconds);
  numbers.set_command_s = seconds([...sets].sort((a, b) => a - b)[2]);
  check('a_command_is_quick', numbers.set_command_s <= BAR.command_s, { s: numbers.set_commands_s, bar: BAR.command_s });

  // On to the next box, as a person goes on.
  await openOn(onScreen[1] ?? edited);
  await resting();
  await until('its frames short again', shortFrames, (p95) => p95 < 100, 60_000);

  // Moved by a command, as a drag is: a row, and no read of the project.
  const moving = { ...reads };
  const { x: fromX, y: fromY } = await page.evaluate((name) => {
    const m = new DOMMatrix(getComputedStyle(window.boxOf(name)).transform);
    return { x: m.e, y: m.f };
  }, edited);
  const [movedX, movedY] = [fromX + 30, fromY + 30];
  const shownMove = within('moved', page.evaluate(([name, x, y]) => window.placedAt(name, x, y), [edited, movedX, movedY]));
  const move = await project('set', edited, `at=${movedX},${movedY}`);
  numbers.move_command_s = seconds(move.ms);
  numbers.move_shows_ms = (await shownMove) - move.at;
  check('a_move_shows_live', numbers.move_shows_ms <= BAR.edit_shows_ms, { ms: numbers.move_shows_ms, bar: BAR.edit_shows_ms });
  check('a_move_reads_nothing', reads.whole === moving.whole && reads.assets === moving.assets, { moving, reads });

  // A box let go by hand on the crowd, its middle in the gutter between two and on both (Paul, 11-06): nothing near has room, even shrunk,
  // so it gives way by a move, at once, and lies on nothing.
  const crowd = await page.evaluate(() => {
    const shown = [...document.querySelectorAll('.react-flow__node')].map((node) => ({ name: node.querySelector('.card')?.dataset.asset, rect: node.querySelector('.card').getBoundingClientRect() }))
      .filter(({ name, rect }) => name?.startsWith('shot_') && rect.left > 240 && rect.top > 72 && rect.right < innerWidth && rect.bottom < innerHeight);
    for (const a of shown) {
      const b = shown.find((other) => other.rect.left > a.rect.right && other.rect.left - a.rect.right < a.rect.width / 2 && Math.abs(other.rect.top - a.rect.top) < 2);
      const carried = b && shown.find((other) => other !== a && other !== b && other.rect.top > a.rect.bottom);
      if (carried) return { carried: carried.name, x: (a.rect.right + b.rect.left) / 2, y: (a.rect.top + a.rect.bottom) / 2 };
    }
    return null;
  });
  check('a_crowd_on_screen', Boolean(crowd), crowd);
  if (crowd) {
    const media = await box(crowd.carried).locator('.media').first().boundingBox();
    const [fromX, fromY] = [media.x + media.width / 2, media.y + media.height / 2];
    await page.mouse.move(fromX, fromY);
    await page.mouse.down();
    await page.mouse.move(fromX, fromY - 3);
    await page.mouse.move(crowd.x, crowd.y, { steps: 10 });
    await page.evaluate((name) => {
      const node = window.boxOf(name);
      const held = node.style.transform;
      window.gaveWay = new Promise((done) => {
        let up = 0;
        document.addEventListener('pointerup', () => { up = performance.now(); }, { capture: true, once: true });
        new MutationObserver((_, watching) => {
          if (up && node.style.transform !== held) {
            watching.disconnect();
            done(performance.now() - up);
          }
        }).observe(node, { attributes: true, attributeFilter: ['style'] });
      });
    }, crowd.carried);
    await page.mouse.up();
    numbers.drop_ms = +(await within('it gave way', page.evaluate(() => window.gaveWay))).toFixed(1);
    check('a_drop_on_a_crowd_gives_way_at_once', numbers.drop_ms <= BAR.drop_ms, { ms: numbers.drop_ms, bar: BAR.drop_ms });
    await page.waitForTimeout(400);
    const lying = await page.evaluate((name) => {
      const mine = window.boxOf(name).querySelector('.card').getBoundingClientRect();
      return [...document.querySelectorAll('.react-flow__node .card')].filter((card) => card.dataset.asset !== name).map((card) => ({ name: card.dataset.asset, rect: card.getBoundingClientRect() }))
        .filter(({ rect }) => rect.left < mine.right - 0.5 && mine.left < rect.right - 0.5 && rect.top < mine.bottom - 0.5 && mine.top < rect.bottom - 0.5).map(({ name: other }) => other);
    }, crowd.carried);
    check('a_drop_on_a_crowd_lies_on_nothing', lying.length === 0, lying);
  }

  // Forty more, as fast as they go: few reads, and right at the end.
  const startBurst = { ...reads };
  const burstOn = onScreen.slice(2, 6);
  const started = Date.now();
  const lanes = await Promise.all(burstOn.map(async (name) => {
    let at = 0;
    for (let turn = 0; turn < 10; turn++) ({ at } = await project('set', name, `instructions=BURST ${name} ${turn} END`));
    return at;
  }));
  const ended = Math.max(...lanes);
  const rightAt = await until('the burst read through', () => burstOn.map((name) => readAlone.get(name)), (now) => burstOn.every((name, at) => now[at]?.words === `BURST ${name} 9 END`));
  numbers.burst = { edits: burstOn.length * 10, seconds: seconds(ended - started), right_after_ms: Math.max(...rightAt.map((row) => row.at)) - ended, whole: reads.whole - startBurst.whole, assets: reads.assets - startBurst.assets };
  check('a_burst_is_few_reads', numbers.burst.whole === 0 && numbers.burst.assets <= 40, numbers.burst);
  check('a_burst_right_in_a_second', numbers.burst.right_after_ms <= BAR.edit_shows_ms, { ...numbers.burst, bar: BAR.edit_shows_ms });

  memory.edits = calciferMb();
  numbers.calcifer_native.edits = await nativeMemory(base, folder);

  // Each local write at this size: a box made, an input set, a box moved, a box gone, out where nothing's looked at. The file is whole after each.
  const writes = { post: [], patch: [], move: [], delete: [] };
  const write = async (held, path, method, body, headers = {}) => {
    const from = performance.now();
    const answer = await fetch(`${base}/v1/projects/${id}${path}`, { method, body: JSON.stringify(body), headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json', ...headers } });
    if (!answer.ok) throw new Error(`${method} ${path}: ${answer.status} ${await answer.text()}`);
    held.push(performance.now() - from);
    return answer.json();
  };
  const made = [];
  for (let i = 0; i < 9; i++) made.push((await write(writes.post, '/assets', 'POST', { tool: 'cozy/edit', kind: 'video', name: `written ${i}`, at: { x: -9000 - i * 800, y: -9000 } }, { 'Idempotency-Key': `scale-${Date.now()}-${i}` })).name);
  for (const name of made) await write(writes.patch, `/assets/${name}`, 'PATCH', { fields: { description: 'A box in a big project.' } });
  for (const [i, name] of made.entries()) await write(writes.move, '/canvas', 'PATCH', { cards: { [name]: { x: -9000 - i * 800, y: -12000 } } });
  for (const name of made) await write(writes.delete, `/assets/${name}`, 'DELETE', {});
  numbers.write_ms = Object.fromEntries(Object.entries(writes).map(([what, took]) => [what, summary(took).median_ms]));
  numbers.write_disk = flushed(folder, join(folder, 'project.db'));
  // Hosted Windows prints these and isn't held to them: on one runner image and one write path a POST read 57 to 195 ms and a plain replace 11 to 142.
  // A slower write shows on Linux and macOS, which share that path, and `numbers` keeps what Windows read.
  if (process.env.RUNNER_OS !== 'Windows') check('a_write_is_under_the_bar', Object.values(numbers.write_ms).every((ms) => ms <= BAR.write_ms), { bar: BAR.write_ms, ...numbers.write_ms, ...numbers.write_disk });
  check('writes_leave_the_project_whole', (await status()).state.assets.length === ASSETS, 'assets count changed');

  // A render from the API, on the fixture tool, landing on the open canvas.
  if ((await (await fetch(`${base}/v1/status`, { headers: { Authorization: `Bearer ${token}` } })).json()).local_usable && !process.env.ALONE) {
    await cozy('--json', 'run', `${TOOL}/still`, 'prompt=primed', '--on', 'local');
    await project('add', `${TOOL}/still`, 'scale_shot', `at=${HOLE.col * 300},${HOLE.row * 320},200`, 'instructions=A landing at scale');
    await box('scale_shot').waitFor();
    const landing = within('its picture', page.evaluate((name) => window.pictureOf(name), 'scale_shot'), 300_000);
    const render = await project('render', 'scale_shot', '--on', 'local');
    numbers.render_command_s = seconds(render.ms);
    numbers.landing_shows_ms = (await landing) - render.at;
    check('a_landing_shows_live', numbers.landing_shows_ms <= BAR.landing_shows_ms, { ms: numbers.landing_shows_ms, bar: BAR.landing_shows_ms });

    // Ten renders asked at once, each its own ask, with the project this size: what a read costs while they're owed.
    const names = Array.from({ length: 10 }, (_, at) => `queued_${at}`);
    for (const name of names) await project('add', `${TOOL}/still`, name, `instructions=${name}`);
    for (const name of names) await project('render', name, '--on', 'local', '--queue');
    const owed = await status();
    numbers.asks = { status_s: seconds(owed.ms), queue: owed.state.queue?.length ?? 0, db_mb: dbMb() };
    check('a_read_with_asks_owed_is_quick', numbers.asks.status_s <= BAR.asks_status_s, { ...numbers.asks, bar: BAR.asks_status_s });
    const cancelling = await project('cancel', ...names);
    numbers.asks.cancel_s = seconds(cancelling.ms);
    memory.renders = calciferMb();
  }

  // The whole of it fitted, and panned. What the page's own thread did in it says whether a slow fit is the page's work or drawing it.
  const fitting = Date.now();
  const unfitted = await worked();
  await page.keyboard.press('f');
  await until('fitted, its frames short', shortFrames, (p95) => p95 < 200, 180_000);
  numbers.fit_all_s = seconds(Date.now() - fitting);
  const fitted = await worked();
  numbers.fit_work_ms = Object.fromEntries(['TaskDuration', 'ScriptDuration', 'LayoutDuration', 'RecalcStyleDuration'].map((kind) => [kind.replace('Duration', '').toLowerCase(), Math.round(fitted[kind] - unfitted[kind])]));
  check('the_whole_of_it_fits', numbers.fit_all_s <= BAR.fit_all_s, { s: numbers.fit_all_s, bar: BAR.fit_all_s, page_thread_ms: numbers.fit_work_ms });
  numbers.mounted_fitted = await page.locator('.react-flow__node').count();
  numbers.pan_fitted = summary(await panning(3000));
  if (numbers.fit_all_s > BAR.fit_all_s) numbers.fitted_profile = await profiled(2000);
  await shot('fitted');

  memory.fitted = calciferMb();
  numbers.calcifer_native.fitted = await nativeMemory(base, folder);
  check('calcifers_memory', memory.edits <= BARS.calcifer_mb, memory);
  check('calcifers_memory_with_asks', memory.fitted <= BARS.calcifer_asks_mb, memory);
  rmSync(smallFolder, { recursive: true, force: true });
  rmSync(keying, { recursive: true, force: true });
  numbers.page_heap_mb = await page.evaluate(() => Math.round(performance.memory.usedJSHeapSize / 1048576));
  check('the_pages_memory', numbers.page_heap_mb <= BARS.page_heap_mb, numbers.page_heap_mb);

  check('never_reloaded', await page.evaluate(() => window.notReloaded === true));
  check('csp_holds', (await page.evaluate(() => window.violations)).length === 0, await page.evaluate(() => window.violations));
  // A box deleted is read once more, which is how the page hears it's gone. Nothing else should be.
  check('only_what_was_deleted_reads_as_gone', [...readGone].every((name) => made.includes(name)), [...readGone]);
  check('no_page_errors', errors.length === 0, errors.slice(0, 8));
  await browser.close();

  // Closed once nothing reads it, the browser gone: what Calcifer built and dropped for 5,000 assets goes back, in a wait
  // as long for this runner as CLOSED_MS is for a fast one.
  numbers.closed_wait_ms = Math.round(CLOSED_MS * SPEED.factor);
  await new Promise((done) => setTimeout(done, numbers.closed_wait_ms));
  memory.closed = calciferMb();
  numbers.calcifer_mappings.closed = residentByMapping();
  numbers.calcifer_native.closed = await nativeMemory(base, folder);
  const closedMetric = process.platform === 'darwin' ? 'physical_footprint' : 'rss';
  numbers.calcifer_closed_memory_metric = closedMetric;
  check('calcifer_gives_back_a_closed_project', closedProjectMemory(process.platform, memory, numbers.calcifer_native, BARS.calcifer_kept_mb),
    process.platform === 'darwin' ? { metric: closedMetric, budget_mb: BARS.calcifer_kept_mb, rss_mb: memory, before: numbers.calcifer_native.before, closed: numbers.calcifer_native.closed } : memory);
  // Again at the end: how steady the runner's speed held.
  seen.runner_after = runnerSpeed();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch(died);

async function died(error) {
  console.error(error);
  await lastLook();
  console.log(JSON.stringify({ failures, seen, during }, null, 2));
  process.exit(2);
}
