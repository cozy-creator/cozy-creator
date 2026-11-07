// Thousands of assets (Paul expects them): a generated 5,000-asset project in the file format as it stands, a real Calcifer, the real CLI, a real
// Chrome. How long `cozy project status` takes, opening it in the page, one edit to show, what's read for it, a box let go on the crowd by hand,
// a render landing, panning, memory.
// The bars are what a person would feel, in BARS. What each took is seen.scale.
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
const { calcifer, signIn, STEP_MS, lastLook, lookAt, png, noStall, renderOf, until } = require('./page');

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
  page_heap_mb: 512,
};

const sha = (bytes) => createHash('sha256').update(bytes).digest('hex');

/** The project: pictures approved as they are, and shots of them, each with two versions, as files. `inputs`, once known, makes them current. */
function generated(folder, inputs, assetCount = ASSETS, pictures = PICTURES) {
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
  const lines = ['version: 4', 'assets:'];
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
    lines.push(`  pic_${i}: {approved: "${hex.slice(0, 12)}"}`);
  }
  const tool = 'birefnet/remove_background';
  for (let i = 0; i < assetCount - pictures; i++) {
    const name = `shot_${i}`;
    lines.push(`  ${name}:`, `    tool: ${tool}`, `    instructions: ${words}<pic_${i % pictures}> and <pic_${(i * 7) % pictures}>.`, '    aspect_ratio: "16:9"');
    const versions = [1, 2].map((number) => {
      const hex = hash(`${name} ${number}`);
      file(hex, 'wide');
      return { number, inputs: inputs?.[name] ?? 'not yet', files: { image: `sha256:${hex}` }, seed: i * number, run: 1000 + i * 2 + number, execution_ms: 1234 };
    });
    assets[`shot ${i}`] = { name, kind: 'image', versions, numbered: 2 };
  }
  replaceFile(join(folder, '.measured.json'), JSON.stringify(measured));
  replaceFile(join(folder, 'project.yaml'), `${lines.join('\n')}\n`);
  replaceFile(join(folder, 'project.lock'), JSON.stringify({ format: 3, assets, makes: { [tool]: 'image' } }));
  // A grid, each where it'd sit if a person had placed it.
  const side = Math.ceil(Math.sqrt(assetCount));
  const rows = ['# Where cards sit. Only the page reads this. Nothing here is an input.', 'version: 1', 'cards:'];
  // A place left empty near the top: a grid has no other room, and a box that lands on another is moved off it.
  const hole = side * HOLE.row + HOLE.col;
  const card = (name, at) => {
    const place = at < hole ? at : at + 1;
    rows.push(`  ${name}: {x: ${(place % side) * 300}, y: ${Math.floor(place / side) * 320}, w: 260}`);
  };
  for (let i = 0; i < assetCount - pictures; i++) card(`shot_${i}`, i);
  for (let i = 0; i < pictures; i++) card(`pic_${i}`, assetCount - pictures + i);
  replaceFile(join(folder, 'project.canvas.yaml'), `${rows.join('\n')}\n`);
  for (const shape of Object.keys(seeds)) rmSync(seedOf(shape));
}

/** A wait that ends, so a cell that can't see what it waits for says so. */
const within = (what, waiting, ms = STEP_MS) => Promise.race([waiting, new Promise((_, fail) => { setTimeout(() => fail(new Error(`${what}: not within ${ms} ms`)), ms).unref(); })]);

function replaceFile(at, text) {
  writeFileSync(`${at}.part`, text);
  renameSync(`${at}.part`, at);
}

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
  const yaml_bytes = statSync(like).size;
  return { flush_4k_ms: flush(4096), flush_yaml_ms: flush(yaml_bytes), replace_yaml_ms: replace(yaml_bytes), yaml_bytes };
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

(async () => {
  const base = calcifer();
  const folder = join(homedir(), '.cozy', 'projects', `web-scale-${Date.now()}`);
  mkdirSync(folder, { recursive: true });
  const numbers = {};
  seen.scale = numbers;
  seen.bars = BARS;

  const cozy = (...words) => new Promise((done, fail) => {
    if (process.env.ARGV_LOG) appendFileSync(process.env.ARGV_LOG, `${words.map((word) => `${word}\x1f`).join('')}\x1e`);
    const from = Date.now();
    execFile(COZY, words, { cwd: folder, encoding: 'utf8', maxBuffer: 1 << 28 }, (error, stdout, stderr) => {
      const at = Date.now();
      if (error) fail(new Error(`cozy ${words.join(' ').slice(0, 300)}: ${error.message}: ${stdout.slice(0, 500)}${stderr.slice(0, 500)}`));
      else done({ stdout, stderr, at, ms: at - from });
    });
  });
  const project = (...words) => cozy('project', '-p', folder, ...words);
  const status = async () => {
    const answer = await cozy('--json', '--full', 'project', '-p', folder, 'status');
    return { state: JSON.parse(answer.stdout), ms: answer.ms };
  };
  const seconds = (ms) => +(ms / 1000).toFixed(2);

  const generating = Date.now();
  generated(folder);
  numbers.generate_s = seconds(Date.now() - generating);

  // The first read of it, then with each shot's real key, so they stand as a person's would.
  const first = await status();
  numbers.status_first_s = seconds(first.ms);
  check('all_are_there', first.state.assets.length === ASSETS, first.state.assets.length);
  seen.project = { id: first.state.id, folder };
  const id = first.state.id;
  const inputs = Object.fromEntries(first.state.assets.filter((asset) => asset.name.startsWith('shot_')).map((asset) => [asset.name, asset.inputs]));
  generated(folder, inputs);
  const read = await status();
  numbers.status_s = seconds(read.ms);
  numbers.lock_mb = +(readFileSync(join(folder, 'project.lock')).length / 1048576).toFixed(1);
  numbers.yaml_mb = +(readFileSync(join(folder, 'project.yaml')).length / 1048576).toFixed(1);
  numbers.status_answer_mb = +(JSON.stringify(read.state).length / 1048576).toFixed(1);
  const standing = read.state.assets.reduce((held, asset) => ({ ...held, [asset.standing]: (held[asset.standing] ?? 0) + 1 }), {});
  numbers.standing = standing;
  check('they_stand_as_a_persons_would', standing.current === ASSETS, standing);
  check('status_is_quick', numbers.status_s <= BARS.status_s, numbers.status_s);
  const again = await status();
  numbers.status_again_s = seconds(again.ms);
  check('status_again_is_quicker', numbers.status_again_s <= BARS.status_again_s, numbers.status_again_s);
  const memory = { read: calciferMb() };
  numbers.calcifer_memory_mb = memory;

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
    if (url.pathname === `/v1/projects/${id}`) reads[url.searchParams.get('assets') === 'false' ? 'heads' : 'whole'] += 1;
    if (url.pathname.startsWith(`/v1/projects/${id}/assets/`)) reads.assets += 1;
  });
  page.on('response', (answer) => {
    if (answer.request().method() !== 'GET') return;
    const { pathname } = new URL(answer.url());
    if (pathname === `/v1/projects/${id}`) answer.body().then((body) => { reads.whole_bytes = Math.max(reads.whole_bytes, body.length); }, () => {});
    if (pathname.startsWith(`/v1/projects/${id}/assets/`)) answer.json().then(({ asset }) => {
      if (asset) readAlone.set(asset.name, { words: asset.fields?.instructions, at: Date.now() });
      else readGone.add(decodeURIComponent(pathname.split('/').at(-1)));
    }, () => {});
  });
  const box = (name) => page.locator('.react-flow__node').filter({ has: page.locator(`.nm:text-is("${name}")`) });
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png`, timeout: 60_000 }).catch(() => {}) : Promise.resolve());

  await signIn(page);
  await page.waitForSelector('.top');
  const { token } = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session')));
  await page.evaluate(() => { window.notReloaded = true; });

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
  const measuredPan = async () => {
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
  const small = JSON.parse((await cozy('--json', '--full', 'project', '-p', smallFolder, 'status')).stdout);
  await page.goto(`${base}/#/projects/${small.id}`);
  await page.waitForSelector('.react-flow__node .nm');
  numbers.pan_small = await measuredPan();
  await fetch(`${base}/v1/projects/${small.id}`, { method: 'DELETE', headers: { Authorization: `Bearer ${token}` } });

  // Opening it: boxes on screen, then the page settled, its frames short.
  const opening = Date.now();
  await page.goto(`${base}/#/projects/${id}`);
  await page.waitForSelector('.nm:text-is("shot_0")', { timeout: 120_000 });
  numbers.open_boxes_s = seconds(Date.now() - opening);
  check('it_opens_quickly', numbers.open_boxes_s <= BARS.open_boxes_s, numbers.open_boxes_s);
  await until('its frames short', shortFrames, (p95) => p95 < 100, 180_000);
  numbers.open_settled_s = seconds(Date.now() - opening);
  check('it_settles_quickly', numbers.open_settled_s <= BARS.open_settled_s, numbers.open_settled_s);
  memory.open = calciferMb();
  numbers.mounted_boxes = await page.locator('.react-flow__node').count();
  numbers.pan_big = await measuredPan();
  const allowed = numbers.pan_small.added_ms_per_frame * BARS.pan_work_times_small + BARS.pan_work_slack_ms;
  check('size_adds_nothing_to_a_pan', numbers.pan_big.added_ms_per_frame <= allowed, { allowed, small: numbers.pan_small, big: numbers.pan_big });
  // Over the bar it says what's spending it, then and there.
  const quiet = numbers.pan_big.rest_ms_per_s <= BARS.rest_ms_per_s;
  if (!quiet) numbers.rest_profile = await profiled(REST_MS);
  check('a_page_at_rest_is_quiet', quiet, { bar: BARS.rest_ms_per_s, big: numbers.pan_big.rest_ms_per_s, kinds: numbers.pan_big.rest_kinds, small: numbers.pan_small.rest_ms_per_s, profile: numbers.rest_profile });
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
    await page.goto(`${base}/#/projects/${id}/assets/${encodeURIComponent(name)}`);
    const here = page.locator(`.card.here[data-asset="${name}"]`);
    await here.waitFor();
    // On a take an image tool made, its words are its next take's, behind its +.
    await renderOf(page, name);
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
  check('one_edit_shows_live', numbers.edit_shows_ms <= BARS.edit_shows_ms, numbers.edit_shows_ms);
  await page.waitForTimeout(600);
  numbers.edit_reads = { whole: reads.whole - before.whole, heads: reads.heads - before.heads, assets: reads.assets - before.assets };
  check('one_edit_reads_only_it', numbers.edit_reads.whole === 0 && numbers.edit_reads.assets >= 1 && numbers.edit_reads.assets <= 3, numbers.edit_reads);
  check('one_edit_read_is_right', readAlone.get(edited)?.words === 'ONE EDIT FROM THE CLI', readAlone.get(edited));
  // The middle of five: one alone on a Windows runner has taken 2.3 s, in a run whose edits averaged 0.8 s, and the others are what a person gets.
  const sets = [edit.ms];
  for (let turn = 1; turn < 5; turn++) sets.push((await project('set', edited, `instructions=EDIT ${turn} FROM THE CLI`)).ms);
  numbers.set_commands_s = sets.map(seconds);
  numbers.set_command_s = seconds([...sets].sort((a, b) => a - b)[2]);
  check('a_command_is_quick', numbers.set_command_s <= BARS.command_s, numbers.set_commands_s);

  // A hand's save, the same way.
  const byHand = onScreen[1] ?? edited;
  await openOn(byHand);
  const shownHand = says(byHand, 'ONE EDIT BY HAND');
  const text = readFileSync(join(folder, 'project.yaml'), 'utf8');
  const lineAt = text.indexOf(`  ${byHand}:\n`);
  const instructionsAt = text.indexOf('    instructions: ', lineAt);
  replaceFile(join(folder, 'project.yaml'), `${text.slice(0, instructionsAt)}    instructions: ONE EDIT BY HAND${text.slice(text.indexOf('\n', instructionsAt))}`);
  const savedAt = Date.now();
  numbers.hand_shows_ms = (await shownHand) - savedAt;
  check('a_hand_shows_live', numbers.hand_shows_ms <= BARS.edit_shows_ms, numbers.hand_shows_ms);
  await page.goto(`${base}/#/projects/${id}`);
  await page.locator('.card.here').waitFor({ state: 'detached' });
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
  check('a_move_shows_live', numbers.move_shows_ms <= BARS.edit_shows_ms, numbers.move_shows_ms);
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
    check('a_drop_on_a_crowd_gives_way_at_once', numbers.drop_ms <= BARS.drop_ms, numbers.drop_ms);
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
  check('a_burst_right_in_a_second', numbers.burst.right_after_ms <= BARS.edit_shows_ms, numbers.burst);

  memory.edits = calciferMb();

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
  numbers.write_disk = flushed(folder, join(folder, 'project.yaml'));
  // Hosted Windows prints these and isn't held to them: on one runner image and one write path a POST read 57 to 195 ms and a plain replace 11 to 142.
  // A slower write shows on Linux and macOS, which share that path, and `numbers` keeps what Windows read.
  if (process.env.RUNNER_OS !== 'Windows') check('a_write_is_under_the_bar', Object.values(numbers.write_ms).every((ms) => ms <= BARS.write_ms), { bar: BARS.write_ms, ...numbers.write_ms, ...numbers.write_disk });
  check('writes_leave_the_project_whole', (await status()).state.assets.length === ASSETS, 'assets count changed');

  // A render from the CLI, on the fixture tool, landing on the open canvas.
  if ((await (await fetch(`${base}/v1/status`, { headers: { Authorization: `Bearer ${token}` } })).json()).local_usable && !process.env.ALONE) {
    await cozy('--json', 'run', `${TOOL}/still`, 'prompt=primed', '--on', 'local');
    await project('add', `${TOOL}/still`, 'scale_shot', `at=${HOLE.col * 300},${HOLE.row * 320},200`, 'instructions=A landing at scale');
    await box('scale_shot').waitFor();
    const landing = within('its picture', page.evaluate((name) => window.pictureOf(name), 'scale_shot'), 300_000);
    const render = await project('render', 'scale_shot', '--on', 'local');
    numbers.render_command_s = seconds(render.ms);
    numbers.landing_shows_ms = (await landing) - render.at;
    check('a_landing_shows_live', numbers.landing_shows_ms <= BARS.landing_shows_ms, numbers.landing_shows_ms);

    // Ten renders asked at once, each its own ask, with the project this size: what a read costs while they're owed.
    const names = Array.from({ length: 10 }, (_, at) => `queued_${at}`);
    for (const name of names) await project('add', `${TOOL}/still`, name, `instructions=${name}`);
    for (const name of names) await project('render', name, '--on', 'local', '--queue');
    const owed = await status();
    numbers.asks = { status_s: seconds(owed.ms), queue: owed.state.queue?.length ?? 0, lock_mb: +(readFileSync(join(folder, 'project.lock')).length / 1048576).toFixed(1) };
    check('a_read_with_asks_owed_is_quick', numbers.asks.status_s <= BARS.asks_status_s, numbers.asks);
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
  check('the_whole_of_it_fits', numbers.fit_all_s <= BARS.fit_all_s, { s: numbers.fit_all_s, page_thread_ms: numbers.fit_work_ms });
  numbers.mounted_fitted = await page.locator('.react-flow__node').count();
  numbers.pan_fitted = summary(await panning(3000));
  if (numbers.fit_all_s > BARS.fit_all_s) numbers.fitted_profile = await profiled(2000);
  await shot('fitted');

  memory.fitted = calciferMb();
  check('calcifers_memory', memory.edits <= BARS.calcifer_mb, memory);
  check('calcifers_memory_with_asks', memory.fitted <= BARS.calcifer_asks_mb, memory);
  rmSync(smallFolder, { recursive: true, force: true });
  numbers.page_heap_mb = await page.evaluate(() => Math.round(performance.memory.usedJSHeapSize / 1048576));
  check('the_pages_memory', numbers.page_heap_mb <= BARS.page_heap_mb, numbers.page_heap_mb);

  check('never_reloaded', await page.evaluate(() => window.notReloaded === true));
  check('csp_holds', (await page.evaluate(() => window.violations)).length === 0, await page.evaluate(() => window.violations));
  // A box deleted is read once more, which is how the page hears it's gone. Nothing else should be.
  check('only_what_was_deleted_reads_as_gone', [...readGone].every((name) => made.includes(name)), [...readGone]);
  check('no_page_errors', errors.length === 0, errors.slice(0, 8));
  console.log(JSON.stringify({ failures, seen }, null, 2));
  await browser.close();
  process.exit(failures.length ? 1 : 0);
})().catch(died);

async function died(error) {
  console.error(error);
  await lastLook();
  console.log(JSON.stringify({ failures, seen, during }, null, 2));
  process.exit(2);
}
