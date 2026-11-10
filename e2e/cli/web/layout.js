// How the canvas and what's around it are laid out (Paul, 11-06: "avoid errors and overlaps; make sure there's good spacing and everything is legible"), in a real browser
// on a real Calcifer, measured: every kind of box (a picture with and without a tool, a shot with beats, text, an Edit, a long name) at rest, under the pointer and selected;
// open on a take, written in, with More and its warnings open; failed, with many takes and an add-on, on a crowded canvas; far out, at the home zoom and close; one being
// made, saying how far it's got, at 75%, where what hangs off a box at rest keeps its size on screen; at 1280×720,
// 1600×1000 and 2560×1440 at 2×. And the frame around the canvas: the top bar, the tools, the drawers, a note, the menus, the names list. Anything that collides, is cut off,
// pokes past its edge, touches its neighbour or its panel's edge, reads too small, or is a square shadow under a drawn edge fails it, each said where, with a picture.
// With scrollbars shown, one anywhere fails. Rastered on the GPU path as a person's Chrome is (page.js GPU), a drawn edge
// whose line is gone from a side, or that draws past it, fails, pictured with it and without it. An open box's panel spans its picture and takes, a gap
// under the picture and before the takes, through zooms by the wheel, the zoom control and the keys. Where nothing is drawn the pointer finds the canvas,
// and a drag there pans it.
//   node layout.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $COZY: the CLI; $SHOT: where to keep its pictures; $ALONE: on a shared computer, so it runs nothing;
// $PAGE_ORIGIN: the page from elsewhere (a `vite` of the tree under test), its API through it; $PROJECT: one an earlier pass made, measured again; $SIZES: some of
// the sizes, by name. Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { execFileSync, spawn } = require('child_process');
const { mkdtempSync, readFileSync, renameSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { BARS, GPU, calcifer, MP4, boxOf, closeBox, openBox, STEP_MS, lastLook, launch, lookAt, png, noStall, renderOf, settled, until, wav, importYaml, yamlOf } = require('./page');

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
/** Ends what's still being made, once the measuring is done. */
let stopMaking = () => {};
async function died(why) {
  failures.push(`${during}: ${why}`);
  stopMaking();
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
function audit({ floor, hangZoom, fitted = true }) {
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
      // Clear of a drawn or ruled edge: a control's own outline mustn't sit on it. Scrolled within the panel, it meets the top or bottom where it fades.
      if (framed && !el.closest('.takes') && (r.left - box.left < pad || box.right - r.right < pad || (!scrolls && !scrolled(el, panel) && (r.top - box.top < pad || box.bottom - r.bottom < pad))) && !panel.matches('.top, .here-side, .zoom, .ptabs')) {
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
  const hung = (node) => [node, ...node.querySelectorAll('.head > *, .ratio-rest, .add-on, .refused-line')].filter(visible).map(R);
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
    // What's under it, one block with its picture and its takes: from the picture's left edge to the takes' right, a gap under the picture
    // and before the takes, and on neither.
    const under = node.querySelector('.here-under');
    if (under && visible(under)) {
      const u = R(under);
      const side = node.querySelector('.here-side:not(:empty)');
      const t = side && visible(side) ? R(side) : null;
      const gap = parseFloat(getComputedStyle(under).getPropertyValue('--here-gap')) * scaleOf(under);
      if (Math.abs(u.left - m.left) > 1.5) say('under-left', under, `${Math.round(u.left - m.left)} px from its picture's left edge`);
      if (Math.abs(u.right - (t ?? m).right) > 1.5) say('under-right', under, `${Math.round(u.right - (t ?? m).right)} px from ${t ? "its takes'" : "its picture's"} right edge`);
      if (u.top - m.bottom < gap - 1) say('under-gap', under, `${Math.round(u.top - m.bottom)} px under its picture, not ${Math.round(gap)}`);
      if (t && t.left - m.right < gap - 1) say('takes-gap', side, `${Math.round(t.left - m.right)} px beside its picture, not ${Math.round(gap)}`);
      if (t && meet(u, t, 0)) say('under-on-takes', under, `${round(u)} on its takes ${round(t)}`);
    }
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
  const floats = ['.top .history', '.top .ptabs', '.top .mtabs', '.top .canvas-controls', '.top .main-menu > summary', '.top .pill', '.tools', '.zoom', '.notes'].flatMap((s) => [...document.querySelectorAll(s)]).filter(visible);
  for (let i = 0; i < floats.length; i++) for (let j = i + 1; j < floats.length; j++) if (meet(R(floats[i]), R(floats[j]))) say('chrome-overlap', floats[i], `on ${where(floats[j])}`);
  // A box being worked on, as the canvas fits it, isn't under what floats: zoomed in past that by hand, it's as near as was asked.
  for (const el of fitted ? document.querySelectorAll('.card.here > .media, .card.here > .tl, .card.here > .head, .here-under, .here-side') : []) {
    if (!visible(el)) continue;
    for (const over of document.querySelectorAll('.top .history, .top .ptabs, .top .mtabs, .top .main-menu > summary, .tools, .zoom')) if (visible(over) && meet(R(el), R(over), 2)) say('under-chrome', el, `under ${where(over)}`);
  }
  // A shadow or fill that's square under a drawn edge, or a tint where there's nothing.
  for (const el of document.querySelectorAll('.here-under *, .drawer *, .tools *, .top *')) {
    if (!visible(el) || el.closest(OVER_PICTURE)) continue;
    const s = getComputedStyle(el);
    if (s.borderImageSource !== 'none' && s.boxShadow !== 'none') say('square-shadow', el, 'a drawn edge with a square shadow');
  }
  // 4. No bar on what scrolls. A drawn edge's sides stretch: tiled, the GPU draws them shifted out at a fractional scale.
  for (const el of document.querySelectorAll('body *')) {
    const s = getComputedStyle(el);
    if (s.borderImageSource !== 'none' && !/^stretch( stretch)?$/.test(s.borderImageRepeat) && visible(el)) say('tiled-edge', el, `its drawn edge's sides ${s.borderImageRepeat}`);
    if (!/auto|scroll/.test(s.overflowX + s.overflowY) || !visible(el)) continue;
    const bar = Math.max(el.offsetWidth - el.clientWidth - parseFloat(s.borderLeftWidth) - parseFloat(s.borderRightWidth), el.offsetHeight - el.clientHeight - parseFloat(s.borderTopWidth) - parseFloat(s.borderBottomWidth));
    const more = el.scrollHeight > el.clientHeight + 1 || el.scrollWidth > el.clientWidth + 1;
    if (bar > 0.5) say('scrollbar', el, `a ${Math.round(bar)} px bar, ${more ? 'with more to scroll' : 'with all of it in view'}`);
  }
  return found;
}

/** Where nothing is drawn, the pointer finds the canvas: no box, column, panel or wrapper takes it unseen. Each point of `around` (else the
 *  window) `step` pixels apart. Self-contained: it runs in the page. */
function emptyHits({ around, step }) {
  const alpha = (color) => {
    const m = color.match(/rgba?\(([^)]+)\)/);
    const parts = m ? m[1].split(/[ ,/]+/).filter(Boolean) : [];
    return parts.length > 3 ? Number(parts[3]) : m ? 1 : 0;
  };
  /** Whether `el` itself draws anything at x, y. */
  const paints = (el, x, y) => {
    const s = getComputedStyle(el);
    if (s.visibility !== 'visible' || Number(s.opacity) === 0) return false;
    // A control's whole box is what's pressed, whatever of it is drawn.
    if (el.matches('img, video, canvas, svg, input, select, textarea, hr, button, a, summary, label, [role=button], [role=slider]') || s.backgroundImage !== 'none' || s.borderImageSource !== 'none' || alpha(s.backgroundColor) > 0.05) return true;
    for (const pseudo of ['::before', '::after']) {
      const p = getComputedStyle(el, pseudo);
      if (p.content !== 'none' && (alpha(p.backgroundColor) > 0.05 || p.backgroundImage !== 'none' || parseFloat(p.borderTopWidth) > 0)) return true;
    }
    const r = el.getBoundingClientRect();
    const k = r.width / (el.offsetWidth || r.width || 1);
    const edges = [['Top', y - r.top], ['Right', r.right - x], ['Bottom', r.bottom - y], ['Left', x - r.left]];
    if (edges.some(([side, d]) => parseFloat(s[`border${side}Width`]) * k >= d && alpha(s[`border${side}Color`]) > 0.05)) return true;
    // A label's box is the label.
    return [...el.childNodes].some((node) => node.nodeType === 3 && node.textContent.trim());
  };
  const where = (el) => `${el.tagName.toLowerCase()}${[...el.classList].slice(0, 3).map((c) => `.${c}`).join('')}`;
  const found = new Map();
  const [x0, y0, x1, y1] = around ?? [0, 0, innerWidth, innerHeight];
  for (let y = Math.max(1, y0); y < Math.min(innerHeight - 1, y1); y += step) {
    for (let x = Math.max(1, x0); x < Math.min(innerWidth - 1, x1); x += step) {
      const stack = document.elementsFromPoint(x, y);
      const pane = stack.findIndex((el) => el.matches('.react-flow__pane'));
      if (pane <= 0 || stack.slice(0, pane).some((el) => paints(el, x, y))) continue;
      const at = `${where(stack[0])}${stack[0].closest('.react-flow__node .card') ? ` <${stack[0].closest('.react-flow__node .card').dataset.asset}>` : ''}`;
      if (!found.has(at)) found.set(at, [Math.round(x), Math.round(y)]);
    }
  }
  return [...found].map(([at, [x, y]]) => ({ rule: 'unseen-blocker', at, what: `takes the pointer at ${x},${y}, where nothing is drawn, from the canvas` }));
}

/** Each drawn edge on top, as the GPU draws it: its line on all four sides, and nothing of it past the line. Each is pictured with it and
 *  without it, the page held still, and what it draws is where the two differ; `decoder` reads the pictures. */
async function drawnEdges(page, decoder, label) {
  const frames = await page.evaluate(() => {
    const all = [...document.querySelectorAll('body *')].filter((el) => getComputedStyle(el).borderImageSource !== 'none' && el.checkVisibility({ opacityProperty: true, visibilityProperty: true }));
    const out = [];
    for (const el of all) {
      if (all.some((other) => other !== el && other.contains(el))) continue;
      const r = el.getBoundingClientRect();
      const scale = r.width / el.offsetWidth;
      const o = parseFloat(getComputedStyle(el).borderImageOutset) * scale;
      if (r.width < 40 || r.height < 30 || r.left - o < 1 || r.top - o < 1 || r.right + o > innerWidth - 1 || r.bottom + o > innerHeight - 1) continue;
      // Along each side clear of its corners, where nothing else is over it.
      const samples = [];
      for (const side of ['top', 'right', 'bottom', 'left']) {
        for (const t of [0.3, 0.5, 0.7]) {
          const x = side === 'left' ? r.left + scale : side === 'right' ? r.right - scale : r.left + t * r.width;
          const y = side === 'top' ? r.top + scale : side === 'bottom' ? r.bottom - scale : r.top + t * r.height;
          const top = document.elementFromPoint(x, y);
          if (top && el.contains(top)) samples.push({ side, x, y });
        }
      }
      el.dataset.drawnEdge = String(out.length);
      out.push({ at: `${el.tagName.toLowerCase()}${[...el.classList].slice(0, 3).map((c) => `.${c}`).join('')}`, rect: [r.left, r.top, r.right, r.bottom], o, scale, samples });
    }
    // Still: what opens is at its end (the GPU path's compositor holds it at its start), and what goes on forever is held.
    for (const held of document.getAnimations()) if (held.timeline === document.timeline && held.effect?.getComputedTiming().iterations !== Infinity) held.finish();
    window.heldStill = document.getAnimations().filter((held) => held.playState === 'running' && held.timeline === document.timeline);
    for (const held of window.heldStill) held.pause();
    return out;
  });
  const said = [];
  const proofs = [];
  for (const [i, frame] of frames.entries()) {
    const [left, top, right, bottom] = frame.rect;
    const pad = frame.o + 3;
    const { width, height } = page.viewportSize();
    const [x, y] = [Math.max(0, Math.floor(left - pad)), Math.max(0, Math.floor(top - pad))];
    const clip = { x, y, width: Math.min(width, Math.ceil(right + pad)) - x, height: Math.min(height, Math.ceil(bottom + pad)) - y };
    const shown = await page.screenshot({ clip });
    await page.evaluate((i) => (document.querySelector(`[data-drawn-edge="${i}"]`).style.visibility = 'hidden'), i);
    const bare = await page.screenshot({ clip });
    await page.evaluate((i) => document.querySelector(`[data-drawn-edge="${i}"]`).style.removeProperty('visibility'), i);
    const found = await decoder.evaluate(edgesOf, { shown: shown.toString('base64'), bare: bare.toString('base64'), clip, frame });
    for (const { rule, what } of found) said.push({ rule, at: frame.at, what });
    if (found.length && process.env.SHOT) proofs.push({ i, frame, clip, viewport: { width, height }, shown, bare, found });
  }
  await page.evaluate(() => {
    for (const el of document.querySelectorAll('[data-drawn-edge]')) delete el.dataset.drawnEdge;
    for (const held of window.heldStill) held.play();
  });
  // Keep the exact comparison inputs after this pass has finished measuring, without another screenshot or DOM read.
  for (const { i, frame, clip, viewport, shown, bare, found } of proofs) {
    const path = `${process.env.SHOT}-${label}-edge-${i}`;
    writeFileSync(`${path}-drawn.png`, shown);
    writeFileSync(`${path}-bare.png`, bare);
    writeFileSync(`${path}-frame.json`, JSON.stringify({ label, frame, clip, viewport, findings: found }, null, 2));
  }
  return said;
}

/** A frame's sides from its two pictures, with it and without it: on each, its line is drawn within a few pixels of its edge, and nothing of
 *  it lies past that. Self-contained: it runs in the decoder's page. */
async function edgesOf({ shown, bare, clip, frame }) {
  const read = async (base64) => {
    const bitmap = await createImageBitmap(await (await fetch(`data:image/png;base64,${base64}`)).blob());
    const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
    const context = canvas.getContext('2d');
    context.drawImage(bitmap, 0, 0);
    return context.getImageData(0, 0, bitmap.width, bitmap.height);
  };
  const [a, b] = [await read(shown), await read(bare)];
  const k = a.width / clip.width;
  const differs = (x, y) => {
    const [px, py] = [Math.round((x - clip.x) * k), Math.round((y - clip.y) * k)];
    if (px < 0 || py < 0 || px >= a.width || py >= a.height) return 0;
    const i = (py * a.width + px) * 4;
    return Math.max(Math.abs(a.data[i] - b.data[i]), Math.abs(a.data[i + 1] - b.data[i + 1]), Math.abs(a.data[i + 2] - b.data[i + 2]));
  };
  const [left, top, right, bottom] = frame.rect;
  const sides = {};
  for (const { side, x, y } of frame.samples) {
    const [dx, dy] = { top: [0, -1], right: [1, 0], bottom: [0, 1], left: [-1, 0] }[side];
    const [ex, ey] = { top: [x, top], right: [right, y], bottom: [x, bottom], left: [left, y] }[side];
    const most = (from, to) => {
      let held = 0;
      for (let d = from; d <= to; d += 0.5 / k) held = Math.max(held, differs(ex + dx * d, ey + dy * d));
      return held;
    };
    const tally = (sides[side] ??= { at: 0, line: 0, past: 0 });
    tally.at++;
    // The line wobbles from 3 px inside its edge to 2 px out; past it to the edge of its picture, nothing.
    if (most(-4 * frame.scale, 2.5 * frame.scale) > 80) tally.line++;
    if (frame.o - 0.5 > 3.5 * frame.scale && most(3.5 * frame.scale, frame.o - 0.5) > 12) tally.past++;
  }
  const found = [];
  for (const [side, { at, line, past }] of Object.entries(sides)) {
    if (line < at) found.push({ rule: 'edge-gone', what: `its ${side} line isn't drawn at ${at - line} of ${at} places` });
    if (at > 1 && past === at) found.push({ rule: 'past-edge', what: `it draws outside its ${side} line` });
  }
  return found;
}

(async () => {
  const base = process.env.PAGE_ORIGIN ?? calcifer();
  const browser = await launch(playwright, BARS);
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
  importYaml(folder, `${yamlOf(folder).trimEnd()}
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
  making:
    tool: ${shotsPy}/scenes
    plate: <clip>
    summary: <hero_art> walks the shore, segment after segment, as the tide comes in and the light goes long across the sand.
    segments: 600
    each: 5
` : ''}`);
  names = ['hero_art', 'wide_art', 'tall_art', LONG, 'sea_sound', 'clip', 'clip_b', 'style', 'still', 'edit', 'odd', ...(local ? ['card', 'posed', 'shot', 'broken', 'making'] : [])];
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
    importYaml(folder, yamlOf(folder).replace(/(\n  still:\n    video: <clip>\n    at: )[\d.]+/, `$1${at}`));
    render('still');
  }
  render('edit');
  const box = (asset) => boxOf(page, asset);
  if (local) {
    // Its first render waits on its install, which the box says.
    render('card', false);
    const waiting = await until('the picture tool waits', () => box('card').locator('.prog b, .take.making').count(), (n) => n > 0, RUN_MS).catch(() => 0);
    seen.waits = waiting > 0;
    if (waiting) for (const finding of await page.evaluate(audit, { floor: FLOOR, hangZoom: HANG_ZOOM })) findings.push({ size: '1600x1000', state: 'waiting-on-install', ...finding });
    await until('the first take', () => api(`/v1/projects/${id}`), (state) => state.assets.find((held) => held.name === 'card')?.prefixes?.length >= 1, RUN_MS);
    for (let i = 0; i < 8; i++) render('card');
    render('shot');
    render('shot');
    render('broken');
  }
  await until('every take landed', () => api(`/v1/projects/${id}`), (state) => !state.assets.some((held) => held.standing === 'making'), RUN_MS);
  // One being made all through the measuring, saying how far it's got, its steps a second each.
  if (local) {
    render('making', false);
    stopMaking = () => execFileSync(COZY, ['project', '-p', folder, 'cancel', 'making'], { stdio: 'ignore' });
  }
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
      await page.waitForFunction(() => !document.getAnimations().some((held) => held.playState === 'running' && held.timeline === document.timeline && held.effect?.getComputedTiming().iterations !== Infinity), null, { timeout: 5000 }).catch(() => {});
      seen.looks = (seen.looks ?? 0) + 1;
      const said = await page.evaluate(audit, { floor: FLOOR, hangZoom: HANG_ZOOM, fitted: !/^zoom-.*-in$/.test(state) });
      // The pointer where nothing's drawn, around a box worked on or all over.
      if (state === 'rest' || /^(open|writing|zoom)-/.test(state)) {
        const block = await page.evaluate(() => {
          const rects = [...document.querySelectorAll('.card.here, .card.here > .here-side, .card.here > .here-under')].map((el) => el.getBoundingClientRect()).filter((r) => r.width);
          return rects.length ? [Math.min(...rects.map((r) => r.left)) - 60, Math.min(...rects.map((r) => r.top)) - 60, Math.max(...rects.map((r) => r.right)) + 60, Math.max(...rects.map((r) => r.bottom)) + 60] : null;
        });
        said.push(...(await page.evaluate(emptyHits, { around: block, step: block ? 10 : 24 })));
      }
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
    // At rest, being made, at 75%: what it says of how far it's got hangs off it as all else at rest does, one size on screen.
    const making = page.locator('.react-flow__node .card[data-asset="making"] > .media').first();
    await home();
    if (names.includes('making') && (await making.count())) {
      const at = await making.boundingBox();
      await page.keyboard.down('Shift');
      await page.mouse.click(at.x + at.width / 2, at.y + at.height / 2);
      await page.keyboard.up('Shift');
      await page.keyboard.press('Shift+2');
      await settled(page);
      await page.mouse.click(2, size.viewport.height - 2);
      await zoomTo(0.75);
      const said = await until('it says how far', () => making.locator('.prog b').innerText().catch(() => ''), (text) => /steps/i.test(text)).catch(() => '');
      check(`making_says_how_far_${size.name}`, /steps/i.test(said), said);
      await look('rest-making', true);
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
    // Zoomed with a box open, by the wheel, the zoom control and the keys: what's under it stays one block with its picture and takes. Then a
    // drag from beside its takes, where nothing's drawn, pans the canvas.
    for (const asset of [many, 'wide_art', 'tall_art']) {
      await openBox(page, asset);
      await settled(page);
      const wheel = async (dy) => {
        const at = await page.locator(`.card.here[data-asset="${asset}"] > .media`).boundingBox();
        await page.mouse.move(at.x + at.width / 2, at.y + at.height / 2);
        await page.keyboard.down('Control');
        for (let i = 0; i < 3; i++) await page.mouse.wheel(0, dy);
        await page.keyboard.up('Control');
      };
      for (const [step, act] of [
        ['wheel-out', () => wheel(120)],
        ['wheel-in', () => wheel(-160)],
        ['control-out', () => page.click('button[aria-label="Zoom out"]')],
        ['control-in', () => page.click('button[aria-label="Zoom in"]')],
        ['key-out', () => page.keyboard.press('Control+Minus')],
        ['key-in', () => page.keyboard.press('Control+Equal')],
      ]) {
        await act();
        await settled(page);
        await look(`zoom-${asset}-${step}`, step.endsWith('out'));
      }
      // Beside its takes where only the canvas is: a box pushed aside may sit close by, and a drag on it would carry it onto this one.
      const from = await page.evaluate(() => {
        const side = document.querySelector('.card.here > .here-side:not(:empty)')?.getBoundingClientRect() ?? document.querySelector('.card.here > .media').getBoundingClientRect();
        const y = side.top + side.height * 0.8;
        const bare = (x) => document.elementFromPoint(x, y)?.classList.contains('react-flow__pane');
        for (let x = side.right + 30; x < innerWidth - 20; x += 20) if (bare(x)) return { x, y };
        for (let x = side.left - 30; x > 20; x -= 20) if (bare(x)) return { x, y };
        return { x: side.right + 30, y };
      });
      const viewport = () => page.evaluate(() => document.querySelector('.react-flow__viewport').style.transform);
      const before = await viewport();
      await page.mouse.move(from.x, from.y);
      await page.mouse.down();
      await page.mouse.move(from.x - 60, from.y - 40, { steps: 6 });
      await page.mouse.up();
      await settled(page);
      const after = await viewport();
      if (after === before) findings.push({ size: size.name, state: `zoom-${asset}`, rule: 'drag-no-pan', at: `beside <${asset}>'s takes`, what: `a drag from ${Math.round(from.x)},${Math.round(from.y)} left the canvas where it was` });
      await closeBox(page);
    }
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

  // As a person's Chrome draws it, on the GPU path: each drawn edge whole at rest, on a box open, zoomed out and written in, and in a drawer; and on a
  // 1.25x screen, where every edge is drawn at a fractional scale.
  const drawn = await launch(playwright, GPU);
  const decoder = await drawn.newPage();
  const asked = SIZES.filter((held) => !process.env.SIZES || process.env.SIZES.split(',').includes(held.name));
  for (const size of [...asked, { name: '1600x1000@1.25', viewport: { width: 1600, height: 1000 }, deviceScaleFactor: 1.25 }]) {
    const context = await drawn.newContext({ viewport: size.viewport, deviceScaleFactor: size.deviceScaleFactor });
    page = await context.newPage();
    watch(page);
    lookAt(page);
    await signedIn(page);
    await page.goto(`${base}/#/projects/${id}`);
    await page.locator('.react-flow__node').first().waitFor();
    await placed();
    const edges = async (state) => {
      // Still for a second: on the GPU path a box's fit can start well after its click.
      const at = () => page.evaluate(() => document.querySelector('.react-flow__viewport')?.style.transform);
      for (let last = await at(), same = 0; same < 10; ) {
        await page.waitForTimeout(100);
        const now = await at();
        same = now === last ? same + 1 : 0;
        last = now;
      }
      seen.looks = (seen.looks ?? 0) + 1;
      const said = await drawnEdges(page, decoder, `${size.name}-${state}`);
      for (const finding of said) findings.push({ size: `${size.name} drawn`, state, ...finding });
      if (process.env.SHOT && said.length) {
        const path = `${process.env.SHOT}-${size.name}-drawn-${state}.png`;
        await page.screenshot({ path, scale: 'css' });
        shots.push(path);
      }
    };
    await edges('rest');
    const many = names.includes('card') ? 'card' : 'still';
    await openBox(page, many);
    await edges(`open-${many}`);
    for (const step of ['out', 'out-again']) {
      await page.click('button[aria-label="Zoom out"]');
      await settled(page);
      await edges(`open-${many}-zoomed-${step}`);
    }
    const next = page.locator(`.card.here[data-asset="${many}"] .here-side .take.add`);
    if (await next.count()) {
      await (await renderOf(page, many)).waitFor();
      await settled(page);
      await edges(`writing-${many}`);
    }
    await closeBox(page);
    for (const drawer of ['runs', 'packages']) {
      await page.goto(`${base}/#/projects/${id}/${drawer}`);
      await page.locator('.drawer').waitFor();
      await page.waitForTimeout(400);
      await edges(`drawer-${drawer}`);
    }
    await context.close();
  }
  await drawn.close();
  stopMaking();

  summed();
  seen.shots = shots.length;
  check('page_errors', errors.length === 0, errors.slice(0, 10));
  for (const rule of new Set(findings.map((held) => held.rule))) check(`no_${rule.replace(/-/g, '_')}`, false, seen.found.filter((held) => held.rule === rule).length);
  check('laid_out', findings.length === 0, findings.length);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch(died);
