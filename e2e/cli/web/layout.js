// How the canvas and what's around it are laid out (Paul, 11-06: "avoid errors and overlaps; make sure there's good spacing and everything is legible"), in a real browser
// on a real Calcifer, measured: every kind of box (a picture with and without a tool, a shot with beats, text, an Edit, a long name) at rest, under the pointer and selected;
// open on a take, written in, with More and its warnings open; failed, with many takes and an add-on, on a crowded canvas; far out, at the home zoom and close; at 1280×720,
// 1600×1000 and 2560×1440 at 2×. And the frame around the canvas: the top bar, the tools, the drawers, a note, the menus, the names list. Anything that collides, is cut off,
// pokes past its edge, touches its neighbour or its panel's edge, reads too small, or is a square shadow under a drawn edge fails it, each said where, with a picture.
//   node layout.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $COZY: the CLI; $SHOT: where to keep its pictures; $ALONE: on a shared computer, so it runs nothing;
// $PAGE_ORIGIN: the page from elsewhere (a `vite` of the tree under test), its API through it; $PROJECT: one an earlier pass made, measured again; $SIZES: some of
// the sizes, by name. Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { execFileSync, spawn } = require('child_process');
const { mkdtempSync, readFileSync, renameSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { calcifer, MP4, boxOf, closeBox, openBox, STEP_MS, lastLook, launch, lookAt, png, noStall, renderOf, settled, until, wav } = require('./page');

const failures = [];
const seen = {};
// Every finding: a size, a state, a rule, where, and what.
const findings = [];
noStall(seen, died);
let during = 'the start';
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  during = name;
  if (!ok) failures.push(name);
}
async function died(why) {
  failures.push(`${during}: ${why}`);
  summed();
  await lastLook();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(1);
}

const COZY = process.env.COZY;
const RUN_MS = 300_000;
const drawPy = join(__dirname, '..', 'project', 'draw.py');
const shotsPy = join(__dirname, '..', 'project', 'shots.py');
const SIZES = [
  { name: '1280x720', viewport: { width: 1280, height: 720 }, deviceScaleFactor: 1 },
  { name: '1600x1000', viewport: { width: 1600, height: 1000 }, deviceScaleFactor: 1 },
  // Drawn at 2×, in software on a runner: each box under the pointer and selected is looked at in the two above.
  { name: '2560x1440@2x', viewport: { width: 2560, height: 1440 }, deviceScaleFactor: 2, open: true },
];
const LONG = 'a_box_whose_name_goes_on_and_on_past_any_width';
/** The least a word may be drawn on screen, in CSS pixels. */
const FLOOR = 10;
/** Past this zoom what hangs off a box at rest shrinks with the canvas (shown.ts, HANG). */
const HANG_ZOOM = 0.5;

/** Each finding once, with every state it was seen in. */
function summed() {
  const once = {};
  for (const { rule, at, what, size, state } of findings) (once[`${rule} ${at}`] ??= { rule, at, what, seen: [] }).seen.push(`${size} ${state}`);
  seen.found = Object.values(once).map((held) => ({ ...held, seen: held.seen.slice(0, 6), times: held.seen.length }));
}

function replaceFile(at, text) {
  writeFileSync(`${at}.part`, text);
  renameSync(`${at}.part`, at);
}

/** What's wrong with how `scope` is laid out, in the page: each finding a rule, where, and what. A rule is one person's eye on it, measured.
 *  Self-contained: it runs in the page. */
function audit({ floor, hangZoom }) {
  const found = [];
  const R = (el) => el.getBoundingClientRect();
  const zoom = Number(document.querySelector('.react-flow__viewport')?.style.transform.match(/scale\(([\d.]+)\)/)?.[1] ?? 1);
  const name = (el) => el.closest?.('.react-flow__node')?.querySelector('.card')?.dataset.asset;
  const where = (el) => {
    const path = [];
    for (let e = el; e && e !== document.body && path.length < 3; e = e.parentElement) {
      const label = e.getAttribute('aria-label');
      path.unshift(`${e.tagName.toLowerCase()}${[...e.classList].slice(0, 2).map((c) => `.${c}`).join('')}${label ? `[${label.slice(0, 24)}]` : ''}`);
    }
    const box = name(el);
    return `${box ? `<${box}> ` : ''}${path.join(' > ')}`;
  };
  const say = (rule, el, what) => found.push({ rule, at: where(el), what });
  const round = (r) => [Math.round(r.left), Math.round(r.top), Math.round(r.width), Math.round(r.height)];
  const visible = (el) => el.checkVisibility?.({ opacityProperty: true, visibilityProperty: true }) && R(el).width >= 1 && R(el).height >= 1;
  const onScreen = (r) => r.right > 0 && r.bottom > 0 && r.left < innerWidth && r.top < innerHeight;
  const meet = (a, b, by = 1) => Math.min(a.right, b.right) - Math.max(a.left, b.left) > by && Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top) > by;
  const within = (a, b, by = 1) => a.left >= b.left - by && a.top >= b.top - by && a.right <= b.right + by && a.bottom <= b.bottom + by;
  const grow = (r, by) => ({ left: r.left - by, top: r.top - by, right: r.right + by, bottom: r.bottom + by });
  const alpha = (color) => {
    const m = color.match(/rgba?\(([^)]+)\)/);
    if (!m) return 1;
    const parts = m[1].split(/[ ,/]+/).filter(Boolean);
    return parts.length > 3 ? Number(parts[3]) : 1;
  };
  /** The scale it's drawn at: a box's words and what's under it are drawn larger and smaller with the canvas and its own transforms. */
  const scaleOf = (el) => (el.offsetWidth ? R(el).width / el.offsetWidth : el.offsetHeight ? R(el).height / el.offsetHeight : 1);
  const ownText = (el) => [...el.childNodes].some((n) => n.nodeType === 3 && n.textContent.trim());
  /** Whether something scrolls it, inside `panel`: what's scrolled out of view is there to scroll to. */
  const scrolled = (el, panel) => {
    for (let e = el.parentElement; e && e !== panel; e = e.parentElement) if (/auto|scroll/.test(getComputedStyle(e).overflowY + getComputedStyle(e).overflowX)) return true;
    return false;
  };
  /** A control drawn as a box of its own: a border, a fill or a drawn edge. Two plain words side by side, or a segmented control's parts, don't touch. */
  const boxed = (el) => {
    const s = getComputedStyle(el);
    return s.borderImageSource !== 'none' || alpha(s.backgroundColor) > 0.05 || (parseFloat(s.borderTopWidth) > 0 && alpha(s.borderTopColor) > 0.05);
  };
  // What a panel is: the box, what's under it and beside it, and each thing floating over the canvas.
  const PANELS = '.here-under, .here-side, .drawer, .top, .tools, .zoom, .zoom-menu, .main-menu nav, .asset-menu, .name-menu, .activity, .notes .note, .ptabs';
  const CONTROLS = 'button, input, select, textarea, summary, a.tb, .chip, .add-on, [role=slider], [contenteditable=true]';
  // What's drawn over a picture, on purpose.
  const OVER_PICTURE = '.media, .tl-film, .take, .slot-media';

  // 1. Text: legible on screen, and not cut off unless it says so with an ellipsis.
  const texts = [...document.querySelectorAll('body *')].filter((el) => ownText(el) && visible(el) && onScreen(R(el)) && !el.closest('.ptabs-ghost, .tool-ghost, [aria-hidden=true]'));
  for (const el of texts) {
    const style = getComputedStyle(el);
    const px = parseFloat(style.fontSize) * scaleOf(el);
    // Further out than --hang stops, what hangs off a box at rest shrinks with the canvas, on purpose: only its words in a frame stay one size.
    const shrinks = el.closest('.react-flow__node') && !el.closest('.card.here') && zoom < hangZoom;
    if (px < floor && !shrinks) say('illegible', el, `${px.toFixed(1)} px on screen: "${el.textContent.trim().slice(0, 30)}"`);
    const clips = /hidden|clip/.test(style.overflowX);
    // An ellipsis is drawn only by a block of text: on a flex or grid box the text is cut where it ends.
    const ellipsis = style.textOverflow === 'ellipsis' && !/flex|grid/.test(style.display);
    if (clips && el.scrollWidth > el.clientWidth + 1 && !ellipsis) say('clipped', el, `its text is ${el.scrollWidth} wide in ${el.clientWidth}, cut with no ellipsis`);
    // Lines cut off below are whole lines: none is cut through its middle.
    const line = parseFloat(style.lineHeight);
    if (/hidden|clip/.test(style.overflowY) && line && el.scrollHeight > el.clientHeight + 1) {
      const cut = (el.clientHeight - parseFloat(style.paddingTop)) % line;
      if (cut > 1 && cut < line - 1) say('line-cut', el, `its last line shows ${Math.round((cut / line) * 100)}% of itself`);
    }
  }

  // 2. Each panel: what's in it stays inside it, is not cut off by it unless it scrolls, and clear of its edge and its neighbours.
  for (const panel of document.querySelectorAll(PANELS)) {
    if (!visible(panel)) continue;
    const box = R(panel);
    const ps = getComputedStyle(panel);
    const scrolls = /auto|scroll/.test(ps.overflowY) || /auto|scroll/.test(ps.overflowX);
    const framed = ps.borderImageSource !== 'none' || parseFloat(ps.borderTopWidth) > 0;
    if (ps.borderImageSource !== 'none' && ps.boxShadow !== 'none') say('square-shadow', panel, 'a drawn edge with a square shadow: a grey box under it');
    const controls = [...panel.querySelectorAll(CONTROLS)].filter((el) => visible(el) && el.closest(PANELS) === panel);
    const scale = scaleOf(panel);
    const pad = 2 * scale;
    for (const el of controls) {
      const r = R(el);
      if (!within(r, box, 1)) {
        if (!scrolls && !scrolled(el, panel)) say('outside', el, `${round(r)} past its panel ${round(box)}`);
        continue;
      }
      // Clear of a drawn or ruled edge: a control's own outline mustn't sit on it.
      if (framed && !el.closest('.takes') && (r.left - box.left < pad || box.right - r.right < pad || (!scrolls && (r.top - box.top < pad || box.bottom - r.bottom < pad))) && !panel.matches('.top, .here-side, .zoom, .ptabs')) {
        say('edge', el, `${Math.round(Math.min(r.left - box.left, box.right - r.right, r.top - box.top, box.bottom - r.bottom) / scale)} px from its panel's edge`);
      }
    }
    // Controls side by side or one over another: never touching, never on each other.
    for (let i = 0; i < controls.length; i++) {
      for (let j = i + 1; j < controls.length; j++) {
        const [a, b] = [controls[i], controls[j]];
        if (a.contains(b) || b.contains(a)) continue;
        if ((a.matches('.take-off') && b.closest('.take-at') === a.parentElement) || (b.matches('.take-off') && a.closest('.take-at') === b.parentElement)) continue;
        if (a.closest(OVER_PICTURE) && a.closest(OVER_PICTURE) === b.closest(OVER_PICTURE)) continue;
        const [ra, rb] = [R(a), R(b)];
        if (meet(ra, rb)) say('overlap', a, `on ${where(b)}: ${round(ra)} and ${round(rb)}`);
        else if (a.parentElement === b.parentElement && boxed(a) && boxed(b) && meet(grow(ra, 1.5 * scale), rb, 0) && !a.closest('.beats')) say('touching', a, `touches ${where(b)}`);
      }
    }
    // Rings (a take's on, what's approved) and outlines cut off by a panel that clips them.
    if (/hidden|clip|auto|scroll/.test(ps.overflowX + ps.overflowY)) {
      for (const el of panel.querySelectorAll('.take.on, .take.approved, .media, button:focus-visible')) {
        if (!visible(el)) continue;
        const s = getComputedStyle(el);
        const ring = s.outlineStyle !== 'none' ? parseFloat(s.outlineWidth) + parseFloat(s.outlineOffset) : 0;
        if (!ring) continue;
        const r = grow(R(el), ring * scaleOf(el));
        const inner = { left: box.left + panel.clientLeft * scale, top: box.top + panel.clientTop * scale, right: box.left + (panel.clientLeft + panel.clientWidth) * scale, bottom: box.top + (panel.clientTop + panel.clientHeight) * scale };
        // A scroller cuts what's scrolled off along its own axis, which is scrolling; across it, a ring cut off is a ring cut off.
        const across = /auto|scroll/.test(ps.overflowY) ? r.left < inner.left - 0.5 || r.right > inner.right + 0.5 : r.top < inner.top - 0.5 || r.bottom > inner.bottom + 0.5;
        const first = /auto|scroll/.test(ps.overflowY) && panel.scrollTop === 0 && r.top < inner.top - 0.5;
        if (across || first) say('ring-cut', el, `its ${ring}px ring is cut off by ${where(panel)}`);
      }
    }
  }

  // 3. Each box: its name and × clear of its picture, its takes level with it, and what hangs under it on nothing else's.
  const boxes = [...document.querySelectorAll('.react-flow__node')].filter((node) => visible(node) && onScreen(R(node)));
  // A box and all that hangs off it: its name rises above it as it's drawn larger, and what's under it hangs below.
  const hung = (node) => [node, ...node.querySelectorAll('.head > *, .ratio-rest, .rest-waiting, .add-on, .refused-line')].filter(visible).map(R);
  for (const node of boxes) {
    const card = node.querySelector('.card');
    const media = node.querySelector(':scope .card > .media, :scope .card > .tl');
    if (!card || !media) continue;
    const m = R(media);
    const ms = getComputedStyle(media);
    const ring = (ms.outlineStyle !== 'none' ? parseFloat(ms.outlineWidth) + parseFloat(ms.outlineOffset) : 1) * scaleOf(media);
    for (const el of node.querySelectorAll('.head > *')) {
      if (!visible(el)) continue;
      const r = R(el);
      if (r.bottom > m.top - ring + 0.5 && r.right > m.left && r.left < m.right) say('on-picture', el, `${Math.round(r.bottom - (m.top - ring))} px into its picture's edge`);
    }
    // Its takes level with it, while the column isn't scrolled.
    const side = node.querySelector('.here-side .takes > :first-child');
    if (side && visible(side) && !node.querySelector('.here-side').scrollTop) {
      const s = R(side);
      const ss = getComputedStyle(side.matches('.take') ? side : side.querySelector('.take') ?? side);
      const ringOn = ss.outlineStyle !== 'none' ? (parseFloat(ss.outlineWidth) + parseFloat(ss.outlineOffset)) * scaleOf(side) : 0;
      if (s.top - ringOn < m.top - 1) say('takes-above', side, `its first take starts ${Math.round(m.top - s.top + ringOn)} px above its picture`);
      if (s.top > m.top + 2 + ringOn) say('takes-below', side, `its first take starts ${Math.round(s.top - m.top)} px below its picture's top`);
    }
    const under = node.querySelector('.here-under');
    if (under && visible(under) && R(under).top < m.bottom + 2) say('under-on-picture', under, `what's under it starts ${Math.round(m.bottom - R(under).top)} px up its picture`);
  }
  // At rest, no box on another, with what hangs under each.
  const resting = boxes.filter((node) => !node.querySelector('.card.here') && !node.classList.contains('dragging'));
  for (let i = 0; i < resting.length; i++) {
    for (let j = i + 1; j < resting.length; j++) {
      const [a, b] = [hung(resting[i]), hung(resting[j])];
      if (a.some((ra) => b.some((rb) => meet(ra, rb)))) say('boxes-overlap', resting[i], `with <${name(resting[j])}>`);
    }
  }
  // What floats over the canvas, on nothing else that floats.
  const floats = ['.top .ptabs', '.top .mtabs', '.top .canvas-controls', '.top .main-menu > summary', '.top .pill', '.tools', '.zoom', '.notes'].flatMap((s) => [...document.querySelectorAll(s)]).filter(visible);
  for (let i = 0; i < floats.length; i++) for (let j = i + 1; j < floats.length; j++) if (meet(R(floats[i]), R(floats[j]))) say('chrome-overlap', floats[i], `on ${where(floats[j])}`);
  // A box being worked on isn't under what floats.
  for (const el of document.querySelectorAll('.card.here > .media, .card.here > .tl, .card.here > .head, .here-under, .here-side')) {
    if (!visible(el)) continue;
    for (const over of document.querySelectorAll('.top .ptabs, .top .mtabs, .top .main-menu > summary, .tools, .zoom')) if (visible(over) && meet(R(el), R(over), 2)) say('under-chrome', el, `under ${where(over)}`);
  }
  // A shadow or fill that's square under a drawn edge, or a tint where there's nothing.
  for (const el of document.querySelectorAll('.here-under *, .drawer *, .tools *, .top *')) {
    if (!visible(el) || el.closest(OVER_PICTURE)) continue;
    const s = getComputedStyle(el);
    if (s.borderImageSource !== 'none' && s.boxShadow !== 'none') say('square-shadow', el, 'a drawn edge with a square shadow');
  }
  return found;
}

(async () => {
  const base = process.env.PAGE_ORIGIN ?? calcifer();
  const browser = await launch(playwright);
  const holder = JSON.parse(readFileSync(process.env.HOLDER, 'utf8'));
  const signedIn = async (page) => {
    // `cozy open --no-open`'s link, on the page under test.
    const said = execFileSync(COZY, ['open', '--no-open'], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
    const link = said.match(/http:\/\/\S+#code=[0-9a-f]+\S*/)?.[0];
    if (!link) throw new Error(`cozy open --no-open printed no link: ${said.slice(0, 300)}`);
    await page.goto(link.replace(/^http:\/\/[^/]+/, base));
    await page.waitForSelector('.top');
    return page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session')).token);
  };
  const errors = [];
  const watch = (page) => {
    page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
    page.setDefaultTimeout(STEP_MS);
  };

  // The project: a box of each kind, made as a person makes them.
  const first = await browser.newContext({ viewport: SIZES[1].viewport });
  let page = await first.newPage();
  watch(page);
  lookAt(page);
  const token = await signedIn(page);
  const api = async (path, init = {}) => (await fetch(`http://${holder.address}${path}`, { ...init, headers: { Authorization: `Bearer ${token}`, ...init.headers } })).json();
  const shots = [];
  // $PROJECT: one an earlier pass made, measured again as it is.
  let id = process.env.PROJECT;
  let names;
  const placed = async () => {
    // Placed, each: off the view a box isn't drawn, so it's counted by its row.
    await until('a row for each', async () => (await api(`/v1/projects/${id}/canvas`)).cards ?? {}, (cards) => names.every((held) => cards[held]));
    await page.keyboard.press('f');
    await settled(page);
  };
  if (id) names = (await api(`/v1/projects/${id}`)).assets.map((held) => held.name);
  else {
  await page.goto(`${base}/#/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-layout-${Date.now()}`);
  await page.click('form button[type=submit]');
  await page.waitForURL(/#\/projects\/[^/]+$/);
  id = page.url().match(/#\/projects\/([^/]+)/)[1];
  const folder = (await api(`/v1/projects/${id}`)).path;
  seen.project = { id, folder };
  const yamlAt = join(folder, 'project.yaml');
  const scratch = mkdtempSync(join(tmpdir(), 'web-layout-'));
  for (const [asset, file, bytes] of [
    ['hero_art', 'hero.png', png(40, 320, 200, true)],
    ['wide_art', 'wide.png', png(120, 640, 360)],
    ['tall_art', 'tall.png', png(200, 240, 420)],
    [LONG, 'long.png', png(160, 400, 300)],
    ['sea_sound', 'sea.wav', wav()],
    ['clip', 'clip.mp4', MP4],
    ['clip_b', 'clip_b.mp4', MP4],
  ]) {
    writeFileSync(join(scratch, file), bytes);
    execFileSync(COZY, ['project', '-p', folder, 'import', join(scratch, file), asset], { encoding: 'utf8' });
  }
  const local = (await api('/v1/status')).local_usable && !process.env.ALONE;
  seen.ran = local;
  replaceFile(yamlAt, `${readFileSync(yamlAt, 'utf8').trimEnd()}
  style:
    text: Bright, hand-painted, cel-shaded, with soft light and long shadows across every surface, and the sea always in the distance.
  still:
    video: <clip>
    at: 0.5
    tool: cozy/frame
  edit:
    tool: cozy/edit
    tracks:
      - clips:
          - <clip>
          - <clip_b>
  odd: {kind: widget}
${local ? `  card:
    tool: ${drawPy}/draw
    instructions: <style> <hero_art> stands on the shore, with <clip> playing behind and a long line of words that runs past the frame's edge.
  posed:
    tool: ${drawPy}/draw
    instructions: <hero_art>, posed.
    lora: [{ref: hf://someone/wushu-style-lora, strength: 0.8}, {ref: hf://someone/ink-wash-with-a-long-name, strength: 0.6}]
  shot:
    tool: ${shotsPy}/shoot
    summary: |
      <hero_art> looks hard at <clip>.
    beats:
      - duration: 4
        description: |
          <wide_art> waits on the shore.
      - duration: 3
        description: |
          It goes on, and on.
  broken:
    tool: ${shotsPy}/crash
    plate: <clip>
    crash: the fixture's own failure, said in full so it wraps
` : ''}`);
  names = ['hero_art', 'wide_art', 'tall_art', LONG, 'sea_sound', 'clip', 'clip_b', 'style', 'still', 'edit', 'odd', ...(local ? ['card', 'posed', 'shot', 'broken'] : [])];
  await placed();
  const render = (asset, wait = true) => {
    const words = ['project', '-p', folder, 'render', asset, ...(local ? ['--on', 'local'] : []), '--plain'];
    if (!wait) return spawn(COZY, words, { stdio: 'ignore' });
    try {
      execFileSync(COZY, words, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], timeout: RUN_MS });
    } catch (error) {
      if (asset !== 'broken') throw error;
    }
  };
  // Takes: the frame thrice, the Edit once, and where this computer runs, many of the picture tool's, the shot's two and a failure.
  for (const at of [0.2, 0.5, 0.8]) {
    replaceFile(yamlAt, readFileSync(yamlAt, 'utf8').replace(/(\n  still:\n    video: <clip>\n    at: )[\d.]+/, `$1${at}`));
    render('still');
  }
  render('edit');
  const box = (asset) => boxOf(page, asset);
  if (local) {
    // Its first render waits on its install, which the box says.
    render('card', false);
    const waiting = await until('the picture tool waits', () => box('card').locator('.rest-waiting, .take.making').count(), (n) => n > 0, RUN_MS).catch(() => 0);
    seen.waits = waiting > 0;
    if (waiting) for (const finding of await page.evaluate(audit, { floor: FLOOR, hangZoom: HANG_ZOOM })) findings.push({ size: '1600x1000', state: 'waiting-on-install', ...finding });
    await until('the first take', () => api(`/v1/projects/${id}`), (state) => state.assets.find((held) => held.name === 'card')?.prefixes?.length >= 1, RUN_MS);
    for (let i = 0; i < 8; i++) render('card');
    render('shot');
    render('shot');
    render('broken');
  }
  await until('every take landed', () => api(`/v1/projects/${id}`), (state) => !state.assets.some((held) => held.standing === 'making'), RUN_MS);
  }
  await first.close();

  // Each size: the canvas at rest at each zoom, each box in each state, and what's around the canvas.
  // $SIZES: some of them, by name, for a quick look.
  for (const size of SIZES.filter((held) => !process.env.SIZES || process.env.SIZES.split(',').includes(held.name))) {
    const context = await browser.newContext({ viewport: size.viewport, deviceScaleFactor: size.deviceScaleFactor });
    page = await context.newPage();
    watch(page);
    lookAt(page);
    await signedIn(page);
    await page.goto(`${base}/#/projects/${id}`);
    await page.locator('.react-flow__node').first().waitFor();
    await placed();
    const look = async (state, keep) => {
      // What opens animates in: measured once it's still. A pulse goes on forever, and is no move.
      await page.waitForFunction(() => !document.getAnimations().some((held) => held.playState === 'running' && held.effect?.getComputedTiming().iterations !== Infinity), null, { timeout: 5000 }).catch(() => {});
      const said = await page.evaluate(audit, { floor: FLOOR, hangZoom: HANG_ZOOM });
      for (const finding of said) findings.push({ size: size.name, state, ...finding });
      if (process.env.SHOT && (said.length || keep)) {
        const path = `${process.env.SHOT}-${size.name}-${state.replace(/[^\w.-]+/g, '_')}.png`;
        await page.screenshot({ path, scale: 'css' });
        shots.push(path);
      }
      return said;
    };
    const zoom = () => page.evaluate(() => Number(document.querySelector('.zoom-at')?.textContent?.replace('%', '')) / 100 || 0);
    const zoomTo = async (want) => {
      for (let i = 0; i < 20; i++) {
        const now = await zoom();
        if (Math.abs(now - want) < 0.005) break;
        await page.click(now > want ? 'button[aria-label="Zoom out"]' : 'button[aria-label="Zoom in"]');
        await page.waitForTimeout(80);
      }
      await settled(page);
    };
    const home = async () => {
      await closeBox(page);
      await page.mouse.move(2, size.viewport.height - 2);
      await page.keyboard.press('f');
      await settled(page);
    };

    await home();
    await look('rest', true);
    for (const [label, want] of [['far', 0.15], ['mid', 0.33]]) {
      await zoomTo(want);
      await look(`rest-${label}`, true);
    }
    // Close, on a box with the most hanging off it: drawn in on it, then 200%.
    await home();
    const near = page.locator(`.react-flow__node .card[data-asset="${names.includes('posed') ? 'posed' : 'still'}"] > .media`).first();
    const middle = await near.boundingBox();
    if (middle) {
      await page.keyboard.down('Shift');
      await page.mouse.click(middle.x + middle.width / 2, middle.y + middle.height / 2);
      await page.keyboard.up('Shift');
      await page.keyboard.press('Shift+2');
      await settled(page);
      await page.mouse.click(2, size.viewport.height - 2);
      await zoomTo(2);
      await look('rest-close', true);
    }
    await home();
    for (const asset of size.open ? [] : names) {
      const media = page.locator(`.react-flow__node .card[data-asset="${asset}"] > .media, .react-flow__node .card[data-asset="${asset}"] > .tl`).first();
      if (!(await media.count())) continue;
      const at = await media.boundingBox();
      if (!at) continue;
      await page.mouse.move(at.x + at.width / 2, at.y + at.height / 2);
      await page.waitForTimeout(120);
      await look(`hover-${asset}`);
      await page.keyboard.down('Shift');
      await page.mouse.click(at.x + at.width / 2, at.y + at.height / 2);
      await page.keyboard.up('Shift');
      await page.waitForTimeout(120);
      await look(`selected-${asset}`);
      await page.mouse.click(2, size.viewport.height - 2);
      await page.mouse.move(2, size.viewport.height - 2);
    }
    for (const asset of names) {
      await openBox(page, asset);
      await page.waitForTimeout(300);
      await look(`open-${asset}`, ['card', 'shot', 'still', 'edit', 'style', 'odd', 'broken', LONG].includes(asset));
      const here = page.locator(`.card.here[data-asset="${asset}"]`);
      const more = here.locator('.more-toggle');
      if (await more.count()) {
        await more.first().click();
        await settled(page);
        await look(`open-${asset}-more`, true);
        await more.first().click();
      }
      const warned = here.locator('.head .iconb.warned');
      if (await warned.count()) {
        await warned.click();
        await settled(page);
        await look(`open-${asset}-warnings`, true);
      }
      const next = here.locator('.here-side .take.add');
      if (await next.count()) {
        await (await renderOf(page, asset)).waitFor();
        await settled(page);
        await look(`writing-${asset}`, true);
        // The names list, from a < typed in its words.
        const words = here.locator('.in-frame .mention-text');
        if (asset === 'card' && (await words.count())) {
          await words.click();
          await page.keyboard.press('End');
          await page.keyboard.type(' <');
          await page.locator('.name-menu').waitFor({ timeout: 3000 }).catch(() => {});
          await look('names-list', true);
          await page.keyboard.press('Escape');
          await page.keyboard.press('Backspace');
          await page.keyboard.press('Backspace');
        }
      }
      await closeBox(page);
    }
    // A note, over the canvas: a take discarded says so and offers Undo, which this takes.
    const many = names.includes('card') ? 'card' : 'still';
    await openBox(page, many);
    const oldest = page.locator(`.card.here[data-asset="${many}"] .here-side .take-at`).last();
    await oldest.hover();
    await oldest.locator('.take-off').click();
    await page.locator('.notes .note').first().waitFor();
    await look('note', true);
    await page.locator('.notes .note button', { hasText: 'Undo' }).first().click();
    await closeBox(page);
    // What's around the canvas.
    await home();
    const chip = page.locator('.mtabs button, .mtabs a').first();
    if (await chip.count()) {
      await chip.click();
      await page.waitForTimeout(200);
      await look('machine-activity', true);
      await page.keyboard.press('Escape');
      await page.mouse.click(2, size.viewport.height - 2);
    }
    await page.click('summary[aria-label=Menu]');
    await look('main-menu', true);
    await page.click('summary[aria-label=Menu]');
    await page.click('.zoom-at');
    await look('zoom-menu', true);
    await page.keyboard.press('Escape');
    const named = page.locator(`.react-flow__node .card[data-asset="wide_art"] > .media`).first();
    const at = await named.boundingBox();
    if (at) {
      await page.mouse.click(at.x + at.width / 2, at.y + at.height / 2, { button: 'right' });
      await page.locator('.asset-menu').waitFor({ timeout: 3000 }).catch(() => {});
      await look('asset-menu', true);
      await page.keyboard.press('Escape');
    }
    for (const drawer of ['machines', 'runs', 'account', 'projects', 'packages']) {
      await page.goto(`${base}/#/projects/${id}/${drawer}`);
      await page.locator('.drawer').waitFor();
      await page.waitForTimeout(400);
      await look(`drawer-${drawer}`, true);
    }
    await page.goto(`${base}/#/projects/${id}`);
    await context.close();
  }

  summed();
  seen.shots = shots.length;
  check('page_errors', errors.length === 0, errors.slice(0, 10));
  for (const rule of new Set(findings.map((held) => held.rule))) check(`no_${rule.replace(/-/g, '_')}`, false, seen.found.filter((held) => held.rule === rule).length);
  check('laid_out', findings.length === 0, findings.length);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch(died);
