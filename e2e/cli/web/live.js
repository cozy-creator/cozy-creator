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
          if (rest.slice(0, end).includes('"type":"resync"')) window.resyncs.push(Date.now());
          rest = rest.slice(end + 2);
        }
      }
    })().catch(() => {});
    return new Response(theirs, { status: answer.status, statusText: answer.statusText, headers: answer.headers });
  };
  const box = (name) => [...document.querySelectorAll('.react-flow__node')].find((node) => node.querySelector('.nm')?.textContent === name);
  window.boxOf = box;
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

  // The initial snapshot deliberately animates nothing. Establish a real, applied
  // replica before measuring an external add, then return to an empty canvas.
  await project('add', 'text', 'live_readiness', 'text=Ready for live edits', 'at=0,0,260');
  await shows({ name: 'live_readiness', has: 'Ready for live edits' });
  await project('delete', 'live_readiness');
  await shows({ gone: 'live_readiness' });
  await until('initialized empty live canvas', () => page.locator('.react-flow__node').count(), (count) => count === 0);
  check('initial_replica_ready', true);

  // A command adds it: it shows by itself where it was told, and lands.
  let place = await spot();
  const added = await timed('add', { name: 'live_a', x: place.x, y: place.y }, () => project('add', 'cozy/edit', 'live_a', `at=${place.x},${place.y},260`, 'description=Words from the CLI'));
  check('add_prints_its_name', added.stdout.trim() === 'live_a', added.stdout);
  check('add_lands', added.landing);
  check('add_in_the_status', (await held('live_a'))?.tool === 'cozy/edit');

  // Its text.
  await timed('set_text', { name: 'live_a', has: 'Second words' }, () => project('set', 'live_a', 'description=Second words'));
  check('set_in_the_status', (await held('live_a')).fields?.description === 'Second words');

  // A picture.
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
  // Its imported picture must be usable before timing state propagation, clear of first-cold media setup.
  await page.waitForFunction((name) => {
    const image = window.boxOf(name)?.querySelector('.media img');
    return image?.complete && image.naturalWidth > 0 && image.naturalHeight > 0;
  }, 'live_pic', { timeout: FIRST_RUN_MS });
  await timed('approve', { name: 'live_tool', picture: true }, () => project('approve', 'live_tool', hash));
  check('approve_in_the_status', (await held('live_tool')).pinned === true);
  await timed('unapprove', { name: 'live_tool', picture: false }, () => project('unapprove', 'live_tool'));
  check('unapprove_in_the_status', !(await held('live_tool')).pinned);

  // Moved and made wider by a command.
  place = await spot();
  await timed('move', { name: 'live_a', x: place.x, y: place.y, w: 320 }, () => project('set', 'live_a', `at=${place.x},${place.y},320`));
  check('its_place_reads_back', (await rows()).live_a.w === 320);

  // Renamed by a command: the same box, where it was, landing on nothing.
  const renamed = await timed('rename', { all: [{ name: 'live_b', x: place.x, y: place.y }, { gone: 'live_a' }] }, () => project('rename', 'live_a', 'live_b'));
  check('rename_does_not_land', !(await page.evaluate(() => Boolean(window.boxOf('live_b')?.querySelector('.card.landing')))), renamed);

  // Deleted by a command, which prints the command that undoes it. That brings it back where it was.
  const deleted = await timed('delete', { gone: 'live_b' }, () => project('delete', 'live_b'));
  const back = deleted.stdout.trim().split('\n').at(-1);
  check('delete_prints_its_undo', back.startsWith(`cozy project -p ${folder} undo `) && /^\d+$/.test(back.split(' ').at(-1)), deleted.stdout);
  check('delete_gone_from_the_status', !(await held('live_b')));
  await timed('undo', { name: 'live_b', x: place.x, y: place.y, w: 320 }, () => project('undo', back.split(' ').at(-1)));
  check('undo_in_the_status', (await held('live_b'))?.fields?.description === 'Hand words two');

  // By hand: a row from the CLI, an asset gone in an edited export, and an export that doesn't read, which is refused and changes nothing.
  place = await spot();
  await timed('hand_row', { name: 'live_b', x: place.x, y: place.y }, () => saved(() => {
    execFileSync(process.env.COZY, ['project', '-p', folder, 'set', 'live_b', `at=${place.x},${place.y}`], { stdio: 'ignore' });
  }));
  await timed('hand_delete', { gone: 'live_auto' }, () => saved(() => importYaml(folder, yaml().replace(`${linesOf(yaml(), 'live_auto').trimEnd()}\n`, ''))));
  const whole = yaml();
  let broken = '';
  try { importYaml(folder, `${whole}\n  : [ this doesn't read\n`); } catch (error) { broken = `${error.stdout ?? ''}${error.stderr ?? ''}${error.message}`; }
  check('broken_file_says_why', broken.includes('project_yaml'), broken);
  check('broken_file_keeps_the_canvas', (await box('live_b').count()) === 1 && yaml() === whole);
  await timed('hand_mended', { name: 'live_b', has: 'Mended' }, () => saved(() => importYaml(folder, whole.replace('Hand words two', 'Mended'))));
  refused.length = 0;

  // What's typed and not saved stays. A command changing another field of that asset leaves it to go on, and what's typed is saved over it.
  const words = box('live_tool').locator('.mention-text[data-field=instructions]');
  const typedText = () => words.evaluate((el) => el.textContent);
  await open('live_tool');
  await words.click();
  await page.keyboard.press('End');
  await page.keyboard.type(' TYPED-ONE');
  // It's behind More, so what shows is that the page read it.
  const readIt = page.waitForResponse((answer) => new URL(answer.url()).pathname.endsWith('/assets/live_tool'));
  const other = await project('set', 'live_tool', 'style=noir');
  await readIt;
  times.another_field = Date.now() - other.at;
  check('another_field_live', times.another_field <= LIVE_MS, times.another_field);
  await page.waitForTimeout(100);
  check('another_field_leaves_it', (await typedText()).includes('TYPED-ONE'), await typedText());
  check('another_field_no_conflict', (await box('live_tool').locator('.status.ood:has-text("changed under this")').count()) === 0);
  await page.mouse.click(1700, 1100);
  await until('typed saved over it', async () => (await held('live_tool')).fields, (fields) => fields?.instructions?.includes('TYPED-ONE') && fields?.style === 'noir');
  check('typed_and_theirs_both_stand', true);

  // The command changes that field: what's typed stays, typing goes on, and the page says so within a second. Keep mine writes it over theirs.
  await open('live_tool');
  await words.click();
  await page.keyboard.press('End');
  await page.keyboard.type(' TYPED-TWO');
  const typedBefore = await typedText();
  await timed('conflict_shows', { name: 'live_tool', conflict: true }, () => project('set', 'live_tool', 'instructions=FROM-THE-CLI'));
  check('conflict_keeps_whats_typed', (await typedText()) === typedBefore, { typed: await typedText(), typedBefore });
  check('conflict_file_holds_theirs', (await held('live_tool')).fields?.instructions === 'FROM-THE-CLI');
  await page.keyboard.type(' MORE');
  check('conflict_typing_goes_on', (await typedText()).endsWith('MORE'), await typedText());
  await shot('conflict');
  await box('live_tool').locator('button:has-text("Keep mine")').click();
  await until('mine kept', async () => (await held('live_tool')).fields?.instructions, (text) => text?.includes('TYPED-TWO MORE'));
  check('conflict_keep_mine', (await box('live_tool').locator('.status.ood:has-text("changed under this")').count()) === 0);

  // Take the file's: what's typed goes, and the box shows the file's.
  await open('live_tool');
  await words.click();
  await page.keyboard.press('End');
  await page.keyboard.type(' TYPED-THREE');
  await timed('conflict_shows_again', { name: 'live_tool', conflict: true }, () => project('set', 'live_tool', 'instructions=FROM-THE-CLI-AGAIN'));
  await box('live_tool').locator('button:has-text("Take the file\'s")').click();
  await until("the file's words", typedText, (text) => text === 'FROM-THE-CLI-AGAIN');
  check('conflict_take_the_files', (await held('live_tool')).fields?.instructions === 'FROM-THE-CLI-AGAIN');

  // Renamed by a command while being typed in: the same box, and what's typed stays.
  await open('live_tool');
  await words.click();
  await page.keyboard.press('End');
  await page.keyboard.type(' TYPED-FOUR');
  await timed('rename_under_typing', { all: [{ name: 'live_held', has: 'TYPED-FOUR' }, { gone: 'live_tool' }] }, () => project('rename', 'live_tool', 'live_held'));
  await page.mouse.click(1700, 1100);
  await until('typed saved under its new name', async () => (await held('live_held'))?.fields?.instructions, (text) => text?.includes('TYPED-FOUR'));
  check('typed_survives_a_rename', true);

  // What the page does, the CLI reads at once.
  const readsBack = async (label, read, wanted) => {
    const from = Date.now();
    await until(label, read, wanted);
    times[`page_${label}`] = Date.now() - from;
    check(`page_${label}_reads_back`, times[`page_${label}`] <= LIVE_MS, times[`page_${label}`]);
  };
  // Measured at rest: a box worked on has the canvas drawn in on it, and leaving it moves everything back.
  await away();
  const moved = await box('live_b').locator('.nm').boundingBox();
  const canvasBefore = (await rows()).live_b;
  await page.mouse.move(moved.x + 5, moved.y + 5);
  await page.mouse.down();
  await page.mouse.move(moved.x + 9, moved.y + 5);
  await page.mouse.move(moved.x + 25, moved.y + 25, { steps: 4 });
  await page.mouse.up();
  await readsBack('move', async () => (await rows()).live_b, (row) => row.x !== canvasBefore.x && row.y !== canvasBefore.y);
  await open('live_held');
  const heldWords = box('live_held').locator('.mention-text[data-field=instructions]');
  await heldWords.click();
  await page.keyboard.press('End');
  await page.keyboard.type(' FROM-THE-PAGE');
  await page.mouse.click(1700, 1100);
  await readsBack('text', async () => (await held('live_held')).fields?.instructions, (text) => text?.includes('FROM-THE-PAGE'));
  await open('live_b');
  await box('live_b').locator('.nm').dblclick();
  await page.waitForSelector('.nm-field');
  await page.keyboard.type('live_c');
  await page.keyboard.press('Enter');
  await readsBack('rename', () => held('live_c'), Boolean);
  await away();
  // An Edit is wide, and back at rest its X, at its right, can be past the window: F brings it all in, as for a person.
  await page.keyboard.press('f');
  await page.waitForTimeout(400);
  await pointAt(page, box('live_c').locator('.nm'));
  await box('live_c').locator('.head [aria-label=Delete]').click();
  await readsBack('delete', () => held('live_c'), (asset) => !asset);
  await page.locator('.note:has-text("live_c") button:has-text("Undo")').click();
  await readsBack('undo', () => held('live_c'), Boolean);
  await away();

  // One asset, whatever the project holds, and a field cleared.
  const shown = await asJson('status', 'live_held');
  check('status_reads_one', shown.asset.name === 'live_held' && Array.isArray(shown.versions), shown.asset);
  await project('set', 'live_held', 'style=');
  check('a_field_cleared', !('style' in (await held('live_held')).fields));
  // An Edit's tracks and clips from a file, as its timeline's gestures write them: the page shows its clip.
  place = await spot();
  const cutFile = join(scratch, 'cut.yaml');
  writeFileSync(cutFile, 'description: A cut from a file\ntimeline:\n  tracks:\n    - clips:\n        - {clip: live_pic, frames: 24}\n');
  await project('add', 'cozy/edit', 'live_cut', `at=${place.x},${place.y},260`);
  await timed('edit_from_a_file', { name: 'live_cut', picture: true }, () => project('set', 'live_cut', '--input', cutFile));
  check('edit_reads_back', (await held('live_cut')).timeline?.tracks?.[0]?.clips?.[0]?.name === 'live_pic', (await held('live_cut')).timeline);
  await project('delete', 'live_cut');

  if (local) {
    // A render from the CLI, on the fixture tool: its progress on its card, moving, with no reload, and it lands. The first one only readies its environment.
    // Read with this machine's Runtime, as a run reads it: a box doesn't wait on PyPI's.
    await cozy('--json', 'run', `${TOOL}/still`, 'prompt=primed', '--on', 'local');
    place = await spot();
    await project('add', `${TOOL}/slow`, 'live_shot', `at=${place.x},${place.y},260`, 'instructions=A slow still', 'seconds:=0.1');
    await box('live_shot').waitFor();
    await project('render', 'live_shot', '--on', 'local');
    await timed('a_first_render', { name: 'live_shot', picture: true }, async () => ({ at: Date.now() }));
    await project('set', 'live_shot', 'seconds:=14');
    // The first render over on the page too: its picture can show from its run before its take lands, and until the page reads that it's
    // landed, its box shows that run whole. Progress sampled from here is the next render's alone.
    await until('the first render over on the page', () => page.evaluate((name) => !window.boxOf(name)?.querySelector('.prog'), 'live_shot'), Boolean);
    const moving = bounded(page.evaluate((name) => window.progressOf(name), 'live_shot'), FIRST_RUN_MS, () => 'its progress never ended');
    await timed('render_starts', { name: 'live_shot', dot: 's-making' }, () => project('render', 'live_shot', '--on', 'local', '--queue'));
    const landed = until('two takes', async () => (await held('live_shot')).versions, (versions) => versions === 2, FIRST_RUN_MS).then(() => Date.now());
    const places = await moving;
    const shownAt = Date.now();
    seen.render_progress = places;
    const fractions = places.map((held) => held.match(/-?[\d.]+/)?.[0]).filter(Boolean).map(Number);
    check('render_progress_moves', fractions.length >= 3 && fractions.every((at, i) => !i || at >= fractions[i - 1]), places);
    // Steps read as steps, whatever the package calls its stage.
    const said = await page.evaluate(() => window.progressSaid);
    seen.render_said = said;
    // The time is Calcifer's for the whole run; its first run here says so until it has steps of its own.
    check('steps_read_as_denoise_steps', said.some((text) => /^Denoise steps: \d+\/10(, (\d+[smhd](\d+[smh])? left|first run on this machine))?$/.test(text)) && !said.some((text) => /drawing/i.test(text)), said);
    const fileAt = await landed;
    times.render_lands = shownAt - fileAt;
    check('render_lands_live', times.render_lands <= LIVE_MS, times.render_lands);
    check('render_lands_a_second_take', (await box('live_shot').locator('.dot.s-making').count()) === 0);
    await timed('approve_a_take', { name: 'live_shot', picture: true }, () => project('approve', 'live_shot', '2'));
    check('approve_a_take_in_the_status', (await held('live_shot')).approved === 2);
    await project('unapprove', 'live_shot');

    // A text asset's words stand wherever it's mentioned: changed by a command, they're on the open canvas, and what mentions them is out of date,
    // which no dot or line says at rest (Paul, 11-05).
    place = await spot();
    await project('add', 'text', 'live_mood', 'text=Soft morning light.');
    await project('add', `${TOOL}/still`, 'live_styled', `at=${place.x},${place.y},260`, 'instructions=A still in <live_mood>.');
    await project('render', 'live_styled', '--on', 'local');
    await shows({ name: 'live_styled', dot: 's-current' });
    await timed('text_asset_outdates', { all: [{ name: 'live_mood', has: 'Hard evening light.' }, { name: 'live_styled', dot: 's-current' }] }, () => project('set', 'live_mood', 'text=Hard evening light.'));
    check('what_mentions_it_is_out_of_date', (await held('live_styled')).standing === 'out_of_date', await held('live_styled'));
    await project('delete', 'live_styled', 'live_mood');

    // What waits its turn says so, and when it has landed no longer does: what's said of the project besides its assets goes when it's over.
    await project('set', 'live_shot', 'seconds:=3');
    place = await spot();
    await project('add', `${TOOL}/still`, 'live_after', `at=${place.x},${place.y},260`, 'instructions=After <live_shot>.');
    const waiting = shows({ name: 'live_after', dot: 's-making' });
    await project('render', 'live_shot', 'live_after', '--on', 'local', '--queue');
    await waiting;
    const afterAt = await until('what waited landed', async () => (await held('live_after')).versions, (versions) => versions === 1, FIRST_RUN_MS).then(() => Date.now());
    const afterDone = await shows({ name: 'live_after', dot: 's-current' });
    times.queued_goes = afterDone.when - afterAt;
    check('queued_goes_when_it_lands', times.queued_goes <= LIVE_MS, times.queued_goes);
    await project('delete', 'live_after');

    // Deleted while it renders: its run stops, nothing's left queued, and the box goes. The line it printed brings it back with its takes.
    await project('set', 'live_shot', 'seconds:=60');
    await timed('long_render_starts', { name: 'live_shot', dot: 's-making' }, () => project('render', 'live_shot', '--on', 'local', '--queue'));
    await until('it running', async () => (await held('live_shot')).making?.length, Boolean);
    const stopped = await timed('delete_while_rendering', { gone: 'live_shot' }, () => project('delete', 'live_shot'));
    const going = await until('nothing owed', async () => (await status()).queue ?? [], (queue) => queue.length === 0, 30_000);
    check('delete_stops_its_render', going.length === 0, going);
    await timed('undo_after_a_render', { name: 'live_shot' }, () => project('undo', stopped.stdout.trim().split(' ').at(-1)));
    check('undo_keeps_its_takes', (await held('live_shot')).versions === 3, await held('live_shot'));

  }
  // A description, on what has a picture. It says its words when it's open.
  await open('live_pic');
  await timed('description_set', { name: 'live_pic', has: 'Said by the CLI' }, () => project('set', 'live_pic', 'description=Said by the CLI'));
  await away();
  await shot('single');

  // A burst: 200 edits as fast as they go. Twelve boxes on screen are written and moved, each by its own line of commands so the last is known,
  // and 40 come with no place. Few reads, shown as it goes, right at the end with no reload, and frames sampled through it.
  const fixed = Array.from({ length: 12 }, (_, i) => `burst_${i}`);
  // Clear of everything so far, and all of it fitted on the screen. A row apart by more than an Edit at rest and the room it keeps under it
  // for its add-ons and the name of a box below (e139a61c6, shown.ts): about 460 at 260 wide. Closer, the page moves one off the other.
  const ROW = 500;
  await Promise.all(fixed.map((name, i) => project('add', 'cozy/edit', name, `at=${260 + (i % 4) * 300},${2000 + Math.floor(i / 4) * ROW},260`, 'description=start')));
  // Fitted to all of them: a box that comes after a fit is drawn for a moment, then not, off the screen (macOS, 41955999465).
  await until('the boxes for the burst', async () => {
    await page.keyboard.press('f');
    await page.waitForTimeout(250);
    return page.evaluate((names) => names.every((name) => {
      const at = window.boxOf(name)?.getBoundingClientRect();
      return at && at.bottom <= innerHeight && at.right <= innerWidth;
    }), fixed);
  }, Boolean);
  const lanes = Array.from({ length: 8 }, () => []);
  const last = new Map();
  const targets = fixed.map((name, i) => ({ name, x: 260 + (i % 4) * 300, y: 2000 + Math.floor(i / 4) * ROW }));
  for (let edit = 0; edit < 160; edit++) {
    const which = edit % fixed.length;
    const lane = lanes[which % lanes.length];
    if (edit % 4 === 3) {
      targets[which].x += 2;
      const { name, x, y } = targets[which];
      lane.push(['set', name, `at=${x},${y},260`]);
    } else {
      const text = `burst-text-${edit}-end`;
      last.set(fixed[which], text);
      lane.push(['set', fixed[which], `description=${text}`]);
    }
  }
  for (let more = 0; more < 40; more++) lanes[more % lanes.length].push(['add', 'cozy/edit', `arrived_${more}`, 'description=Arrived in the burst']);
  check('burst_is_200_edits', lanes.flat().length === 200, lanes.flat().length);
  await page.evaluate((name) => window.watching(name), fixed[0]);
  const [projectReads, headReads, canvasReads, assetReads] = [reads.project, reads.heads, reads.canvas, reads.assets];
  const cdp = await context.newCDPSession(page);
  await cdp.send('Performance.enable');
  const threadBusy = async () => (await cdp.send('Performance.getMetrics')).metrics.find(({ name }) => name === 'TaskDuration').value * 1000;
  const busyFrom = await threadBusy();
  const started = Date.now();
  const lasts = await Promise.all(lanes.map(async (lane) => {
    let at = 0;
    for (const words of lane) ({ at } = await project(...words));
    return at;
  }));
  const ended = Math.max(...lasts);
  const right = { all: fixed.map((name, i) => ({ name, has: last.get(name), x: targets[i].x, y: targets[i].y })) };
  const { when } = await shows(right, STEP_MS);
  const thread = ((await threadBusy()) - busyFrom) / (when - started);
  const { frames, texts } = await bounded(page.evaluate(() => new Promise((done) => { window.stopWatching = done; })), STEP_MS, () => 'no frame came');
  const burst = {
    edits: 200,
    seconds: +((ended - started) / 1000).toFixed(2),
    right_after_ms: when - ended,
    project_reads: reads.project - projectReads,
    head_reads: reads.heads - headReads,
    canvas_reads: reads.canvas - canvasReads,
    asset_reads: reads.assets - assetReads,
    shown_along_the_way: texts.length,
    frames: summary(frames),
    thread: +thread.toFixed(2),
  };
  seen.burst = burst;
  check('burst_right_in_a_second', burst.right_after_ms <= LIVE_MS, burst);
  check('burst_reads_coalesced', burst.project_reads === 0 && burst.head_reads <= 20 && burst.asset_reads <= 200, burst);
  check('burst_shown_as_it_goes', burst.shown_along_the_way >= 3, { burst, texts: texts.slice(0, 8) });
  check('burst_not_thrashing', burst.thread <= BURST_THREAD, burst);
  const after = await status();
  for (const [name, text] of last) if (after.assets.find((asset) => asset.name === name)?.fields?.description !== text) check(`burst_${name}_in_the_file`, false, text);
  const arrived = after.assets.filter((asset) => asset.name.startsWith('arrived_'));
  check('burst_all_arrived', arrived.length === 40, arrived.length);
  // Fitted to what's there as the last of them find their places.
  await until('every box placed and shown', async () => {
    await page.keyboard.press('f');
    await page.waitForTimeout(250);
    return page.evaluate(() => [...document.querySelectorAll('.nm')].filter((name) => name.textContent.startsWith('arrived_')).length);
  }, (count) => count === 40);
  await page.waitForTimeout(400);
  await shot('burst');
  const cards = await until('every row written', rows, (now) => arrived.every((asset) => now[asset.name]));
  check('burst_rows_written', arrived.every((asset) => cards[asset.name]));
  // What the page placed: the rest are where this cell said.
  const placedOver = (await overlaps()).filter((pair) => pair.includes('arrived_'));
  check('burst_on_nothing', placedOver.length === 0, placedOver.slice(0, 12));

  // The stream made to fall behind. The page stops reading, Calcifer's socket to it fills, and the ring goes round: what happened in between
  // never reaches it as events. A real text, a move and a new box are in there, and only `resync` can tell it of them.
  const streamsBefore = streams;
  const canvasBefore2 = reads.canvas;
  await page.keyboard.press('f');
  await cdp.send('Debugger.enable');
  await cdp.send('Debugger.pause');
  const clearOfAll = await spot();
  const stalled = Date.now();
  const fill = 'f'.repeat(200);
  const big = Object.fromEntries(Array.from({ length: 1000 }, (_, i) => [`${fill}_${i}`, { x: i, y: 0 }]));
  // Past what a socket and the browser hold on any OS, each its own event: Windows has held 24 MB and never fallen behind (41971301588).
  const body = JSON.stringify({ cards: big });
  for (let sent = 0; sent < 64 << 20; sent += body.length) {
    const answer = await api(`/v1/projects/${id}/canvas`, { method: 'PATCH', body });
    if (answer.status !== 200) throw new Error(`filling: ${JSON.stringify(answer)}`);
  }
  await api(`/v1/projects/${id}/canvas`, { method: 'PATCH', body: JSON.stringify({ cards: Object.fromEntries(Object.keys(big).map((name) => [name, null])) }) });
  await project('set', fixed[0], 'description=SAID-WHILE-BEHIND');
  await project('set', fixed[1], `at=${clearOfAll.x},${clearOfAll.y},260`);
  await project('add', 'cozy/edit', 'made_while_behind', 'at=1000,700,260', 'description=Made while behind');
  for (let ping = 0; ping < RING + 64; ping++) {
    const answer = await api(`/v1/projects/${id}/canvas`, { method: 'PATCH', body: JSON.stringify({ cards: { ping: { x: ping, y: 0 } } }) });
    if (answer.status !== 200) throw new Error(`pinging: ${JSON.stringify(answer)}`);
  }
  const heldUp = Date.now() - stalled;
  await cdp.send('Debugger.resume');
  await cdp.send('Debugger.disable');
  const caughtUp = await shows({ all: [{ name: fixed[0], has: 'SAID-WHILE-BEHIND' }, { name: fixed[1], x: clearOfAll.x, y: clearOfAll.y }] }, STEP_MS);
  const behind = {
    stalled_ms: heldUp,
    caught_up_ms: caughtUp.when - stalled - heldUp,
    resyncs: (await page.evaluate(() => window.resyncs)).length,
    streams: streams - streamsBefore,
    canvas_reads: reads.canvas - canvasBefore2,
  };
  seen.behind = behind;
  check('fell_behind_for_real', behind.resyncs >= 1, behind);
  check('fell_behind_on_one_stream', behind.streams === 0, behind);
  check('fell_behind_read_again', behind.canvas_reads >= 1, behind);
  await page.keyboard.press('f');
  await until('what was made while behind', () => box('made_while_behind').count(), (count) => count === 1);
  check('fell_behind_nothing_lost', (await held(fixed[0])).fields?.description === 'SAID-WHILE-BEHIND' && (await held('made_while_behind'))?.fields?.description === 'Made while behind');
  // The stream is live again: a command shows as before.
  await timed('live_after_falling_behind', { name: fixed[0], has: 'AND-LIVE-AGAIN' }, () => project('set', fixed[0], 'description=AND-LIVE-AGAIN'));

  check('never_reloaded', documents === 0 && (await page.evaluate(() => window.notReloaded === true)), { documents });
  check('one_stream_throughout', streams === 0, { streams });
  check('csp_holds', (await page.evaluate(() => window.violations)).length === 0, await page.evaluate(() => window.violations));
  await new Promise((done) => setTimeout(done, 300));
  // An asset gone by the time the page read it is no fault: that's how it hears it's gone.
  const unasked = refused.filter((refusal) => refusal.url.includes('/v1/projects/') && refusal.code !== 'asset_changed' && !(refusal.code === 'asset_unknown' && refusal.method === 'GET'));
  check('nothing_refused_unasked', unasked.length === 0, unasked);
  check('no_page_errors', errors.length === 0, { errors, refused });

  // A read of what's there isn't an arrival.
  await page.reload();
  await page.waitForSelector('.nm');
  await until('its boxes', () => page.locator('.nm').count(), (count) => count > 10);
  check('a_reload_lands_nothing', (await page.locator('.card.landing').count()) === 0);
  rmSync(scratch, { recursive: true, force: true });
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
