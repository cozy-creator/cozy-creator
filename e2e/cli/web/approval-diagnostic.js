// The CLI and the page are one project (Paul, 11-04: "edits to the same project with a CLI will appear almost immediately in the webui; the
// user should see the canvas being edited live"), in a real browser on a real Calcifer, with the real CLI, and the canvas never reloaded. From
// each command's return to it showing, or a hand's save: add, text, approve, delete, put back, move, rename. A new box finds a free place on
// screen and lands. A render from the CLI shows progress on its card and lands, on the fixture tool. What's typed and unsaved stays when the
// CLI changes that field, and the page shows the conflict. What the page does reads back in the CLI. 200 edits as fast as they go: right at the
// end, shown as it goes, only what changed read, frames sampled. And the stream made to fall behind for real, so `resync` runs: nothing's lost.
//   node live.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $COZY: the CLI; $SHOT: where to keep its screenshots; $ALONE: on a shared
// computer, so it runs nothing; $ARGV_LOG: where each cozy call is logged for coverage. Prints what it saw as JSON and exits 1 if anything failed.
const { chromium } = require(process.env.PLAYWRIGHT);
const { execFile, execFileSync } = require('child_process');
const { appendFileSync, mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { calcifer, signIn, STEP_MS, boxOf, closeBox, lastLook, lookAt, noStall, openBox, png, until, pointAt, importYaml, yamlOf } = require('./page');

const failures = [];
const seen = {};
noStall(seen, died);
let during = 'the start';
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  during = name;
  if (!ok) failures.push(name);
}

// "About a second": from a command's return, or a file's save, to the page showing it. The stream carries it, and a look at the files is 250 ms.
const LIVE_MS = 1200;
const FIRST_RUN_MS = 300_000;
// Of the page's thread through a burst: read 0.12 on Windows, 0.31 on Linux, 0.56 on macOS. This leaves a quarter for drawing and input, short of the whole
// thread that's thrashing. Frames are reported, not judged: a Mac runner has no GPU and takes 50 to 66 ms a frame on ten boxes. Being right in a second is `LIVE_MS`.
const BURST_THREAD = 0.75;
const COZY = process.env.COZY;
const TOOL = join(__dirname, '..', 'project', 'tool.py');
// The stream's ring holds this many of what projects say.
const RING = 1024;

/** What the page is watched by, before it loads: its CSP, what `GET /v1/events` says, and when something shows, by its own frames. */
const WATCH = () => {
  window.violations = [];
  document.addEventListener('securitypolicyviolation', (e) => window.violations.push(`${e.violatedDirective}: ${e.blockedURI}`));
  window.resyncs = [];
  const trace = window.approvalTrace = { events: [], media: [], loads: [] };
  const stash = (list, value) => { if (list.length < 120) list.push(value); };
  const fetched = window.fetch.bind(window);
  window.fetch = async (input, init) => {
    const answer = await fetched(input, init);
    if (!String(input.url ?? input).includes('/v1/events') || !answer.body) return answer;
    const [mine, theirs] = answer.body.tee();
    (async () => {
      const reader = mine.pipeThrough(new TextDecoderStream()).getReader();
      let rest = '';
      for (;;) {
        const { value, done } = await reader.read();
        if (done) return;
        rest += value;
        for (let end = rest.indexOf('\n\n'); end >= 0; end = rest.indexOf('\n\n')) {
          const frame = rest.slice(0, end);
          try {
            const event = JSON.parse(frame.split('\n').filter(line => line.startsWith('data: ')).map(line => line.slice(6)).join('\n'));
            if (event.type === 'project.changed' && event.id === trace.project) {
              stash(trace.events, { at: Date.now(), cursor: event.cursor?.sequence ?? frame.match(/^id: (.*)$/m)?.[1], assets: event.assets, head: event.head });
            }
          } catch { /* Existing tee can receive an SSE comment or hello frame. */ }
          if (rest.slice(0, end).includes('"type":"resync"')) window.resyncs.push(Date.now());
          rest = rest.slice(end + 2);
        }
      }
    })().catch(() => {});
    return new Response(theirs, { status: answer.status, statusText: answer.statusText, headers: answer.headers });
  };
  const box = (name) => [...document.querySelectorAll('.react-flow__node')].find((node) => node.querySelector('.nm')?.textContent === name);
  window.boxOf = box;
  window.startApprovalObservation = (project) => {
    trace.project = project;
    const mediaOf = (name) => box(name)?.querySelector('.media');
    const last = new Map();
    const names = ['live_pic', 'live_lift', 'live_tool'];
    const observe = () => {
      for (const name of names) {
        const media = mediaOf(name);
        const img = media?.querySelector('img');
        const state = { exists: Boolean(media), blank: media?.classList.contains('blank'), img: Boolean(img) };
        const signature = JSON.stringify(state);
        if (last.get(name) !== signature) {
          last.set(name, signature);
          stash(trace.media, { name, at: Date.now(), ...state });
        }
      }
    };
    for (const event of ['load', 'error']) document.addEventListener(event, (e) => {
      if (!(e.target instanceof HTMLImageElement)) return;
      for (const name of names) if (mediaOf(name)?.contains(e.target)) {
        stash(trace.loads, { name, event, at: Date.now() });
      }
    }, true);
    const canvas = document.querySelector('.react-flow__nodes');
    if (!canvas) throw new Error('the observation needs the project canvas');
    new MutationObserver(observe).observe(canvas, { subtree: true, childList: true, attributes: true, attributeFilter: ['class', 'src'] });
    observe();
  };
  /** What the pointer did and where the page was, for a box that doesn't open. First at the window, so it hears a click d3-drag then swallows. */
  window.clicks = [];
  for (const type of ['pointerdown', 'pointerup', 'click']) {
    window.addEventListener(type, ({ target }) => window.clicks.push(`${type} ${target.tagName}.${target.className} ${location.hash}`.slice(0, 160)), true);
  }
  /** All it says, at rest or worked on where it stands. */
  const wordsOf = (name) => box(name)?.textContent ?? '';
  const placeOf = (node) => {
    const m = new DOMMatrix(getComputedStyle(node).transform);
    return { x: m.e, y: m.f };
  };
  const holds = (spec) => {
    if (spec.all) return spec.all.every(holds);
    if (spec.gone) return !box(spec.gone);
    const node = box(spec.name);
    if (!node) return false;
    if (spec.has !== undefined && !wordsOf(spec.name).includes(spec.has)) return false;
    if (spec.lacks !== undefined && wordsOf(spec.name).includes(spec.lacks)) return false;
    if (spec.x !== undefined && (Math.abs(placeOf(node).x - spec.x) > 0.5 || Math.abs(placeOf(node).y - spec.y) > 0.5)) return false;
    if (spec.w !== undefined && Math.abs(node.offsetWidth - spec.w) > 0.5) return false;
    if (spec.dot && !node.querySelector(`.dot.${spec.dot}`)) return false;
    if (spec.picture !== undefined && Boolean(node.querySelector('.media img')) !== spec.picture) return false;
    if (spec.conflict && !wordsOf(spec.name).includes('The file changed under this')) return false;
    return true;
  };
  /** When `spec` first holds, by the page's own frames, and whether its box was landing then. */
  window.shown = (spec) => new Promise((done) => {
    const look = () => {
      if (!holds(spec)) return requestAnimationFrame(look);
      const node = spec.name && box(spec.name);
      done({ when: Date.now(), landing: Boolean(node?.querySelector('.card.landing')) });
    };
    look();
  });
  /** What of `spec` doesn't hold, as the page has it. */
  window.unshown = (spec) => (spec.all ?? [spec]).filter((one) => !holds(one)).map((one) => {
    const node = box(one.name ?? one.gone);
    return { wanted: one, drawn: Boolean(node), text: node?.textContent.slice(0, 160), place: node && placeOf(node) };
  });
  /** Each place its progress reached, until it has none. */
  window.progressOf = (name) => new Promise((done) => {
    const places = [];
    window.progressSaid = [];
    const look = () => {
      const bar = box(name)?.querySelector('.prog');
      if (bar) {
        const place = bar.querySelector('i')?.style.transform || 'unknown';
        if (places.at(-1) !== place) places.push(place);
        const says = bar.querySelector('b')?.textContent;
        if (says && window.progressSaid.at(-1) !== says) window.progressSaid.push(says);
      }
      if (!bar && places.length) done(places);
      else requestAnimationFrame(look);
    };
    look();
  });
  /** Frame times, and each text `name` showed, until `window.stopWatching`. */
  window.watching = (name) => {
    const [frames, texts] = [[], []];
    let last = performance.now();
    const step = (now) => {
      frames.push(now - last);
      last = now;
      const text = box(name)?.textContent;
      if (text && texts.at(-1) !== text) texts.push(text);
      if (window.stopWatching) window.stopWatching({ frames, texts });
      else requestAnimationFrame(step);
    };
    requestAnimationFrame(step);
  };
};

function summary(frames) {
  const sorted = [...frames].sort((a, b) => a - b);
  return { median_ms: +sorted[sorted.length >> 1].toFixed(1), p95_ms: +sorted[Math.floor(sorted.length * 0.95)].toFixed(1), worst_ms: +sorted.at(-1).toFixed(1), frames: sorted.length };
}

function replaceFile(at, text) {
  writeFileSync(`${at}.part`, text);
  renameSync(`${at}.part`, at);
}

/** One asset's lines of a project.yaml, as the file holds them. */
function linesOf(text, name) {
  const lines = text.split('\n');
  const at = lines.findIndex((line) => line.startsWith(`  ${name}:`));
  if (at < 0) return null;
  const end = lines.findIndex((line, n) => n > at && /^ {0,2}\S/.test(line));
  return lines.slice(at, end < 0 ? undefined : end).join('\n');
}

(async () => {
  const base = calcifer();
  const browser = await chromium.launch({ executablePath: process.env.CHROME, headless: true });
  const context = await browser.newContext({ viewport: { width: 1800, height: 1200 } });
  const page = await context.newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  seen.page_errors = errors;
  const refused = [];
  seen.refused = refused;
  page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
  page.on('console', (m) => { if (m.type() === 'error' && !/status of 4\d\d|net::ERR_/.test(m.text())) errors.push(`console: ${m.text()}`); });
  page.on('response', (answer) => {
    const url = new URL(answer.url());
    if (answer.status() < 400) return;
    const refusal = { status: answer.status(), method: answer.request().method(), url: url.pathname, during };
    refused.push(refusal);
    answer.json().then(({ error }) => Object.assign(refusal, { code: error?.code, message: error?.message })).catch(() => {});
  });
  await page.addInitScript(WATCH);
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());

  await signIn(page);
  await page.waitForSelector('.top');
  const { token } = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session')));
  const api = async (path, init = {}) => {
    const answer = await fetch(base + path, { ...init, headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json', ...init.headers } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  };
  const local = (await api('/v1/status')).body.local_usable && !process.env.ALONE;
  seen.ran = local;

  await page.goto(`${base}/#/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-live-${Date.now()}`);
  await page.click('form button[type=submit]');
  await page.waitForURL(/#\/projects\/[^/]+$/);
  const id = page.url().match(/#\/projects\/([^/]+)/)[1];
  const folder = (await api(`/v1/projects/${id}`)).body.path;
  seen.project = { id, folder };
  const yaml = () => yamlOf(folder);
  const scratch = mkdtempSync(join(tmpdir(), 'web-live-'));

  await page.evaluate(() => { window.notReloaded = true; });
  // Off where the form's button was: a box arriving under the pointer opens, and an Edit open says its words behind More.
  await page.mouse.move(2, 500);
  let documents = 0;
  let streams = 0;
  const reads = { project: 0, heads: 0, canvas: 0, assets: 0 };
  context.on('request', (request) => {
    const path = new URL(request.url()).pathname;
    if (request.resourceType() === 'document') documents += 1;
    if (path === '/v1/events') streams += 1;
    if (request.method() === 'GET' && path === `/v1/projects/${id}`) reads[new URL(request.url()).searchParams.get('assets') === 'false' ? 'heads' : 'project'] += 1;
    if (request.method() === 'GET' && path === `/v1/projects/${id}/canvas`) reads.canvas += 1;
    if (request.method() === 'GET' && path.startsWith(`/v1/projects/${id}/assets/`)) reads.assets += 1;
  });

  /** A call of the real CLI, and when it returned. */
  /** `waiting`, or what `said` says after `ms`. */
  const bounded = (waiting, ms, said) => {
    let timer;
    const late = new Promise((_, fail) => { timer = setTimeout(async () => fail(new Error(await said())), ms); });
    return Promise.race([waiting, late]).finally(() => clearTimeout(timer));
  };
  /** When `spec` first shows. One that never does says what the page has of it, and what Calcifer does, so it's plain which of them is behind. */
  const shows = (spec, ms = FIRST_RUN_MS) => bounded(page.evaluate((asked) => window.shown(asked), spec), ms, async () => {
    const page_has = await page.evaluate((asked) => window.unshown(asked), spec);
    const canvas = await rows();
    const calcifer = await Promise.all(page_has.slice(0, 4).map(async ({ wanted }) => {
      const name = wanted.name ?? wanted.gone;
      const { status: answered, body } = await api(`/v1/projects/${id}/assets/${name}`);
      return { name, answered, fields: body?.asset?.fields, row: canvas[name] };
    }));
    return `never showed ${JSON.stringify(spec)}, of it ${page_has.length} not: ${JSON.stringify({ page_has: page_has.slice(0, 4), calcifer, reads })}`;
  });
  const cozy = (...words) => new Promise((done, fail) => {
    if (process.env.ARGV_LOG) appendFileSync(process.env.ARGV_LOG, `${words.map((word) => `${word}\x1f`).join('')}\x1e`);
    execFile(COZY, words, { cwd: folder, env: { ...process.env, COZY_PROJECT: folder }, encoding: 'utf8', maxBuffer: 1 << 26, timeout: FIRST_RUN_MS }, (error, stdout, stderr) => {
      const at = Date.now();
      if (error) fail(new Error(`cozy ${words.join(' ')}: ${error.message}: ${stdout}${stderr}`));
      else done({ stdout, stderr, at });
    });
  });
  const project = (...words) => cozy('project', '-p', folder, ...words);
  const asJson = async (...words) => JSON.parse((await cozy('--json', '--full', 'project', '-p', folder, ...words)).stdout);
  const status = () => asJson('status');
  const rows = async () => (await api(`/v1/projects/${id}/canvas`)).body.cards;
  const held = async (name) => (await status()).assets.find((asset) => asset.name === name);

  const times = {};
  seen.times_ms = times;
  /** `act`, and how long from its return until `spec` showed. */
  const timed = async (label, spec, act, within = LIVE_MS) => {
    const showing = shows(spec);
    showing.catch(() => {});
    const done = await act();
    const { when, landing } = await showing;
    const ms = when - done.at;
    times[label] = ms;
    check(`${label}_live`, ms <= within, { ms, within });
    return { ...done, ms, landing };
  };
  /** A save by hand. */
  const saved = (write) => {
    write();
    return { at: Date.now() };
  };
  const box = (name) => boxOf(page, name);
  const away = async () => {
    await closeBox(page);
    await page.mouse.move(2, 500);
  };
  const open = (name) => openBox(page, name);
  /** A place on screen that's clear, as what lands on a box is moved off it: right of where the page puts what has none. */
  const spot = async () => {
    const boxes = Object.values((await rows()) ?? {});
    const clear = (x, y) => boxes.every((box) => x + 360 <= box.x || box.x + (box.w ?? 720) <= x || y + 400 <= box.y || box.y + 400 <= y);
    // Left of 1060: an Edit open is 720 wide, its X at its right, and the canvas never scrolls to reach it.
    for (let y = 90; ; y += 170) for (const x of [700, 1040]) if (clear(x, y)) return { x, y };
  };
  const overlaps = () => page.evaluate(() => {
    const rects = [...document.querySelectorAll('.react-flow__node')].map((node) => ({ name: node.querySelector('.nm')?.textContent, rect: node.getBoundingClientRect() }));
    const over = [];
    rects.forEach((a, i) => rects.slice(i + 1).forEach((b) => {
      if (a.rect.left < b.rect.right - 1 && b.rect.left < a.rect.right - 1 && a.rect.top < b.rect.bottom - 1 && b.rect.top < a.rect.bottom - 1) over.push(`${a.name}/${b.name}`);
    }));
    return over;
  });
  const onScreen = (name) => page.evaluate((asked) => {
    const rect = window.boxOf(asked)?.getBoundingClientRect();
    return Boolean(rect) && rect.left >= 0 && rect.top >= 0 && rect.right <= innerWidth && rect.bottom <= innerHeight;
  }, name);

  // A command adds it: it shows by itself where it was told, and lands.
  await page.locator('.empty-canvas').waitFor();
  let place = await spot();
  const added = await timed('add', { name: 'live_a', x: place.x, y: place.y }, () => project('add', 'cozy/edit', 'live_a', `at=${place.x},${place.y},260`, 'description=Words from the CLI'));
  check('add_prints_its_name', added.stdout.trim() === 'live_a', added.stdout);
  check('add_lands', added.landing);
  check('add_in_the_status', (await held('live_a'))?.tool === 'cozy/edit');

  // Its text.
  await timed('set_text', { name: 'live_a', has: 'Second words' }, () => project('set', 'live_a', 'description=Second words'));
  check('set_in_the_status', (await held('live_a')).fields?.description === 'Second words');

  // A picture.
  const requests = [];
  const bodyChecks = [];
  const observeRequest = (step, request, status) => {
    const url = new URL(request.url());
    const project = `/v1/projects/${id}`;
    if (url.pathname !== project && !url.pathname.startsWith(`${project}/assets/`) && !url.pathname.startsWith(`${project}/thumbs/`)) return;
    if (requests.length < 160) requests.push({ step, at: Date.now(), method: request.method(), path: url.pathname, during, status });
  };
  context.on('request', request => observeRequest('start', request));
  context.on('requestfinished', request => observeRequest('finished', request));
  context.on('requestfailed', request => observeRequest('failed', request));
  context.on('response', answer => {
    observeRequest('headers', answer.request(), answer.status());
    if (answer.request().method() !== 'GET' || new URL(answer.url()).pathname !== `/v1/projects/${id}/assets/live_tool`) return;
    const headersAt = Date.now();
    bodyChecks.push(answer.json().then(body => {
      const asset = body.asset ?? body;
      if (requests.length < 160) requests.push({ step: 'live-tool-body', at: Date.now(), headersAt, pinned: asset.pinned, sha256: asset.sha256, exists: Boolean(asset) });
    }).catch(() => {}));
  });
  await page.evaluate(project => window.startApprovalObservation(project), id);
  const picture = join(scratch, 'Live Picture.png');
  writeFileSync(picture, png(77));
  const imported = await timed('import', { name: 'live_pic' }, () => cozy('--json', 'project', '-p', folder, 'import', picture, 'live_pic'));
  check('import_lands', imported.landing);
  const hash = JSON.parse(imported.stdout).sha256.replace('sha256:', '').slice(0, 12);

  // A file of the project's lifted out as an asset of its own, with its description.
  const lifted = await timed('lift_a_file', { name: 'live_lift' }, () => project('import', hash, 'live_lift', '--description', 'Lifted out'));
  check('lift_prints_its_name', lifted.stdout.trim() === 'live_lift', lifted.stdout);
  check('lift_keeps_its_words', (await held('live_lift'))?.fields?.description === 'Lifted out');

  // By hand, in place and by a rename over it, as editors save.
  await timed('hand_text_in_place', { name: 'live_a', has: 'Hand words one' }, () => saved(() => importYaml(folder, yaml().replace('Second words', 'Hand words one'))));
  await timed('hand_text_renamed_over', { name: 'live_a', has: 'Hand words two' }, () => saved(() => importYaml(folder, yaml().replace('Hand words one', 'Hand words two'))));

  // A new box by hand has no row: the page gives it a free place on screen, writes it, and it lands. Its tool needs no package, so this runs anywhere.
  const handBox = '  live_tool:\n    tool: no-such-package-here\n    instructions: Hand instructions\n';
  const handAdded = await timed('hand_add', { name: 'live_tool' }, () => saved(() => importYaml(folder, `${yaml().trimEnd()}\n${handBox}`)));
  check('hand_add_lands', handAdded.landing);
  const toolRow = await until('its row', async () => (await rows()).live_tool, Boolean);
  check('hand_add_row_written', Number.isFinite(toolRow.x), toolRow);
  check('hand_add_on_screen', await onScreen('live_tool'), toolRow);
  check('hand_add_on_nothing', (await overlaps()).length === 0, await overlaps());

  // The same for a command that gives no place.
  const auto = await timed('add_placed', { name: 'live_auto' }, () => project('add', 'cozy/edit', 'live_auto', 'description=Placed by the page'));
  check('add_placed_lands', auto.landing);
  const autoRow = await until('its row', async () => (await rows()).live_auto, Boolean);
  check('add_placed_on_screen', await onScreen('live_auto'), autoRow);
  check('add_placed_on_nothing', (await overlaps()).length === 0, await overlaps());

  // Approved by a command: it turns into that picture. Unapproved: back. Out of date has no dot now (Paul, 11-05): what shows is the picture.
  const beforeApprove = Date.now();
  const approval = await timed('approve', { name: 'live_tool', picture: true }, () => project('approve', 'live_tool', hash));
  seen.approvalBounds = { started: beforeApprove, cliReturned: approval.at, shown: approval.at + approval.ms, ms: approval.ms };
  check('approve_in_the_status', (await held('live_tool')).pinned === true);
  await timed('unapprove', { name: 'live_tool', picture: false }, () => project('unapprove', 'live_tool'));
  check('unapprove_in_the_status', !(await held('live_tool')).pinned);
  await Promise.all(bodyChecks);
  seen.approvalTrace = await page.evaluate(() => window.approvalTrace);
  seen.requests = requests;
  seen.csp = await page.evaluate(() => window.violations);
  await browser.close();
  rmSync(scratch, { recursive: true, force: true });
  process.stdout.write(JSON.stringify({ failures, seen }, null, 2) + '\n');
  process.exitCode = failures.length ? 1 : 0;
})().catch(died);

function died(error) {
  process.stderr.write(`${error.name}: ${String(error.message).replace(/(?:media|code)=[^&#\s]+/g, '<REDACTED>')}\n`);
  process.stdout.write(JSON.stringify({ failures, seen }, null, 2) + '\n');
  process.exit(2);
}
