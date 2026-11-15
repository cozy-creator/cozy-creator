// A second API client and the page edit one project on a real Calcifer. The canvas is never reloaded.
// Each request is timed through its appearance, including writes to a field being typed in, renders, and a 200-edit burst.
//   node live.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $COZY: the CLI; $SHOT: where to keep its screenshots; $ALONE: on a shared
// computer, so it runs nothing; $ARGV_LOG: where each cozy call is logged for coverage. Prints what it saw as JSON and exits 1 if anything failed.
const { chromium } = require(process.env.PLAYWRIGHT);
const { execFile } = require('child_process');
const { appendFileSync, mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { holderToken, projectFolder, projectFromPage, projectAction, projectJson, calcifer, signIn, STEP_MS, boxOf, closeBox, lastLook, lookAt, noStall, openBox, png, until, pointAt, addBoxes, settled } = require('./page');

const { canvasBox, canvasView, hoverBox, screenshotPixels, fitCanvas } = require('./page');

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
  const inspection=()=>document.querySelector('.canvas-root')?.canvasInspection;
  const box=name=>inspection()?.card(name) ?? null;
  window.boxOf=box;
  window.clicks=[];
  for(const type of ['pointerdown','pointerup','click','dblclick']) {
    window.addEventListener(type,({target})=>window.clicks.push(`${type} ${target.tagName}.${target.className} ${location.pathname}`.slice(0,160)),true);
  }
  const focusedBox=name=>document.querySelector(`[data-focused-asset="${CSS.escape(name)}"]`);
  const wordsOf=name=>{
    const focus=focusedBox(name);
    if(focus)return focus.textContent ?? '';
    const drawn=box(name)?.rendered;
    return drawn ? [drawn.words,...Object.values(drawn.labels).map(label=>label.fullText ?? label.text)].filter(Boolean).join(' ') : '';
  };
  const holds=spec=>{
    if(spec.all)return spec.all.every(holds);
    if(spec.gone)return !box(spec.gone) && !focusedBox(spec.gone);
    const card=box(spec.name),focus=focusedBox(spec.name),drawn=card?.rendered;
    if(!drawn && !focus)return false;
    if(spec.has!==undefined && !wordsOf(spec.name).includes(spec.has))return false;
    if(spec.lacks!==undefined && wordsOf(spec.name).includes(spec.lacks))return false;
    if(spec.x!==undefined && (!card?.position || Math.abs(card.position.x-spec.x)>0.5 || Math.abs(card.position.y-spec.y)>0.5))return false;
    if(spec.w!==undefined && (!card || Math.abs(card.bounds.width/inspection().camera().zoom-spec.w)>0.5))return false;
    const busy=focus ? Boolean(focus.querySelector('.state-dot.queued,.state-dot.running')) : Boolean(drawn.stateDot && ['queued','running'].includes(drawn.state));
    const dot=focus ? Boolean(focus.querySelector('.state-dot')) : Boolean(drawn.stateDot);
    if(spec.dot==='busy' && !busy)return false;
    if(spec.dot==='none' && dot)return false;
    const picture=focus ? [...focus.querySelectorAll('.media img')].some(image=>image.complete && image.naturalWidth>0 && image.naturalHeight>0) : Boolean(drawn.media.ready);
    if(spec.picture!==undefined && picture!==spec.picture)return false;
    return true;
  };
  /** Frame timing reads actual retained display state. Arrival comes only from the renderer's real animation diagnostic. */
  window.shown=spec=>new Promise(done=>{
    const look=()=>{
      if(!holds(spec))return requestAnimationFrame(look);
      done({when:Date.now(),landing:spec.name ? box(spec.name)?.rendered?.landing?.active===true : false});
    };
    look();
  });
  window.unshown=spec=>(spec.all ?? [spec]).filter(one=>!holds(one)).map(one=>{
    const card=box(one.name ?? one.gone);
    return {wanted:one,drawn:Boolean(card?.rendered),text:wordsOf(one.name ?? one.gone).slice(0,160),place:card?.position};
  });
  window.progressOf=name=>new Promise(done=>{
    const places=[];window.progressSaid=[];
    const look=()=>{
      const drawn=box(name)?.rendered,progress=drawn?.progress,meter=drawn?.meter;
      if(!drawn)return requestAnimationFrame(look);
      if(progress || meter) {
        const fraction=progress?.fraction,place=meter && Number.isFinite(fraction) ? `scaleX(${fraction})` : 'unknown';
        if(places.at(-1)!==place)places.push(place);
        const says=progress ? [progress.state,progress.count].filter(Boolean).join(' ') : '';
        if(says && window.progressSaid.at(-1)!==says)window.progressSaid.push(says);
      }
      if(!progress && !meter && places.length && drawn.media.ready)done(places);else requestAnimationFrame(look);
    };
    look();
  });
  window.watching=name=>{
    const frames=[],texts=[];let last=performance.now();
    const step=now=>{
      frames.push(now-last);last=now;
      const text=wordsOf(name);if(text && texts.at(-1)!==text)texts.push(text);
      if(window.stopWatching)window.stopWatching({frames,texts});else requestAnimationFrame(step);
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
  const token = holderToken();
  const api = async (path, init = {}) => {
    const answer = await fetch(base + path, { ...init, headers: { Authorization: `Bearer ${token}`, 'cozy-project-storage': '3', 'Content-Type': 'application/json', ...init.headers } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  };
  const local = (await api('/v1/status')).body.local_usable && !process.env.ALONE;
  seen.ran = local;

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s folder"]', projectFolder(`web-live-${Date.now()}`));
  await page.getByLabel("Create without encryption", { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const folder = (await api(`/v1/projects/${id}`)).body.path;
  seen.project = { id, folder };
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
    execFile(COZY, words, { cwd: folder, encoding: 'utf8', maxBuffer: 1 << 26, timeout: FIRST_RUN_MS }, (error, stdout, stderr) => {
      const at = Date.now();
      if (error) fail(new Error(`cozy ${words.join(' ')}: ${error.message}: ${stdout}${stderr}`));
      else done({ stdout, stderr, at });
    });
  });
  const project = (...words) => projectAction(folder, ...words);
  const asJson = async (...words) => (await project(...words)).value;
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
  /** The page's writes of `name` wait until the function this returns is called. */
  const holdWrites = async (name) => {
    let open;
    const held = new Promise((done) => (open = done));
    const path = `**/v1/projects/*/assets/${name}`;
    const route = (call) => (call.request().method() === 'PATCH' ? held.then(() => call.fallback().catch(() => {})) : call.fallback());
    await page.route(path, route);
    return async () => {
      open();
      await page.unroute(path, route);
    };
  };
  /** Another writer's edit, through the project API. */
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
  const clearOf = (boxes) => (x, y) => boxes.every((box) => x + 360 <= box.x || box.x + (box.w ?? 720) <= x || y + 400 <= box.y || box.y + 400 <= y);
  const spot = async () => {
    const clear = clearOf(Object.values((await rows()) ?? {}));
    // Left of 1060: an Edit open is 720 wide, its X at its right, and the canvas never scrolls to reach it.
    for (let y = 90; ; y += 170) for (const x of [700, 1040]) if (clear(x, y)) return { x, y };
  };
  /** A clear spot wholly in the canvas's view, whole pixels on the canvas. */
  const spotInView = async () => {
    const clear = clearOf(Object.values((await rows()) ?? {}));
    const view = await page.evaluate(() => {
      const root=document.querySelector('.canvas-root'),at=root.canvasInspection.camera(),pane=root.getBoundingClientRect();
      return {x:-at.x/at.zoom,y:-at.y/at.zoom,w:pane.width/at.zoom,h:pane.height/at.zoom};
    });
    for (let y = Math.ceil(view.y); y + 400 <= view.y + view.h; y += 20) for (let x = Math.ceil(view.x); x + 360 <= view.x + view.w; x += 20) if (clear(x, y)) return { x, y };
    throw new Error(`no clear spot in view: ${JSON.stringify(view)}`);
  };
  const overlaps = () => page.evaluate(() => {
    const inspection=document.querySelector('.canvas-root').canvasInspection;
    const rects=inspection.names().map(name=>({name,card:inspection.card(name)})).filter(({card})=>card?.rendered).map(({name,card:{bounds:b}})=>({name,rect:{left:b.x,top:b.y,right:b.x+b.width,bottom:b.y+b.height}}));
    const over = [];
    rects.forEach((a, i) => rects.slice(i + 1).forEach((b) => {
      if (a.rect.left < b.rect.right - 1 && b.rect.left < a.rect.right - 1 && a.rect.top < b.rect.bottom - 1 && b.rect.top < a.rect.bottom - 1) over.push(`${a.name}/${b.name}`);
    }));
    return over;
  });
  const onScreen = (name) => page.evaluate((asked) => {
    const card=window.boxOf(asked),rect=card?.bounds;
    return Boolean(card?.rendered) && rect.x>=0 && rect.y>=0 && rect.x+rect.width<=innerWidth && rect.y+rect.height<=innerHeight;
  }, name);

  // Pixel receipts are outside timed propagation/burst spans, and use screenshots rather than the renderer's metadata.
  const pixelsSeen=(seen.pixels={});
  const capture=async (name,label)=>{
    await page.mouse.move(2,500);
    await until('native overlay off',()=>page.locator('.canvas-node').count(),n=>n===0);
    const card=await until(`${name} displayed`,()=>canvasBox(page,name),card=>Boolean(card?.rendered));
    const bytes=await page.screenshot();
    if(process.env.SHOT && label)writeFileSync(`${process.env.SHOT}-${label}.png`,bytes);
    return {card,read:screenshotPixels(bytes),dpr:await page.evaluate(()=>devicePixelRatio),camera:await canvasView(page)};
  };
  const imageSamples=(shot,points)=>points.map(([x,y])=>shot.read((shot.card.body.x+shot.card.body.width*x)*shot.dpr,(shot.card.body.y+shot.card.body.height*y)*shot.dpr));
  const matchesImage=(rgb,[x,y])=>Math.abs(rgb[0]-77)<=18 && Math.abs(rgb[1]-x*255)<=20 && Math.abs(rgb[2]-y*255)<=20;
  const picturePixels=async(name,label,points=[[0.25,0.25],[0.75,0.25],[0.25,0.75],[0.75,0.75]])=>{
    const result=await until(`${name} actual PNG pixels`,async()=>{
      const shot=await capture(name,label);return {ready:shot.card.rendered.media.ready,actual:imageSamples(shot,points)};
    },value=>value.ready && value.actual.every((rgb,i)=>matchesImage(rgb,points[i])));
    pixelsSeen[label]={points,...result};check(label,true);return points;
  };

  // The empty hint is published only after the first canonical reconciliation.
  await page.locator('.empty-canvas').waitFor();
  check('initial_replica_ready', true);

  // A command adds it: it shows by itself where it was told, and lands.
  let place = await spot();
  // A box of words: its words are what it shows (a description shows nowhere, cl-900).
  const added = await timed('add', { name: 'live_a', x: place.x, y: place.y }, () => project('add', 'text', 'live_a', `at=${place.x},${place.y},260`, 'text=Words from the CLI'));
  check('add_returns_its_name', added.value.name === 'live_a', added.value);
  check('add_lands', added.landing);
  check('add_in_the_status', (await held('live_a'))?.kind === 'text');

  // Its text. Let the actual arrival animation finish before taking the independent pixel baseline.
  await until('arrival animation finished',()=>canvasBox(page,'live_a'),card=>Boolean(card?.rendered) && !card.rendered.landing?.active);
  await settled(page);
  const textBefore=await capture('live_a','text-before');
  await timed('set_text', { name: 'live_a', has: 'Second words' }, () => project('set', 'live_a', 'text=Second words'));
  check('set_in_the_status', (await held('live_a')).fields?.text === 'Second words');
  const textAfter=await capture('live_a','text-after');
  const beforeWords=textBefore.card.rendered.labels.words,afterWords=textAfter.card.rendered.labels.words;
  let changed=0;
  if(beforeWords && afterWords && JSON.stringify(textBefore.camera)===JSON.stringify(textAfter.camera)) {
    const x=Math.min(beforeWords.x,afterWords.x),y=Math.min(beforeWords.y,afterWords.y),right=Math.max(beforeWords.x+beforeWords.width,afterWords.x+afterWords.width),bottom=Math.max(beforeWords.y+beforeWords.height,afterWords.y+afterWords.height);
    for(let py=y;py<bottom;py++)for(let px=x;px<right;px++){
      const a=textBefore.read(px*textBefore.dpr,py*textBefore.dpr),b=textAfter.read(px*textAfter.dpr,py*textAfter.dpr);
      if(a.some((value,i)=>Math.abs(value-b[i])>=25))changed++;
    }
  }
  pixelsSeen.changed_text={changed,before:beforeWords,after:afterWords};
  check('text_update_changes_actual_pixels',changed>=8 && (afterWords?.fullText ?? afterWords?.text)==='Second words',pixelsSeen.changed_text);


  // A picture.
  const picture = join(scratch, 'Live Picture.png');
  writeFileSync(picture, png(77));
  const imported = await timed('import', { name: 'live_pic' }, () => project('import', picture, 'live_pic'));
  check('import_lands', imported.landing);
  const hash = imported.value.sha256.replace('sha256:', '').slice(0, 12);

  // A file of the project's lifted out as an asset of its own, with its description.
  const lifted = await timed('lift_a_file', { name: 'live_lift' }, () => project('import', hash, 'live_lift', '--description', 'Lifted out'));
  check('lift_returns_its_name', lifted.value.name === 'live_lift', lifted.value);
  check('lift_keeps_its_words', (await held('live_lift'))?.fields?.description === 'Lifted out');

  // Another writer's edits, one after another.
  await timed('hand_text_in_place', { name: 'live_a', has: 'Hand words one' }, () => saved(() => projectJson(folder, 'PATCH', '/assets/live_a', { fields: { text: 'Hand words one' } })));
  await timed('hand_text_renamed_over', { name: 'live_a', has: 'Hand words two' }, () => saved(() => projectJson(folder, 'PATCH', '/assets/live_a', { fields: { text: 'Hand words two' } })));

  // A new box from another writer has no row: the page gives it a free place on screen, writes it, and it lands. Its tool is no package (its kind
  // given, so nothing reads it), so this runs anywhere.
  const handBox = { name: 'live_tool', tool: 'no-such-package-here', kind: 'image', fields: { instructions: 'Hand instructions' } };
  const handAdded = await timed('hand_add', { name: 'live_tool' }, () => saved(() => addBoxes(folder, [handBox])));
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
    const media=window.boxOf(name)?.rendered?.media;
    return media?.ready && media.width>0 && media.height>0;
  }, 'live_pic', { timeout: FIRST_RUN_MS });
  await picturePixels('live_pic','imported_picture_pixels');
  await timed('approve', { name: 'live_tool', picture: true }, () => project('approve', 'live_tool', hash));
  check('approve_in_the_status', (await held('live_tool')).pinned === true);
  const approvedPoints=await picturePixels('live_tool','approved_picture_pixels',[[0.25,0.1],[0.75,0.1],[0.25,0.25],[0.75,0.25]]);
  await timed('unapprove', { name: 'live_tool', picture: false }, () => project('unapprove', 'live_tool'));
  check('unapprove_in_the_status', !(await held('live_tool')).pinned);
  const unapproved=await capture('live_tool','unapproved-picture'),leftPixels=imageSamples(unapproved,approvedPoints);
  pixelsSeen.unapproved_picture={actual:leftPixels};
  check('unapprove_removes_actual_picture_pixels',!unapproved.card.rendered.media.ready && leftPixels.filter((rgb,i)=>!matchesImage(rgb,approvedPoints[i])).length>=2,leftPixels);

  // Moved and made wider by a command.
  place = await spot();
  await timed('move', { name: 'live_a', x: place.x, y: place.y, w: 320 }, () => project('set', 'live_a', `at=${place.x},${place.y},320`));
  check('its_place_reads_back', (await rows()).live_a.w === 320);

  // Renamed by a command: the same box, where it was, landing on nothing.
  const renamed = await timed('rename', { all: [{ name: 'live_b', x: place.x, y: place.y }, { gone: 'live_a' }] }, () => project('rename', 'live_a', 'live_b'));
  check('rename_does_not_land', !(await page.evaluate(() => window.boxOf('live_b')?.rendered?.landing?.active === true)), renamed);

  // Deleted by a command, which prints the command that undoes it. That brings it back where it was.
  const deleted = await timed('delete', { gone: 'live_b' }, () => project('delete', 'live_b'));
  const back = deleted.value.history;
  check('delete_has_an_undo_entry', Number.isInteger(back), deleted.value);
  check('delete_gone_from_the_status', !(await held('live_b')));
  await timed('undo', { name: 'live_b', x: place.x, y: place.y, w: 320 }, () => project('undo', back));
  check('undo_in_the_status', (await held('live_b'))?.fields?.text === 'Hand words two');

  // Another writer: a row, an asset gone, and its words.
  place = await spot();
  await timed('hand_row', { name: 'live_b', x: place.x, y: place.y }, () => saved(() => {
    projectJson(folder, 'PATCH', '/canvas', { cards: { 'live_b': { x: place.x, y: place.y } } });
  }));
  await timed('hand_delete', { gone: 'live_auto' }, () => saved(() => projectJson(folder, 'DELETE', '/assets/live_auto')));
  await timed('hand_mended', { name: 'live_b', has: 'Mended' }, () => saved(() => projectJson(folder, 'PATCH', '/assets/live_b', { fields: { text: 'Mended' } })));
  // Nothing so far was refused on purpose (the broken project.yaml this once cleared for is gone), so every refusal counts at the end.
  // A conflict asked of the person: Paul ruled none (cl-904, merged by character), whatever it would be called.
  const asked = async () => (await page.getByRole('alertdialog').count()) + (await box('live_tool').getByRole('button', { name: /keep mine|keep theirs|take the file/i }).count());

  // What's typed and not saved stays. A command changing another field of that asset leaves it to go on, and what's typed is saved over it.
  const words = box('live_tool').locator('.mention-text[data-field=instructions]');
  const typedText = () => words.evaluate((el) => el.textContent);
  await open('live_tool');
  await words.click();
  await page.keyboard.press('End');
  await page.keyboard.type(' TYPED-ONE');
  // It's behind More, so what shows is that the page read it.
  // The page's read of it, not its own save of what's typed (a PATCH to the same path).
  const readIt = page.waitForResponse((answer) => answer.request().method() === 'GET' && new URL(answer.url()).pathname.endsWith('/assets/live_tool'));
  const other = await project('set', 'live_tool', 'style=noir');
  await readIt;
  times.another_field = Date.now() - other.at;
  check('another_field_live', times.another_field <= LIVE_MS, times.another_field);
  await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
  check('another_field_leaves_it', (await typedText()).includes('TYPED-ONE'), await typedText());
  check('another_field_nothing_asked', (await asked()) === 0);
  await page.mouse.click(1700, 1100);
  await until('typed saved over it', async () => (await held('live_tool')).fields, (fields) => fields?.instructions?.includes('TYPED-ONE') && fields?.style === 'noir');
  check('typed_and_theirs_both_stand', true);

  // The command changes that field while what's typed is on its way: what's typed stands, then both merge by character, and typing goes on, with
  // nothing asked (cl-904, Paul 11-12: Automerge text). Its write waits until the command's is in, as a slower network's would.
  await open('live_tool');
  await words.click();
  const release = await holdWrites('live_tool');
  await page.keyboard.press('End');
  await page.keyboard.type(' TYPED-TWO');
  const typedBefore = await typedText();
  await project('set', 'live_tool', 'instructions=FROM-THE-CLI');
  check('theirs_in_the_file', (await held('live_tool')).fields?.instructions === 'FROM-THE-CLI');
  await page.waitForTimeout(LIVE_MS);
  check('typed_words_kept', (await typedText()).includes('TYPED-TWO'), { typed: await typedText(), typedBefore });
  await page.keyboard.type(' MORE');
  await release();
  await until('mine merged with theirs', async () => (await held('live_tool')).fields?.instructions, (text) => text?.includes('TYPED-TWO MORE') && text.includes('FROM-THE-CLI'));
  // The page shows the merged words, typing on where its caret was.
  const merged = await until('the page shows them merged', typedText, (text) => text.includes('FROM-THE-CLI') && text.endsWith('TYPED-TWO MORE')).catch(typedText);
  check('typing_goes_on_merged', merged.includes('FROM-THE-CLI') && merged.endsWith('TYPED-TWO MORE'), merged);
  await shot('merged');
  check('nothing_asked', (await asked()) === 0);

  // Renamed by a command while being typed in: the same box, and what's typed stays.
  await open('live_tool');
  await words.click();
  await page.keyboard.press('End');
  await page.keyboard.type(' TYPED-FOUR');
  await timed('rename_under_typing', { all: [{ name: 'live_held', has: 'TYPED-FOUR' }, { gone: 'live_tool' }] }, () => project('rename', 'live_tool', 'live_held'));
  await page.mouse.click(1700, 1100);
  await until('typed saved under its new name', async () => (await held('live_held'))?.fields?.instructions, (text) => text?.includes('TYPED-FOUR'));
  check('typed_survives_a_rename', true);

  // What the page does, the API reads at once.
  const readsBack = async (label, read, wanted) => {
    const from = Date.now();
    await until(label, read, wanted);
    times[`page_${label}`] = Date.now() - from;
    check(`page_${label}_reads_back`, times[`page_${label}`] <= LIVE_MS, times[`page_${label}`]);
  };
  // Measured at rest: a box worked on has the canvas drawn in on it, and leaving it moves everything back.
  await away();
  const moved = (await canvasBox(page,'live_b')).title;
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
  // Open, its name is Focus's heading's Rename (cl-893); the box's .nm is the canvas tile's.
  await page.locator('[data-focused-asset="live_b"] .focus-name').click();
  await page.waitForSelector('.nm-field');
  await page.keyboard.type('live_c');
  await page.keyboard.press('Enter');
  await readsBack('rename', () => held('live_c'), Boolean);
  await away();
  // An Edit is wide, and back at rest its X, at its right, can be past the window: F brings it all in, as for a person.
  await fitCanvas(page);
  await page.waitForTimeout(400);
  await hoverBox(page,'live_c');
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
  const cutFile = join(scratch, 'cut.json');
  writeFileSync(cutFile, JSON.stringify({ description: 'A cut from a file', timeline: { tracks: [{ clips: [{ clip: 'live_pic', frames: 24 }] }] } }));
  await project('add', 'cozy/edit', 'live_cut', `at=${place.x},${place.y},260`);
  await timed('edit_from_a_file', { name: 'live_cut', picture: true }, () => project('set', 'live_cut', '--input', cutFile));
  check('edit_reads_back', (await held('live_cut')).timeline?.tracks?.[0]?.clips?.[0]?.name === 'live_pic', (await held('live_cut')).timeline);
  await project('delete', 'live_cut');

  if (local) {
    // A render from the API, on the fixture tool: its progress on its card, moving, with no reload, and it lands. The first one only readies its environment.
    // Read with this machine's Runtime, as a run reads it: a box doesn't wait on PyPI's.
    await cozy('--json', 'run', `${TOOL}/still`, 'prompt=primed', '--on', 'local');
    place = await spot();
    // The machine estimates the overall bar from prior runs of this size.
    // Match the measured work while priming that estimate, then observe the next run.
    await project('add', `${TOOL}/slow`, 'live_shot', `at=${place.x},${place.y},260`, 'instructions=A slow still', 'seconds:=14');
    await until('live_shot on the canvas', () => canvasBox(page,'live_shot'), Boolean);
    await project('render', 'live_shot', '--on', 'local');
    await timed('a_first_render', { name: 'live_shot', picture: true }, async () => ({ at: Date.now() }));
    await project('set', 'live_shot', 'seconds:=14');
    // The first render over on the page too: its picture can show from its run before its take lands, and until the page reads that it's
    // landed, its box shows that run whole. Progress sampled from here is the next render's alone.
    await until('the first render over on the page', () => page.evaluate((name) => Boolean(window.boxOf(name)?.rendered) && !window.boxOf(name).rendered.progress, 'live_shot'), Boolean);
    const moving = bounded(page.evaluate((name) => window.progressOf(name), 'live_shot'), FIRST_RUN_MS, () => 'its progress never ended');
    await timed('render_starts', { name: 'live_shot', dot: 'busy' }, () => project('render', 'live_shot', '--on', 'local', '--queue'));
    const landed = until('two takes', async () => (await held('live_shot')).versions, (versions) => versions === 2, FIRST_RUN_MS).then(() => Date.now());
    const places = await moving;
    const shownAt = Date.now();
    seen.render_progress = places;
    const fractions = places.map((held) => held.match(/-?[\d.]+/)?.[0]).filter(Boolean).map(Number);
    check('render_progress_moves', fractions.length >= 3 && fractions.every((at, i) => !i || at >= fractions[i - 1]), places);
    // A count reads under the stage its package names (cl-898): `Drawing - 3 / 10`, never another stage's word for it, and no "step" or
    // "denoise" for a stage not known to denoise.
    const said = await page.evaluate(() => window.progressSaid);
    seen.render_said = said;
    const isCount = (text) => /\b\d+ \/ \d+$/.test(text);
    const firstCount = said.findIndex(isCount);
    check('counts_read_under_their_stage', firstCount >= 0 && said.filter(isCount).every((text) => /^Drawing - \d+ \/ 10$/.test(text)) && !said.some((text) => /denois/i.test(text)), said);
    const fileAt = await landed;
    times.render_lands = shownAt - fileAt;
    check('render_lands_live', times.render_lands <= LIVE_MS, times.render_lands);
    check('render_lands_a_second_take', !['queued','running'].includes((await canvasBox(page,'live_shot'))?.rendered?.state));
    await timed('approve_a_take', { name: 'live_shot', picture: true }, () => project('approve', 'live_shot', '2'));
    check('approve_a_take_in_the_status', (await held('live_shot')).approved === 2);
    await project('unapprove', 'live_shot');

    // A text asset's words stand wherever it's mentioned: changed by a command, they're on the open canvas, and what mentions them is out of date,
    // which no dot or line says at rest (Paul, 11-05).
    place = await spot();
    await project('add', 'text', 'live_mood', 'text=Soft morning light.');
    await project('add', `${TOOL}/still`, 'live_styled', `at=${place.x},${place.y},260`, 'instructions=A still in <live_mood>.');
    await project('render', 'live_styled', '--on', 'local');
    await shows({ name: 'live_styled', dot: 'none' });
    await timed('text_asset_outdates', { all: [{ name: 'live_mood', has: 'Hard evening light.' }, { name: 'live_styled', dot: 'none' }] }, () => project('set', 'live_mood', 'text=Hard evening light.'));
    check('what_mentions_it_is_out_of_date', (await held('live_styled')).standing === 'out_of_date', await held('live_styled'));
    await project('delete', 'live_styled', 'live_mood');

    // What waits its turn says so, and when it has landed no longer does: what's said of the project besides its assets goes when it's over.
    await project('set', 'live_shot', 'seconds:=3');
    place = await spot();
    await project('add', `${TOOL}/still`, 'live_after', `at=${place.x},${place.y},260`, 'instructions=After <live_shot>.');
    const waiting = shows({ name: 'live_after', dot: 'busy' });
    await project('render', 'live_shot', 'live_after', '--on', 'local', '--queue');
    await waiting;
    const afterAt = await until('what waited landed', async () => (await held('live_after')).versions, (versions) => versions === 1, FIRST_RUN_MS).then(() => Date.now());
    const afterDone = await shows({ name: 'live_after', dot: 'none' });
    times.queued_goes = afterDone.when - afterAt;
    check('queued_goes_when_it_lands', times.queued_goes <= LIVE_MS, times.queued_goes);
    await project('delete', 'live_after');

    // Deleted while it renders: its run stops, nothing's left queued, and the box goes. The line it printed brings it back with its takes.
    await project('set', 'live_shot', 'seconds:=60');
    await timed('long_render_starts', { name: 'live_shot', dot: 'busy' }, () => project('render', 'live_shot', '--on', 'local', '--queue'));
    await until('it running', async () => (await held('live_shot')).making?.length, Boolean);
    const stopped = await timed('delete_while_rendering', { gone: 'live_shot' }, () => project('delete', 'live_shot'));
    const going = await until('nothing owed', async () => (await status()).queue ?? [], (queue) => queue.length === 0, 30_000);
    check('delete_stops_its_render', going.length === 0, going);
    await timed('undo_after_a_render', { name: 'live_shot' }, () => project('undo', stopped.value.history));
    check('undo_keeps_its_takes', (await held('live_shot')).versions === 3, await held('live_shot'));

  }
  // A description, on what has a picture: kept in the file, and shown nowhere, open or not (cl-900).
  await open('live_pic');
  await project('set', 'live_pic', 'description=Said by the CLI');
  await until('the description in the file', async () => (await held('live_pic'))?.fields?.description, (said) => said === 'Said by the CLI');
  await page.waitForTimeout(500);
  check('a_description_is_shown_nowhere', !(await page.locator('body').innerText()).includes('Said by the CLI'));
  await away();
  await shot('single');

  // A burst: 200 edits as fast as they go. Twelve boxes on screen are written and moved, each by its own line of commands so the last is known,
  // and 40 come with no place. Few reads, shown as it goes, right at the end with no reload, and frames sampled through it.
  const fixed = Array.from({ length: 12 }, (_, i) => `burst_${i}`);
  // Clear of everything so far, and all of it fitted on the screen. A row apart by more than an Edit at rest and the room it keeps under it
  // for its add-ons and the name of a box below (e139a61c6, shown.ts): about 460 at 260 wide. Closer, the page moves one off the other.
  const ROW = 500;
  const addedBurst = await Promise.all(fixed.map((name, i) => project('add', 'text', name, `at=${260 + (i % 4) * 300},${2000 + Math.floor(i / 4) * ROW},260`, 'text=start')));
  // Fitted to all of them: a box that comes after a fit is drawn for a moment, then not, off the screen (macOS, 41955999465).
  try {
    await until('the boxes for the burst', async () => {
      await fitCanvas(page);
      await page.waitForTimeout(250);
      return page.evaluate((names) => names.every((name) => {
        const card=window.boxOf(name),at=card?.bounds;
        return card?.rendered && at.x>=0 && at.y>=0 && at.y+at.height<=innerHeight && at.x+at.width<=innerWidth;
      }), fixed);
    }, Boolean);
  } catch (error) {
    const state = await status();
    const cards = await rows();
    seen.burst_setup = {
      requested: fixed,
      replies: addedBurst.map(({ at, value }) => ({ at, name: value?.name })),
      backend: state.assets.filter((asset) => asset.name.startsWith('burst_')).map(({ name, kind }) => ({ name, kind })),
      rows: Object.fromEntries(Object.entries(cards).filter(([name]) => name.startsWith('burst_'))),
      page: await page.evaluate((names) => ({
        focused: document.activeElement?.tagName,
        viewport: document.querySelector('.canvas-root')?.canvasInspection?.camera(),
        boxes: names.map((name) => {
          const node = window.boxOf(name);
          return {name,position:node?.position,rendered:Boolean(node?.rendered),rect:node?.bounds};
        }),
      }), fixed),
    };
    throw error;
  }
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
      lane.push(['set', fixed[which], `text=${text}`]);
    }
  }
  for (let more = 0; more < 40; more++) lanes[more % lanes.length].push(['add', 'cozy/edit', `arrived_${more}`, 'description=Arrived in the burst']);
  check('burst_is_200_edits', lanes.flat().length === 200, lanes.flat().length);
  await page.evaluate((name) => window.watching(name), fixed[0]);
  const [projectReads, headReads, canvasReads, assetReads] = [reads.project, reads.heads, reads.canvas, reads.assets];
  const cdp = await context.newCDPSession(page);
  await cdp.send('Performance.enable');
  const threadSample = async () => {
    const { metrics } = await cdp.send('Performance.getMetrics');
    const task = metrics.find(({ name }) => name === 'TaskDuration')?.value;
    const at = metrics.find(({ name }) => name === 'Timestamp')?.value;
    if (!Number.isFinite(task) || task < 0 || !Number.isFinite(at) || at < 0) throw new Error('invalid Performance TaskDuration/Timestamp sample');
    return { TaskDuration: task, Timestamp: at };
  };
  const busyFrom = await threadSample();
  const started = Date.now();
  const lasts = await Promise.all(lanes.map(async (lane) => {
    let at = 0;
    for (const words of lane) ({ at } = await project(...words));
    return at;
  }));
  const ended = Math.max(...lasts);
  const right = { all: fixed.map((name, i) => ({ name, has: last.get(name), x: targets[i].x, y: targets[i].y })) };
  const { when } = await shows(right, STEP_MS);
  const busyTo = await threadSample();
  const task_s = busyTo.TaskDuration - busyFrom.TaskDuration;
  const elapsed_s = busyTo.Timestamp - busyFrom.Timestamp;
  if (!Number.isFinite(task_s) || task_s < 0 || !Number.isFinite(elapsed_s) || elapsed_s <= 0) throw new Error('invalid Performance TaskDuration/Timestamp interval');
  const thread = task_s / elapsed_s;
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
    thread_mixed: +(task_s * 1000 / (when - started)).toFixed(2),
    thread_samples: { before: busyFrom, after: busyTo, task_s, elapsed_s, started_ms: started, shown_ms: when },
  };
  seen.burst = burst;
  check('burst_right_in_a_second', burst.right_after_ms <= LIVE_MS, burst);
  check('burst_reads_coalesced', burst.project_reads === 0 && burst.head_reads <= 20 && burst.asset_reads <= 200, burst);
  check('burst_shown_as_it_goes', burst.shown_along_the_way >= 3, { burst, texts: texts.slice(0, 8) });
  check('burst_not_thrashing', burst.thread <= BURST_THREAD, burst);
  const after = await status();
  for (const [name, text] of last) if (after.assets.find((asset) => asset.name === name)?.fields?.text !== text) check(`burst_${name}_in_the_file`, false, text);
  const arrived = after.assets.filter((asset) => asset.name.startsWith('arrived_'));
  check('burst_all_arrived', arrived.length === 40, arrived.length);
  // Fitted to what's there as the last of them find their places.
  await until('every box placed and shown', async () => {
    await fitCanvas(page);
    await page.waitForTimeout(250);
    return page.evaluate(() => {const i=document.querySelector('.canvas-root')?.canvasInspection;return i?.names().filter(name=>name.startsWith('arrived_') && i.card(name)?.rendered).length ?? 0;});
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
  // Somewhere clear in view: the canvas draws only the boxes in view, so a move past its edge would never show.
  await fitCanvas(page);
  await settled(page);
  const clearOfAll = await spotInView();
  await cdp.send('Debugger.enable');
  await cdp.send('Debugger.pause');
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
  await project('set', fixed[0], 'text=SAID-WHILE-BEHIND');
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
  await fitCanvas(page);
  await until('what was made while behind', async () => (await canvasBox(page,'made_while_behind'))?.rendered, Boolean);
  check('fell_behind_nothing_lost', (await held(fixed[0])).fields?.text === 'SAID-WHILE-BEHIND' && (await held('made_while_behind'))?.fields?.description === 'Made while behind');
  // The stream is live again: a command shows as before.
  await timed('live_after_falling_behind', { name: fixed[0], has: 'AND-LIVE-AGAIN' }, () => project('set', fixed[0], 'text=AND-LIVE-AGAIN'));

  check('never_reloaded', documents === 0 && (await page.evaluate(() => window.notReloaded === true)), { documents });
  check('one_stream_throughout', streams === 0, { streams });
  check('csp_holds', (await page.evaluate(() => window.violations)).length === 0, await page.evaluate(() => window.violations));
  await new Promise((done) => setTimeout(done, 300));
  // An asset gone by the time the page read it is no fault: that's how it hears it's gone.
  const unasked = refused.filter((refusal) => refusal.url.includes('/v1/projects/') && !(refusal.code === 'asset_unknown' && refusal.method === 'GET'));
  check('nothing_refused_unasked', unasked.length === 0, unasked);
  check('no_page_errors', errors.length === 0, { errors, refused });

  // A read of what's there isn't an arrival.
  await page.reload();
  await until('its displayed boxes', () => page.evaluate(() => {const i=document.querySelector('.canvas-root')?.canvasInspection;return i?.names().filter(name=>i.card(name)?.rendered).length ?? 0;}), count=>count>10);
  const arrivals=await page.evaluate(()=>{const i=document.querySelector('.canvas-root').canvasInspection;return i.names().filter(name=>i.card(name)?.rendered?.landing?.active===true);});
  check('a_reload_lands_nothing', arrivals.length===0,arrivals);
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
