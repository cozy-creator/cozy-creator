// How the canvas and what's around it are laid out (Paul, 11-06: "avoid errors and overlaps; make sure there's good spacing and everything is legible"), in a real browser
// on a real Calcifer, measured: every kind of box (a picture with and without a tool, a shot with beats, text, an Edit, a long name) at rest, under the pointer and selected;
// open on a take, written in, with More and its warnings open; failed, with many takes and an add-on, on a crowded canvas; far out, at the home zoom and close; one being
// made, saying how far it's got, at 75%; at 1280×720,
// 1600×1000 and 2560×1440 at 2×. And the frame around the canvas: the top bar, the tools, the drawers, a note, the menus, the names list. Anything that collides, is cut off,
// pokes past its edge, touches its neighbour or its panel's edge, reads too small, or is a square shadow under a drawn edge fails it, each said where, with a picture.
// With scrollbars shown, one anywhere fails. Rastered on the GPU path as a person's Chrome is (page.js GPU), a drawn edge
// whose line is gone from a side, or that draws past it, fails, pictured with it and without it. An open box is in Focus (cl-893): its picture zoomed by
// the wheel, its zoom control and the keys leaves what's around it where it was and uncovered, and a drag on it enlarged pans it. Where nothing is drawn
// on the canvas the pointer finds it, and a drag there pans it.
//   node layout.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $COZY: the CLI; $SHOT: where to keep its pictures; $ALONE: on a shared computer, so it runs nothing;
// $PAGE_ORIGIN: the page from elsewhere (a `vite` of the tree under test), its API through it; $PROJECT: one an earlier pass made, measured again; $SIZES: some of
// the sizes, by name (including 1600x1000@1.25 for the fractional raster check). Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { mkdtempSync, readFileSync, renameSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { projectFromPage, projectUrl, signIn,importFile, projectAction, projectJson, BARS, GPU, calcifer, MP4, boxOf, closeBox, openBox, STEP_MS, lastLook, launch, lookAt, png, noStall, renderOf, settled, until, wav, addBoxes } = require('./page');

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
const DRAWN_SIZES = [...SIZES, { name: '1600x1000@1.25', viewport: { width: 1600, height: 1000 }, deviceScaleFactor: 1.25 }];
const requestedSizes = process.env.SIZES ? process.env.SIZES.split(',') : undefined;
if (requestedSizes?.some((name) => !DRAWN_SIZES.some((size) => size.name === name))) throw new Error(`unknown layout size: ${process.env.SIZES}`);
const selectedSizes = SIZES.filter((size) => !requestedSizes || requestedSizes.includes(size.name));
const selectedDrawnSizes = DRAWN_SIZES.filter((size) => !requestedSizes || requestedSizes.includes(size.name));
const LONG = 'a_box_whose_name_goes_on_and_on_past_any_width';
/** The least a word may be drawn, in CSS pixels: on screen, or on the canvas at 100%, where all of a box is drawn at the canvas's scale (cl-890). */
const FLOOR = 10;

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

/** A resting canvas at 200%: a corner may hit a box, and a zoom button's move may still be on its way. */
async function restClose(page) {
  const state = () => page.evaluate(() => {
    const transform = document.querySelector('.react-flow__viewport')?.style.transform;
    return {
      hash: location.pathname,
      open: [...document.querySelectorAll('.card.here')].map((card) => card.dataset.asset),
      transform,
      zoom: transform ? new DOMMatrix(transform).a : null,
    };
  });
  const requireRest = (at) => {
    if (at.hash.includes('/assets/') || at.open.length) throw new Error(`rest-close expected a resting canvas: ${JSON.stringify(at)}`);
  };
  await closeBox(page);
  requireRest(await state());
  if (!(await page.locator('.zoom-menu').count())) await page.click('.zoom-at');
  await page.getByRole('menuitem', { name: 'Zoom to 200%', exact: true }).click();
  await until('rest-close reaches 200%', state, (at) => {
    requireRest(at);
    return at.zoom === 2;
  }, 5000);
  await settled(page);
  const at = await state();
  requireRest(at);
  if (at.zoom !== 2) throw new Error(`rest-close expected 200%: ${JSON.stringify(at)}`);
  return at;
}

/** What's wrong with how `scope` is laid out, in the page: each finding a rule, where, and what. A rule is one person's eye on it, measured.
 *  Self-contained: it runs in the page. */
function audit({ floor, fitted = true }) {
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
    // A box's words grow and shrink with the canvas: they're read at 100%.
    const onCanvas = Boolean(el.closest('.react-flow__node'));
    const px = (parseFloat(style.fontSize) * scaleOf(el)) / (onCanvas ? zoom : 1);
    if (px < floor) say('illegible', el, `${px.toFixed(1)} px ${onCanvas ? 'at 100%' : 'on screen'}: "${el.textContent.trim().slice(0, 30)}"`);
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
  const hung = (node) => [node, ...node.querySelectorAll('.head > *, .ratio-rest, .tile-under, .refused-line')].filter(visible);
  for (const node of boxes) {
    const card = node.querySelector('.card');
    const media = node.querySelector(':scope .card > .media, :scope .card > .tl');
    if (!card || !media) continue;
    const m = R(media);
    const ms = getComputedStyle(media);
    // The painted frame may letterbox scene-scaled content so its controls can stay readable.
    const content = media.querySelector(':scope > .media-pixels');
    if (card.classList.contains('here') && content && visible(content)) {
      const ink = R(content);
      if (ink.left < m.left - 1.5 || ink.right > m.right + 1.5 || ink.top < m.top - 1.5 || ink.bottom > m.bottom + 1.5) say('content-outside-frame', content, `${round(ink)} outside ${round(m)}`);
      if (Math.abs((ink.left - m.left) - (m.right - ink.right)) > 1.5) say('content-off-center', content, `${Math.round(ink.left - m.left)} px left, ${Math.round(m.right - ink.right)} px right`);
      const image = [...content.querySelectorAll('img')].find((img) => img.complete && img.naturalWidth && img.naturalHeight && !img.style.transform);
      if (image && !card.querySelector('.in-frame') && Math.abs(ink.height - ink.width * image.naturalHeight / image.naturalWidth) > 1.5) say('content-aspect', content, `${Math.round(ink.width)}×${Math.round(ink.height)} from ${image.naturalWidth}×${image.naturalHeight}`);
    }
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
    // What's under it, one block with its painted frame and its takes: from the frame's left edge to the takes' right, a gap under the frame
    // and before the takes, and on neither.
    const under = node.querySelector('.here-under');
    if (under && visible(under)) {
      const u = R(under);
      const side = node.querySelector('.here-side:not(:empty)');
      const t = side && visible(side) ? R(side) : null;
      const gap = parseFloat(getComputedStyle(under).getPropertyValue('--here-gap')) * scaleOf(under);
      if (Math.abs(u.left - m.left) > 1.5) say('under-left', under, `${Math.round(u.left - m.left)} px from its frame's left edge`);
      if (Math.abs(u.right - (t ?? m).right) > 1.5) say('under-right', under, `${Math.round(u.right - (t ?? m).right)} px from ${t ? "its takes'" : "its frame's"} right edge`);
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
      const on = a.flatMap((ea) => b.filter((eb) => meet(R(ea), R(eb))).map((eb) => [ea, eb]))[0];
      if (on) say('boxes-overlap', resting[i], `with <${name(resting[j])}>: ${where(on[0])} ${round(R(on[0]))} on ${where(on[1])} ${round(R(on[1]))}`);
    }
  }
  // What floats over the canvas, on nothing else that floats.
  const floats = ['.top .history', '.top .ptabs', '.top .mtabs', '.top .canvas-controls', '.top .main-menu > summary', '.top .pill', '.tools', '.zoom', '.notes'].flatMap((s) => [...document.querySelectorAll(s)]).filter(visible);
  for (let i = 0; i < floats.length; i++) for (let j = i + 1; j < floats.length; j++) if (meet(R(floats[i]), R(floats[j]))) say('chrome-overlap', floats[i], `on ${where(floats[j])}`);
  // A box being worked on, as the canvas fits it, isn't under what floats: zoomed in past that by hand, it's as near as was asked. In Focus its
  // picture is in Focus's frame and its name in Focus's heading (cl-893).
  for (const el of fitted ? document.querySelectorAll('.card.here > .media, .card.here > .tl, .card.here > .head, .focus-image-transform > .media, .focus-image-transform > .tl, .focus-heading, .here-under, .here-side') : []) {
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
  // Focus covers the canvas, and a press on its background leaves it (cl-893): no point of it is the canvas's.
  if (document.querySelector('[data-focused-asset]')) return [];
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

/** Same SVG, two fixed outline colors; all other paths, alpha and rendering context are retained. */
async function frameInkReferences(page, i) {
  return page.evaluate(async (i) => {
    const el = window.drawnFrames[i];
    if (!el?.isConnected) throw new Error('outline reference target is detached');
    const source = getComputedStyle(el).borderImageSource;
    const match = /^url\(["']?(.*?)["']?\)$/.exec(source);
    if (!match) throw new Error(`outline reference needs one SVG source: ${source}`);
    const response = await fetch(match[1]);
    if (!response.ok) throw new Error(`outline reference SVG refused: ${response.status}`);
    const text = await response.text();
    const svg = new DOMParser().parseFromString(text, 'image/svg+xml');
    const root = svg.documentElement;
    const paths = [...root.children];
    if (svg.querySelector('parsererror') || root.localName !== 'svg' || paths.length !== 2 || paths.some((path) => path.localName !== 'path' || path.children.length) || paths[1].getAttribute('fill-rule') !== 'evenodd' || paths.some((path) => !/^#[0-9a-f]{6}$/i.test(path.getAttribute('fill') ?? ''))) throw new Error('unsupported outline reference SVG schema');
    const allowedRoot = new Set(['xmlns', 'width', 'height', 'viewBox']);
    const allowedPath = new Set(['d', 'fill', 'fill-rule', 'opacity', 'fill-opacity']);
    if ([...root.attributes].some((a) => !allowedRoot.has(a.name)) || paths.some((path) => [...path.attributes].some((a) => !allowedPath.has(a.name)))) throw new Error('unsupported outline reference SVG attributes');
    const signature = (doc) => {
      const root = doc.documentElement;
      return JSON.stringify({ root: [...root.attributes].map((a) => [a.name, a.value]), paths: [...root.children].map((path, at) => [...path.attributes].filter((a) => !(at === 1 && a.name === 'fill')).map((a) => [a.name, a.value])) });
    };
    const identity = signature(svg);
    const saved = Object.fromEntries(['border-image-source', 'visibility'].map((name) => [name, [el.style.getPropertyValue(name), el.style.getPropertyPriority(name)]]));
    const urls = [];
    const xml = {};
    try {
      for (const [name, color] of [['black', '#000000'], ['white', '#ffffff']]) {
        const copy = svg.cloneNode(true);
        copy.documentElement.children[1].setAttribute('fill', color);
        if (signature(copy) !== identity) throw new Error('outline reference changed SVG geometry or alpha');
        xml[name] = new XMLSerializer().serializeToString(copy);
        const url = URL.createObjectURL(new Blob([xml[name]], { type: 'image/svg+xml' }));
        urls.push(url);
        const image = new Image();
        image.src = url;
        await image.decode();
      }
      return { source, saved, urls, black: urls[0], white: urls[1], identity, original: text, xml };
    } catch (error) {
      for (const url of urls) URL.revokeObjectURL(url);
      throw error;
    }
  }, i);
}

async function frameInkPaint(page, i, reference, source) {
  return page.evaluate(async ({ i, reference, source }) => {
    const el = window.drawnFrames[i];
    if (!el?.isConnected) throw new Error('outline reference target moved away');
    if (source === 'original') {
      const saved = reference.saved['border-image-source'];
      if (saved[0]) el.style.setProperty('border-image-source', saved[0], saved[1]);
      else el.style.removeProperty('border-image-source');
    } else el.style.setProperty('border-image-source', `url("${reference[source]}")`, 'important');
    await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    const actual = getComputedStyle(el).borderImageSource;
    if (source === 'original' ? actual !== reference.source : !actual.includes(reference[source])) throw new Error('outline reference source did not paint');
  }, { i, reference, source });
}

async function frameInkRelease(page, i, reference) {
  return page.evaluate(async ({ i, reference }) => {
    const el = window.drawnFrames[i];
    try {
      if (el) for (const [name, saved] of Object.entries(reference.saved)) {
        if (saved[0]) el.style.setProperty(name, saved[0], saved[1]);
        else el.style.removeProperty(name);
      }
      if (!el) throw new Error('outline reference target missing at restoration');
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      const current = Object.fromEntries(Object.keys(reference.saved).map((name) => [name, [el.style.getPropertyValue(name), el.style.getPropertyPriority(name)]]));
      if (JSON.stringify(current) !== JSON.stringify(reference.saved) || getComputedStyle(el).borderImageSource !== reference.source) throw new Error('outline reference original style did not restore');
      return current;
    } finally { for (const url of reference.urls) URL.revokeObjectURL(url); }
  }, { i, reference });
}

/** Hold each visible frame still for four pictures: actual, black outline, white outline, and bare. */
async function drawnEdges(page, decoder, label) {
  const said = [];
  const proofs = [];
  try {
    // Settle before reading a rectangle: finishing an animation can move the frame itself.
    const ids = await page.evaluate(() => {
      for (const held of document.getAnimations()) if (held.timeline === document.timeline && held.effect?.getComputedTiming().iterations !== Infinity) held.finish();
      window.heldStill = document.getAnimations().filter((held) => held.playState === 'running' && held.timeline === document.timeline);
      for (const held of window.heldStill) held.pause();
      const all = [...document.querySelectorAll('body *')].filter((el) => getComputedStyle(el).borderImageSource !== 'none' && el.checkVisibility({ opacityProperty: true, visibilityProperty: true }));
      const ids = [];
      window.drawnFrames = [];
      window.drawnEdgeBefore = [];
      for (const el of all) {
        if (all.some((other) => other !== el && other.contains(el))) continue;
        window.drawnEdgeBefore.push({ marker: el.getAttribute('data-drawn-edge'), hadStyle: el.hasAttribute('style') });
        el.dataset.drawnEdge = String(ids.length);
        window.drawnFrames.push(el);
        ids.push(ids.length);
      }
      return ids;
    });
    // Each capture has its own current frame. The viewport may refit while earlier frames are read.
    const frameAt = (i) => page.evaluate((i) => {
      const el = window.drawnFrames[i];
      if (!el?.isConnected) return null;
      const r = el.getBoundingClientRect();
      const style = getComputedStyle(el);
      const scale = r.width / el.offsetWidth;
      const o = parseFloat(style.borderImageOutset) * scale;
      const samples = [];
      for (const side of ['top', 'right', 'bottom', 'left']) {
        for (const t of [0.3, 0.5, 0.7]) {
          const x = side === 'left' ? r.left + scale : side === 'right' ? r.right - scale : r.left + t * r.width;
          const y = side === 'top' ? r.top + scale : side === 'bottom' ? r.bottom - scale : r.top + t * r.height;
          const top = document.elementFromPoint(x, y);
          if (top && el.contains(top)) samples.push({ side, x, y });
        }
      }
      const clipping = [];
      for (let at = el; at; at = at.parentElement) {
        const css = getComputedStyle(at);
        clipping.push([css.overflowX, css.overflowY, css.clipPath, css.maskImage, css.borderRadius, css.opacity, css.filter, css.transform, at === el ? '' : css.visibility]);
      }
      return {
        at: `${el.tagName.toLowerCase()}${[...el.classList].slice(0, 3).map((c) => `.${c}`).join('')}`,
        rect: [r.left, r.top, r.right, r.bottom], o, scale, samples,
        view: [innerWidth, innerHeight, devicePixelRatio, document.querySelector('.react-flow__viewport')?.style.transform ?? ''],
        image: style.borderImageSource,
        slices: [style.borderImageSlice, style.borderImageWidth, style.borderImageRepeat, style.borderImageOutset],
        clipping,
        eligible: !(r.width < 40 || r.height < 30 || r.left - o < 1 || r.top - o < 1 || r.right + o > innerWidth - 1 || r.bottom + o > innerHeight - 1),
      };
    }, i);
    const geometry = (frame) => frame && JSON.stringify([frame.rect, frame.scale, frame.view, frame.slices, frame.clipping]);
    const paint = (frame) => frame && JSON.stringify([frame.o, frame.image]);
    for (const i of ids) {
      const first = await frameAt(i);
      if (first && !first.eligible) continue; // The same size/on-screen exclusions as the pixel audit.
      let last;
      const what = `drawn edge ${i} stays in place through its four pictures`;
      let pair;
      try {
        await until(what, async () => {
          const frame = await frameAt(i);
          if (!frame?.eligible) { last = { frame, stable: false }; return false; }
          const [left, top, right, bottom] = frame.rect;
          const pad = frame.o + 3;
          const [width, height] = frame.view;
          const [x, y] = [Math.max(0, Math.floor(left - pad)), Math.max(0, Math.floor(top - pad))];
          const clip = { x, y, width: Math.min(width, Math.ceil(right + pad)) - x, height: Math.min(height, Math.ceil(bottom + pad)) - y };
          const reference = await frameInkReferences(page, i);
          let shown, black, white, bare, afterShown, afterBlack, afterWhite, afterBare, release;
          try {
            shown = await page.screenshot({ clip });
            afterShown = await frameAt(i);
            await frameInkPaint(page, i, reference, 'black');
            black = await page.screenshot({ clip });
            afterBlack = await frameAt(i);
            await frameInkPaint(page, i, reference, 'white');
            white = await page.screenshot({ clip });
            afterWhite = await frameAt(i);
            await frameInkPaint(page, i, reference, 'original');
            await page.evaluate((i) => window.drawnFrames[i].style.setProperty('visibility', 'hidden', 'important'), i);
            bare = await page.screenshot({ clip });
            afterBare = await frameAt(i);
          } finally { release = await frameInkRelease(page, i, reference); }
          const restored = await frameAt(i);
          const stable = reference.source === frame.image && afterBlack?.image.includes(reference.black) && afterWhite?.image.includes(reference.white) && [afterShown, afterBlack, afterWhite, afterBare, restored].every((after) => geometry(after) === geometry(frame))
            && [afterShown, afterBare, restored].every((after) => paint(after) === paint(frame));
          last = { frame, afterShown, afterBlack, afterWhite, afterBare, restored, reference, release, clip, viewport: { width, height }, shown, black, white, bare, stable };
          return stable;
        }, (stable) => stable, STEP_MS);
        pair = last;
      } catch (error) {
        if (!last || !error.message.startsWith(`${what}: still `)) throw error;
        const found = [{ rule: 'capture-moved', what: `${what}: geometry kept changing during capture` }];
        for (const finding of found) said.push({ at: first?.at ?? `drawn edge ${i}`, ...finding });
        if (process.env.SHOT) proofs.push({ i, ...last, found });
        continue;
      }
      const { frame, clip, shown, black, white, bare } = pair;
      const found = await decoder.evaluate(edgesOf, { shown: shown.toString('base64'), black: black.toString('base64'), white: white.toString('base64'), bare: bare.toString('base64'), clip, frame });
      for (const { rule, what } of found) said.push({ rule, at: frame.at, what });
      if (found.length && process.env.SHOT) proofs.push({ i, ...pair, found });
    }
  } finally {
    await page.evaluate(() => {
      for (const [i, el] of (window.drawnFrames ?? []).entries()) {
        const { marker, hadStyle } = window.drawnEdgeBefore[i];
        if (marker === null) delete el.dataset.drawnEdge;
        else el.setAttribute('data-drawn-edge', marker);
        // Read the attribute itself: CSSOM may not have serialized the empty style yet.
        if (!hadStyle && el.getAttribute('style') === '') el.removeAttribute('style');
      }
      for (const held of window.heldStill ?? []) held.play();
      delete window.drawnFrames;
      delete window.drawnEdgeBefore;
      delete window.heldStill;
    });
  }
  // Keep the exact comparison inputs after this pass has finished measuring, without another screenshot or DOM read.
  for (const { i, frame, clip, viewport, shown, black, white, bare, afterShown, afterBlack, afterWhite, afterBare, restored, reference, release, found } of proofs) {
    const path = `${process.env.SHOT}-${label}-edge-${i}`;
    if (shown) writeFileSync(`${path}-drawn.png`, shown);
    if (bare) writeFileSync(`${path}-bare.png`, bare);
    if (black) writeFileSync(`${path}-black.png`, black);
    if (white) writeFileSync(`${path}-white.png`, white);
    writeFileSync(`${path}-frame.json`, JSON.stringify({ label, frame, clip, viewport, afterShown, afterBlack, afterWhite, afterBare, restored, reference, release, findings: found }, null, 2));
  }
  return said;
}

/** Where a drag pans the canvas: a point where nothing's drawn on it, and the drag's end too, inside the viewport. */
function panPoint() {
  const bare = (x, y) => x >= 80 && y >= 60 && x <= innerWidth - 20 && y <= innerHeight - 20 && document.elementFromPoint(x, y)?.classList.contains('react-flow__pane');
  for (let y = 100; y <= innerHeight - 20; y += 20) for (let x = 140; x <= innerWidth - 20; x += 20) if (bare(x, y) && bare(x - 60, y - 40)) return { x, y };
  return null;
}

/** Compare the outline references near each edge, and the original with the bare capture outside it. Runs in the decoder's page. */
async function edgesOf({ shown, bare, black, white, clip, frame }) {
  const read = async (base64) => {
    const bitmap = await createImageBitmap(await (await fetch(`data:image/png;base64,${base64}`)).blob());
    const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
    const context = canvas.getContext('2d');
    context.drawImage(bitmap, 0, 0);
    try { return context.getImageData(0, 0, bitmap.width, bitmap.height); }
    finally { bitmap.close(); }
  };
  const [a, b, inkBlack, inkWhite] = await Promise.all([read(shown), read(bare), read(black), read(white)]);
  if ([b, inkBlack, inkWhite].some((image) => image.width !== a.width || image.height !== a.height)) throw new Error('outline comparison dimensions differ');
  const k = a.width / clip.width;
  const differs = (one, two, x, y) => {
    const [px, py] = [Math.round((x - clip.x) * k), Math.round((y - clip.y) * k)];
    if (px < 0 || py < 0 || px >= a.width || py >= a.height) return 0;
    const i = (py * a.width + px) * 4;
    return Math.max(Math.abs(one.data[i] - two.data[i]), Math.abs(one.data[i + 1] - two.data[i + 1]), Math.abs(one.data[i + 2] - two.data[i + 2]));
  };
  const [left, top, right, bottom] = frame.rect;
  const sides = {};
  for (const { side, x, y } of frame.samples) {
    const [dx, dy] = { top: [0, -1], right: [1, 0], bottom: [0, 1], left: [-1, 0] }[side];
    const [ex, ey] = { top: [x, top], right: [right, y], bottom: [x, bottom], left: [left, y] }[side];
    const most = (one, two, from, to) => {
      let held = 0;
      for (let d = from; d <= to; d += 0.5 / k) held = Math.max(held, differs(one, two, ex + dx * d, ey + dy * d));
      return held;
    };
    const tally = (sides[side] ??= { at: 0, line: 0, past: 0 });
    tally.at++;
    // The line wobbles from 3 px inside its edge to 2 px out; past it to the edge of its picture, nothing.
    if (most(inkBlack, inkWhite, -4 * frame.scale, 2.5 * frame.scale) > 80) tally.line++;
    if (frame.o - 0.5 > 3.5 * frame.scale && most(a, b, 3.5 * frame.scale, frame.o - 0.5) > 12) tally.past++;
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
    await signIn(page);
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
  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-layout-${Date.now()}`);
  await page.getByLabel("Create without encryption", { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  id = (await projectFromPage(page)).id;
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
    importFile(folder, join(scratch, file), asset);
  }
  const local = (await api('/v1/status')).local_usable && !process.env.ALONE;
  seen.ran = local;
  // A tool Calcifer hasn't read yet is given the kind it makes. `kind` and `at` below `fields` are a field of that name: odd's own, and cozy/frame's
  // time in its clip.
  addBoxes(folder, [
    { name: 'style', fields: { text: 'Bright, hand-painted, cel-shaded, with soft light and long shadows across every surface, and the sea always in the distance.' } },
    { name: 'still', tool: 'cozy/frame', kind: 'image', fields: { video: '<clip>', at: 0.5 } },
    { name: 'edit', tool: 'cozy/edit', fields: { tracks: [{ clips: ['<clip>', '<clip_b>'] }] } },
    { name: 'odd', kind: 'image', fields: { kind: 'widget' } },
    ...(local ? [
      { name: 'card', tool: `${drawPy}/draw`, kind: 'image', fields: { instructions: "<style> <hero_art> stands on the shore, with <clip> playing behind and a long line of words that runs past the frame's edge." } },
      // Its add-ons are hand-written, for a tool that loads no model: made with no tool, which nothing checks them against, and given it after.
      { name: 'posed', kind: 'image', fields: { instructions: '<hero_art>, posed.', lora: [{ ref: 'hf://someone/wushu-style-lora', strength: 0.8 }, { ref: 'hf://someone/ink-wash-with-a-long-name', strength: 0.6 }] } },
      { name: 'shot', tool: `${shotsPy}/shoot`, kind: 'video', fields: { summary: '<hero_art> looks hard at <clip>.', beats: [{ duration: 4, description: '<wide_art> waits on the shore.' }, { duration: 3, description: 'It goes on, and on.' }] } },
      { name: 'broken', tool: `${shotsPy}/crash`, kind: 'video', fields: { plate: '<clip>', crash: "the fixture's own failure, said in full so it wraps" } },
      { name: 'making', tool: `${shotsPy}/scenes`, kind: 'video', fields: { plate: '<clip>', summary: '<hero_art> walks the shore, segment after segment, as the tide comes in and the light goes long across the sand.', segments: 600, each: 5 } },
    ] : []),
  ]);
  if (local) projectJson(folder, 'PATCH', '/assets/posed', { tool: `${drawPy}/draw` });
  names = ['hero_art', 'wide_art', 'tall_art', LONG, 'sea_sound', 'clip', 'clip_b', 'style', 'still', 'edit', 'odd', ...(local ? ['card', 'posed', 'shot', 'broken', 'making'] : [])];
  await placed();
  const render = async (asset, wait = true) => {
    try { return await projectAction(folder, 'render', asset, '--on', 'local', ...(wait ? [] : ['--queue'])); }
    catch (error) { if (asset !== 'broken') throw error; }
  };
  // Takes: the frame thrice, the Edit once, and where this computer runs, many of the picture tool's, the shot's two and a failure.
  for (const at of [0.2, 0.5, 0.8]) {
    projectJson(folder, 'PATCH', '/assets/still', { fields: { at } });
    await render('still');
  }
  await render('edit');
  const box = (asset) => boxOf(page, asset);
  if (local) {
    // Its first render waits on its install, which the box says.
    await render('card', false);
    const waiting = await until('the picture tool waits', () => box('card').locator('.prog b, .take.making').count(), (n) => n > 0, RUN_MS).catch(() => 0);
    seen.waits = waiting > 0;
    if (waiting) {
      const said = await page.evaluate(audit, { floor: FLOOR });
      for (const finding of said) findings.push({ size: '1600x1000', state: 'waiting-on-install', ...finding });
      if (process.env.SHOT && said.length) await page.screenshot({ path: `${process.env.SHOT}-1600x1000-waiting-on-install.png`, scale: 'css' });
    }
    await until('the first take', () => api(`/v1/projects/${id}`), (state) => state.assets.find((held) => held.name === 'card')?.prefixes?.length >= 1, RUN_MS);
    for (let i = 0; i < 8; i++) await render('card');
    await render('shot');
    await render('shot');
    await render('broken');
  }
  await until('every take landed', () => api(`/v1/projects/${id}`), (state) => !state.assets.some((held) => held.standing === 'making'), RUN_MS);
  // One being made all through the measuring, saying how far it's got, its steps a second each.
  if (local) {
    await render('making', false);
    stopMaking = () => projectJson(folder, 'POST', '/cancel', { names: ['making'] });
  }
  }
  await first.close();

  // Each size: the canvas at rest at each zoom, each box in each state, and what's around the canvas.
  // $SIZES: some of them, by name, for a quick look.
  for (const size of selectedSizes) {
    const context = await browser.newContext({ viewport: size.viewport, deviceScaleFactor: size.deviceScaleFactor });
    page = await context.newPage();
    watch(page);
    lookAt(page);
    await signedIn(page);
    await page.goto(await projectUrl(id));
    await page.locator('.react-flow__node').first().waitFor();
    await placed();
    const look = async (state, keep, fitted = true, beforeAudit) => {
      // What opens animates in: measured once it's still. A pulse goes on forever, and is no move.
      await page.waitForFunction(() => !document.getAnimations().some((held) => held.playState === 'running' && held.timeline === document.timeline && held.effect?.getComputedTiming().iterations !== Infinity), null, { timeout: 5000 }).catch(() => {});
      if (beforeAudit) await beforeAudit();
      seen.looks = (seen.looks ?? 0) + 1;
      const said = await page.evaluate(audit, { floor: FLOOR, fitted });
      // The pointer where nothing's drawn, around a box worked on or all over.
      if (state === 'rest' || /^(open|writing|zoom|fit)-/.test(state)) {
        const block = await page.evaluate(() => {
          const rects = [...document.querySelectorAll('.card.here, .card.here > .here-side, .card.here > .here-under')].map((el) => el.getBoundingClientRect()).filter((r) => r.width);
          return rects.length ? [Math.min(...rects.map((r) => r.left)) - 60, Math.min(...rects.map((r) => r.top)) - 60, Math.max(...rects.map((r) => r.right)) + 60, Math.max(...rects.map((r) => r.bottom)) + 60] : null;
        });
        said.push(...(await page.evaluate(emptyHits, { around: block, step: block ? 10 : 24 })));
      }
      for (const finding of said) findings.push({ size: size.name, state, ...finding });
      // How large an open box's words are on screen as the canvas draws in on it, for the record: a zoom makes them larger or smaller.
      if (/^(open|writing)-/.test(state)) {
        const least = await page.evaluate(() => {
          const zoom = new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform).a;
          const sizes = [...document.querySelectorAll('.card.here, .card.here *')]
            .filter((el) => [...el.childNodes].some((n) => n.nodeType === 3 && n.textContent.trim()) && el.checkVisibility({ opacityProperty: true, visibilityProperty: true }) && el.offsetHeight)
            .map((el) => (parseFloat(getComputedStyle(el).fontSize) * el.getBoundingClientRect().height) / el.offsetHeight);
          return sizes.length ? { zoom: Math.round(zoom * 1000) / 1000, least_px: Math.round(Math.min(...sizes) * 10) / 10 } : null;
        });
        if (least) (seen.open_on_screen ??= []).push({ size: size.name, state, ...least });
      }
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
        await settled(page);
      }
      await until(`the camera reaches ${want}`, () => page.evaluate(() => new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform).a), (actual) => actual === want);
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
      (seen.rest_close ??= []).push({ size: size.name, ...(await restClose(page)) });
      await look('rest-close', true);
    }
    // At rest, being made, at 75%: what it says of how far it's got.
    const making = page.locator('.react-flow__node .card[data-asset="making"] > .media').first();
    await home();
    if (names.includes('making')) {
      const makingState = () => page.evaluate(() => {
        const media = document.querySelector('.card[data-asset="making"] > .media');
        const rect = media?.getBoundingClientRect();
        const left = document.querySelector('.tools')?.getBoundingClientRect().right ?? 0;
        const top = document.querySelector('.top')?.getBoundingClientRect().bottom ?? 0;
        const viewport = document.querySelector('.react-flow__viewport');
        const transform = viewport ? getComputedStyle(viewport).transform : null;
        // Its render's lines, under the picture at rest (cl-932): the machine and its time, then what it's doing.
        const progress = media?.closest('.card')?.querySelector(':scope > .tile-under .prog .face-state b');
        const visibility = { opacityProperty: true, visibilityProperty: true };
        const rendered = Boolean(media?.checkVisibility?.(visibility));
        const inBounds = Boolean(rect && rect.width && rect.height && rect.left >= left && rect.top >= top && rect.right <= innerWidth && rect.bottom <= innerHeight);
        const progressVisible = Boolean(progress?.checkVisibility?.(visibility));
        // Its state line (cl-898): `Segment 3 of 600: Denoising - Step 2 / 5`.
        const text = progress?.innerText ?? null;
        let reason = null;
        if (!media) reason = 'missing-media';
        else if (!rendered) reason = 'hidden-media';
        else if (!inBounds) reason = 'offscreen-media';
        else if (!progress) reason = 'missing-progress-label';
        else if (!progressVisible) reason = 'hidden-progress-label';
        else if (!text) reason = 'empty-progress-text';
        return {
          hash: location.pathname, open: [...document.querySelectorAll('.card.here')].map((card) => card.dataset.asset),
          selected: [...document.querySelectorAll('.react-flow__node.selected')].map((node) => node.querySelector('.card')?.dataset.asset ?? node.dataset.id),
          zoom: transform === null ? null : new DOMMatrix(transform).a, transform, mounted: Boolean(media),
          rect: rect && { x: rect.x, y: rect.y, width: rect.width, height: rect.height }, rendered, inBounds, visible: rendered && inBounds,
          progress: Boolean(progress), progressVisible, text, reason,
        };
      });
      const restingMaking = (at) => at.zoom === 0.75 && at.mounted && at.visible && !at.open.length && !at.selected.length;
      const makingProgress = (at) => restingMaking(at) && at.progressVisible && /\bStep \d+ \/ \d+/.test(at.text ?? '');
      await making.waitFor().catch(async (error) => {
        const state = await makingState();
        (seen.making_setup ??= []).push({ size: size.name, phase: 'mount', ...state });
        throw new Error(`making media did not mount visibly: ${JSON.stringify(state)}`, { cause: error });
      });
      // Shift adds to a selection: rest-close leaves its box selected, and Shift+2 would fit both.
      await page.keyboard.press('Escape');
      await page.waitForFunction(() => !document.querySelector('.card.here') && !document.querySelector('.react-flow__node.selected'));
      const at = await making.boundingBox();
      await page.keyboard.down('Shift');
      await page.mouse.click(at.x + at.width / 2, at.y + at.height / 2);
      await page.keyboard.up('Shift');
      await page.waitForFunction(() => {
        const selected = [...document.querySelectorAll('.react-flow__node.selected')];
        return selected.length === 1 && selected[0].querySelector('.card')?.dataset.asset === 'making';
      });
      (seen.making_selected ??= []).push({ size: size.name, selected: await page.evaluate(() => [...document.querySelectorAll('.react-flow__node.selected')].map((node) => ({ id: node.getAttribute('data-id'), name: node.querySelector('.card')?.dataset.asset }))) });
      await page.keyboard.press('Shift+2');
      await settled(page);
      await page.keyboard.press('Escape');
      await page.waitForFunction(() => !document.querySelector('.card.here') && !document.querySelector('.react-flow__node.selected'));
      await zoomTo(0.75);
      const setup = await makingState();
      (seen.making_setup ??= []).push({ size: size.name, ...setup });
      if (!restingMaking(setup)) throw new Error(`making setup is not a visible resting 75% canvas: ${JSON.stringify(setup)}`);
      const progress = await until('it says how far', makingState, makingProgress).catch(async () => {
        const state = await makingState();
        (seen.making_progress_failure ??= []).push({ size: size.name, ...state });
        return null;
      });
      const said = progress?.text ?? '';
      check(`making_says_how_far_${size.name}`, /\bStep \d+ \/ \d+/.test(said), said);
      await look('rest-making', true, true, async () => {
        const state = await makingState();
        (seen.making_audit ??= []).push({ size: size.name, ...state });
        if (!restingMaking(state)) throw new Error(`making is not visible on the resting 75% canvas before audit: ${JSON.stringify(state)}`);
      });
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
      // Its warnings, where the project has any for it (graph.ts warningsOf): in Focus, the heading's Warnings button opens them.
      const warned = page.locator(`[data-focused-asset="${asset}"]`).getByRole('button', { name: 'Warnings', exact: true });
      const warns = ((await api(`/v1/projects/${id}`)).warnings ?? []).some((line) => line.name === asset || line.name?.startsWith(`${asset}.`));
      if (warns) {
        check(`open_${asset}_offers_its_warnings_${size.name}`, (await warned.count()) === 1, await page.locator(`[data-focused-asset="${asset}"] .focus-heading`).innerText().catch(() => null));
        if (await warned.count()) {
          await warned.click();
          await settled(page);
          await look(`open-${asset}-warnings`, true);
        }
      }
      const next = here.locator('.take.add');
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
          // Audited only once it's there: a names list that never opens is a failure, not a look at the box without it.
          const listed = await page.getByRole('listbox').first().waitFor({ timeout: 5000 }).then(() => true, () => false);
          check(`names_list_opens_${size.name}`, listed);
          if (listed) await look('names-list', true);
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
    // Zoomed with a box open, by the wheel, its zoom control and the keys, is its picture zoomed in Focus (cl-893: the canvas's zoom is Wander's
    // and isn't there). What's around the picture stays one block where it was: Focus's heading, the frame, its takes and what's under it, each
    // its own, with none of the picture drawn on it. Then a drag on the picture, enlarged, pans it. The canvas behind stays where it was.
    for (const asset of [many, 'wide_art', 'tall_art']) {
      await openBox(page, asset);
      await settled(page);
      const focus = page.locator(`[data-focused-asset="${asset}"]`);
      const camera = () => focus.locator('.focus-image-transform').evaluate((el) => {
        const m = new DOMMatrix(getComputedStyle(el).transform);
        return { zoom: m.a, x: m.e, y: m.f };
      });
      const canvas = () => page.evaluate(() => document.querySelector('.react-flow__viewport')?.style.transform);
      const around = () => focus.evaluate((held) => [...held.querySelectorAll('.focus-heading, .focus-viewer, .here-side, .here-under, .image-controls')]
        .filter((el) => el.checkVisibility())
        .map((el) => {
          const r = el.getBoundingClientRect();
          const top = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
          return { part: el.classList[0], rect: [r.left, r.top, r.right, r.bottom], covered: Boolean(top?.closest('.focus-image-transform')) && !el.contains(top) };
        }));
      const behind = await canvas();
      const before = await around();
      const middle = async () => {
        const at = await focus.locator('.focus-viewer').boundingBox();
        return { x: at.x + at.width / 2, y: at.y + at.height / 2 };
      };
      const wheel = async (dy) => {
        const { x, y } = await middle();
        await page.mouse.move(x, y);
        await page.keyboard.down('Control');
        for (let i = 0; i < 3; i++) await page.mouse.wheel(0, dy);
        await page.keyboard.up('Control');
      };
      const still = (state, what) => {
        if (what) findings.push({ size: size.name, state, rule: 'zoom-canvas', at: `<${asset}>`, what });
      };
      for (const [step, act] of [
        ['wheel-out', () => wheel(120)],
        ['wheel-in', () => wheel(-160)],
        ['control-out', () => focus.getByRole('button', { name: 'Zoom out image', exact: true }).click()],
        ['control-in', () => focus.getByRole('button', { name: 'Zoom in image', exact: true }).click()],
        ['key-out', () => page.keyboard.press('Control+Minus')],
        ['key-in', () => page.keyboard.press('Control+Equal')],
      ]) {
        const state = `zoom-${asset}-${step}`;
        const was = (await camera()).zoom;
        await act();
        await settled(page);
        const now = (await camera()).zoom;
        (seen.focus_zooms ??= []).push({ size: size.name, asset, step, was, now });
        // Out from fitted it stays fitted: in Focus a picture is never smaller than its frame.
        if (step.endsWith('-in') ? !(now > was) : !(now < was || (was === 1 && now === 1))) findings.push({ size: size.name, state, rule: 'zoom-unmoved', at: `<${asset}>'s picture`, what: `${step} took its zoom from ${was} to ${now}` });
        for (const part of await around()) {
          const held = before.find((one) => one.part === part.part);
          if (!held || part.rect.some((v, i) => Math.abs(v - held.rect[i]) > 1)) findings.push({ size: size.name, state, rule: 'zoom-moved', at: `<${asset}> .${part.part}`, what: `${JSON.stringify(held?.rect.map(Math.round))} became ${JSON.stringify(part.rect.map(Math.round))}` });
          if (part.covered) findings.push({ size: size.name, state, rule: 'zoom-covers', at: `<${asset}> .${part.part}`, what: 'its zoomed picture is drawn on it' });
        }
        still(state, (await canvas()) !== behind && `zooming its picture moved the canvas behind Focus to ${await canvas()}`);
        await look(state, step.endsWith('out'), false);
      }
      const from = await camera();
      const { x, y } = await middle();
      await page.mouse.move(x, y);
      await page.mouse.down();
      try { await page.mouse.move(x - 60, y - 40, { steps: 6 }); }
      finally { await page.mouse.up(); }
      await settled(page);
      const to = await camera();
      (seen.focus_pans ??= []).push({ size: size.name, asset, from, to });
      if (Math.abs(to.x - from.x) < 1 && Math.abs(to.y - from.y) < 1) findings.push({ size: size.name, state: `zoom-${asset}`, rule: 'drag-no-pan', at: `<${asset}>'s picture`, what: `a drag from ${Math.round(x)},${Math.round(y)} left it where it was, at ${from.zoom}` });
      still(`zoom-${asset}`, (await canvas()) !== behind && `a drag on its picture moved the canvas behind Focus to ${await canvas()}`);
      // Zoom to fit, by its keys: the picture as it opened.
      await page.keyboard.press('Shift+Digit1');
      await settled(page);
      const fit = await camera();
      if (fit.zoom !== 1 || Math.abs(fit.x) > 0.5 || Math.abs(fit.y) > 0.5) findings.push({ size: size.name, state: `fit-${asset}`, rule: 'fit-unfitted', at: `<${asset}>'s picture`, what: `Zoom to fit left it at ${JSON.stringify(fit)}` });
      await look(`fit-${asset}`, true);
      await closeBox(page);
    }
    // At rest, a drag where nothing's drawn pans the canvas.
    await home();
    const from = await page.evaluate(panPoint);
    const viewport = () => page.evaluate(() => document.querySelector('.react-flow__viewport').style.transform);
    const before = await viewport();
    if (from) {
      await page.mouse.move(from.x, from.y);
      await page.mouse.down();
      try { await page.mouse.move(from.x - 60, from.y - 40, { steps: 6 }); }
      finally { await page.mouse.up(); }
      await settled(page);
    }
    const after = await viewport();
    (seen.pan_targets ??= []).push({ size: size.name, from, before, after });
    if (!from) findings.push({ size: size.name, state: 'rest', rule: 'pan-target', at: 'the canvas', what: 'no bare canvas in view to drag' });
    else if (after === before) findings.push({ size: size.name, state: 'rest', rule: 'drag-no-pan', at: 'the canvas', what: `a drag from ${from.x},${from.y} left the canvas where it was` });
    // What's around the canvas.
    await home();
    const chip = page.locator('.mtabs button, .mtabs a').first();
    if (await chip.count()) {
      await chip.click();
      const doing = await page.getByRole('dialog', { name: /is doing$/ }).first().waitFor({ timeout: 5000 }).then(() => true, () => false);
      check(`machine_activity_opens_${size.name}`, doing);
      if (doing) await look('machine-activity', true);
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
      const menu = await page.getByRole('menu').first().waitFor({ timeout: 5000 }).then(() => true, () => false);
      check(`asset_menu_opens_${size.name}`, menu);
      if (menu) await look('asset-menu', true);
      await page.keyboard.press('Escape');
    }
    for (const drawer of ['machines', 'runs', 'account', 'projects', 'packages']) {
      await page.goto(await projectUrl(id, `/${drawer}`));
      // Looked at once it's there; look() waits out what moves (it can't wait on an animation the GPU path holds at its start).
      await page.locator('.drawer').waitFor();
      await look(`drawer-${drawer}`, true);
    }
    await page.goto(await projectUrl(id));
    await context.close();
  }

  // As a person's Chrome draws it, on the GPU path: each drawn edge whole at rest, on a box open, its picture zoomed in and written in, and in a drawer; and on a
  // 1.25x screen, where every edge is drawn at a fractional scale.
  const drawn = await launch(playwright, GPU);
  const decoder = await drawn.newPage();
  for (const size of selectedDrawnSizes) {
    const context = await drawn.newContext({ viewport: size.viewport, deviceScaleFactor: size.deviceScaleFactor });
    page = await context.newPage();
    watch(page);
    lookAt(page);
    await signedIn(page);
    await page.goto(await projectUrl(id));
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
    // Zoomed with it open is its picture zoomed, in Focus (cl-893): the canvas's zoom isn't there.
    for (const step of ['in', 'in-again']) {
      await page.locator(`[data-focused-asset="${many}"]`).getByRole('button', { name: 'Zoom in image', exact: true }).click();
      await settled(page);
      await edges(`open-${many}-zoomed-${step}`);
    }
    await page.keyboard.press('Shift+Digit1');
    await settled(page);
    const next = page.locator(`.card.here[data-asset="${many}"] .take.add`);
    if (await next.count()) {
      await (await renderOf(page, many)).waitFor();
      await settled(page);
      await edges(`writing-${many}`);
    }
    await closeBox(page);
    for (const drawer of ['runs', 'packages']) {
      await page.goto(await projectUrl(id, `/${drawer}`));
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
