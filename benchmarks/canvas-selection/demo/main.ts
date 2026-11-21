// Interactive renderer A/B prototype: one Creator-like scene and controller, one active renderer.
import '../src/style.css';
import './demo.css';
import { createPerspective } from '../src/perspective';
import { createDemoUI } from './ui';
import type { DemoMode, DemoUIState } from './ui-types';
import type { Adapter, BenchOptions, Camera, Card, CreateAdapter } from '../src/types';

const params = new URLSearchParams(location.search);
const surface = document.querySelector<HTMLElement>('#surface')!;
document.body.classList.add('demo-mode');
document.title = 'Creator · Canvas comparison';
const grid = document.createElement('div'); grid.className = 'demo-grid';
Object.assign(grid.style, { inset: '-48px', width: 'calc(100% + 96px)', height: 'calc(100% + 96px)', willChange: 'transform' });
const scene = document.createElement('div'); scene.className = 'demo-scene';
Object.assign(scene.style, { position: 'absolute', inset: '0' });
const hoverLayer = document.createElement('div'); hoverLayer.className = 'demo-hover-layer';
Object.assign(hoverLayer.style, { position: 'absolute', left: '0', top: '0', pointerEvents: 'none', transformOrigin: '0 0', zIndex: '5' });
const marquee = document.createElement('div'); marquee.className = 'demo-marquee'; marquee.hidden = true;
Object.assign(marquee.style, { position: 'absolute', pointerEvents: 'none', border: '1px solid #ff9a5e', background: '#ff9a5e16', zIndex: '6' });
surface.append(grid, scene, hoverLayer, marquee);

let width = innerWidth, height = innerHeight;
let camera: Camera = { x: 330, y: 125, zoom: .85 };
let perspective = createPerspective(surface, width, height);
let mode: DemoMode = ['pixi', 'reactflow', 'react-dom'].includes(params.get('mode') || '') ? params.get('mode') as DemoMode : 'react-dom';
let baseCount = Math.max(1, Math.min(5000, Number(params.get('n') || 1000)));
let cards = new Map<string, Card>(), nextId = 0, selected = new Set<string>();
let adapter: Adapter | null = null, options: BenchOptions | null = null;
let switching = false, desiredMode: DemoMode | null = null, loaded = 0, uiFrame = 0, pollTimer = 0;
let space = false, tilted = false, statsOn = false, touring = false, tourFrame = 0;
let contextMenu: HTMLElement | null = null, focusPanel: HTMLElement | null = null, focused: string | null = null;
let viewer: Awaited<ReturnType<typeof import('../src/three-demo').createThreeDemo>> | null = null;
let viewerHost: HTMLElement | null = null, viewerOn = false;
let statsFrame = 0, fps = 0, p95 = 0, lastStats = 0, lastFrame = 0;
const intervals: number[] = [];
const prompts = new Map<string, string>(), jobs = new Map<string, number>();
const assets = new Map<number, { small: string; large: string; full?: string; video?: string }>();
assets.set(5000, { small: '/demo/video256.webp', large: '/demo/video512.webp', video: '/motion.mp4' });
let nextAsset = 5001;
const blobs: string[] = [];
const names = ['coast_morning', 'ridge_study', 'motion_take', 'forest_light', 'lake_ref', 'mountain_pass', 'quiet_shore', 'sunset_grade'];

// The same small spatial index is used for input in every renderer.
const buckets = new Map<string, Set<string>>();
const cellKeys = (card: Card) => { const keys: string[] = []; for (let x = Math.floor(card.x / 512); x <= Math.floor((card.x + card.width) / 512); x++) for (let y = Math.floor(card.y / 512); y <= Math.floor((card.y + card.height) / 512); y++) keys.push(`${x}:${y}`); return keys; };
function indexAdd(card: Card) { for (const key of cellKeys(card)) { if (!buckets.has(key)) buckets.set(key, new Set()); buckets.get(key)!.add(card.id); } }
function indexRemove(card: Card) { for (const key of cellKeys(card)) { const set = buckets.get(key); set?.delete(card.id); if (!set?.size) buckets.delete(key); } }
function makeCard(i: number, x: number, y: number, kind: 'image' | 'video' = 'image'): Card {
  const w = kind === 'video' ? 360 : 300;
  return { id: String(i), x, y, width: w, height: 32 + Math.round(w * 9 / 16) + 40, asset: kind === 'video' ? 5000 : i % 5000,
    label: `${names[i % names.length]}${i >= 8 ? `_${Math.floor(i / 8) + 1}` : ''}`, version: 1 + i % 3,
    tool: kind === 'video' ? 'MiniMax H3 · Video' : i % 3 ? 'Qwen Image 2.1 · Image' : 'Flux Krea · Image', kind,
    status: i % 29 === 1 ? 'Denoising Image · Step 12 / 30' : '', state: i % 29 === 1 ? 'rendering' : i % 11 === 0 ? 'ready' : undefined,
    progress: i % 29 === 1 ? .4 : undefined };
}
function seed(count: number) {
  for (const timer of jobs.values()) clearInterval(timer); jobs.clear();
  cards.clear(); selected.clear(); buckets.clear(); prompts.clear();
  const cols = Math.ceil(Math.sqrt(count * 1.4));
  for (let i = 0; i < count; i++) { const card = makeCard(i, i % cols * 410, Math.floor(i / cols) * 330, i % 17 === 2 ? 'video' : 'image'); cards.set(card.id, card); indexAdd(card); }
  nextId = count;
}
function assetUrl(asset: number, tier: number) { const own = assets.get(asset); return own ? new URL(own.video || tier > 256 ? own.large : own.small, location.href).href : `${location.origin}/cards/${((asset % 5000) + 5000) % 5000}/${tier}.webp`; }
function visible(card: Card) { return card.x * camera.zoom + camera.x < width + 64 && (card.x + card.width) * camera.zoom + camera.x > -64 && card.y * camera.zoom + camera.y < height + 64 && (card.y + card.height) * camera.zoom + camera.y > -64; }
function makeOptions(): BenchOptions { return { appearance: 'creator', cards: [...cards.values()], camera: { ...camera }, width, height,
  assetUrl, tier: cam => 300 * cam.zoom * devicePixelRatio <= 256 ? 256 : 512,
  visible: (card, cam) => card.x * cam.zoom + cam.x < width + 64 && (card.x + card.width) * cam.zoom + cam.x > -64 && card.y * cam.zoom + cam.y < height + 64 && (card.y + card.height) * cam.zoom + cam.y > -64,
  lod: cam => cam.zoom < .2 ? 2 : cam.zoom < .45 ? 1 : 0, onDraw: scheduleUI }; }
const factories: Record<DemoMode, () => Promise<CreateAdapter>> = {
  'react-dom': async () => (await import('../src/react-dom')).createReactDomAdapter,
  pixi: async () => (await import('../src/pixi')).createPixiAdapter,
  reactflow: async () => (await import('../src/reactflow')).createReactFlowAdapter,
};
function urlState() { const url = new URL(location.href); url.searchParams.set('demo', '1'); url.searchParams.set('mode', mode); url.searchParams.set('n', String(baseCount)); if (mode === 'reactflow') url.searchParams.set('rfLayer', '1'); history.replaceState(null, '', url); }
async function switchMode(next: DemoMode, force = false) {
  if (switching) { desiredMode = next; return; }
  if (adapter && mode === next && !force) return;
  stopHover(); closeMenu(); mode = next; switching = true; loaded = 0; urlState(); scheduleUI();
  clearTimeout(pollTimer); adapter?.destroy(); adapter = null;
  options = makeOptions();
  try { await document.fonts.ready; const create = await factories[next](); adapter = await create(scene, options); adapter.setCamera(camera); adapter.setSelection?.([...selected]); if (!adapter.setSelection) adapter.setSelected([...selected][0] ?? null); adapter.resize?.(width, height); }
  catch (error) { ui.toast(`Could not start ${next}: ${String(error)}`); console.error(error); }
  switching = false; scheduleUI(); pollReady();
  if (desiredMode) { const wanted = desiredMode; desiredMode = null; if (wanted !== mode) void switchMode(wanted); }
}
function pollReady() { clearTimeout(pollTimer); if (!adapter) return; loaded = adapter.readyFraction(); scheduleUI(); if (loaded < 1) pollTimer = window.setTimeout(pollReady, 100); }
function drawGrid() { const scale = camera.zoom / 2 ** Math.floor(Math.log2(camera.zoom)), step = 24 * scale; grid.style.transform = `translate(${((camera.x % step) + step) % step - 24}px,${((camera.y % step) + step) % step - 24}px) scale(${scale})`; hoverLayer.style.transform = `translate3d(${camera.x}px,${camera.y}px,0) scale(${camera.zoom})`; }
function setCamera(next: Camera, stop = true) { if (stop) stopHover(); camera = next; adapter?.setCamera(camera); drawGrid(); scheduleUI(); }
function home() { setCamera({ x: width > 900 ? 330 : 36, y: 125, zoom: .85 }); }
function fit() { if (!cards.size) return home(); const all = [...cards.values()], left = Math.min(...all.map(c => c.x)), top = Math.min(...all.map(c => c.y)), right = Math.max(...all.map(c => c.x + c.width)), bottom = Math.max(...all.map(c => c.y + c.height)); const inset = width > 900 ? 285 : 24; const z = Math.min(1.5, (width - inset - 35) / (right - left + 30), (height - 190) / (bottom - top + 30)); setCamera({ x: inset + (width - inset - 35 - (right - left) * z) / 2 - left * z, y: 90 + (height - 190 - (bottom - top) * z) / 2 - top * z, zoom: Math.max(.015, z) }); }
function localPoint(x: number, y: number) { return perspective.unproject(x, y); }
function worldPoint(x: number, y: number) { const p = localPoint(x, y); return { x: (p.x - camera.x) / camera.zoom, y: (p.y - camera.y) / camera.zoom }; }
function pick(x: number, y: number) { const p = worldPoint(x, y), ids = buckets.get(`${Math.floor(p.x / 512)}:${Math.floor(p.y / 512)}`); if (!ids) return null; const candidates = [...ids].reverse(); for (const id of candidates) { const c = cards.get(id)!; if (p.x >= c.x && p.x <= c.x + c.width && p.y >= c.y && p.y <= c.y + c.height) return c; } return null; }
function zoomAt(factor: number, x = width / 2 + (width > 900 ? 100 : 0), y = height / 2) { const p = localPoint(x, y), z = Math.max(.015, Math.min(4, camera.zoom * factor)), ratio = z / camera.zoom; setCamera({ x: p.x - (p.x - camera.x) * ratio, y: p.y - (p.y - camera.y) * ratio, zoom: z }); }
function select(ids: Iterable<string>) { selected = new Set([...ids].filter(id => cards.has(id))); adapter?.setSelection?.([...selected]); if (!adapter?.setSelection) adapter?.setSelected([...selected][0] ?? null); scheduleUI(); }
function patch(id: string, changes: Partial<Card>) { const c = cards.get(id); if (!c) return; const moved = changes.x !== undefined || changes.y !== undefined || changes.width !== undefined || changes.height !== undefined; if (moved) indexRemove(c); Object.assign(c, changes); if (moved) indexAdd(c); adapter?.updateCard?.(id, changes); scheduleUI(); }
function move(moves: { id: string; x: number; y: number }[]) { for (const p of moves) { const c = cards.get(p.id); if (!c) continue; indexRemove(c); c.x = p.x; c.y = p.y; indexAdd(c); } if (adapter?.moveCards) adapter.moveCards(moves); else for (const p of moves) adapter?.moveCard(p.id, p.x, p.y); scheduleUI(); }
const undo: { back(): void; forward(): void }[] = [], redo: typeof undo = [];
function record(back: () => void, forward: () => void) { undo.push({ back, forward }); if (undo.length > 60) undo.shift(); redo.length = 0; scheduleUI(); }
function undoOne() { const op = undo.pop(); if (op) { op.back(); redo.push(op); scheduleUI(); } }
function redoOne() { const op = redo.pop(); if (op) { op.forward(); undo.push(op); scheduleUI(); } }
function restore(list: Card[]) { for (const c of list) { const clone = { ...c }; cards.set(c.id, clone); indexAdd(clone); adapter?.addCard?.(clone); } scheduleUI(); }
function removeIds(ids: string[]) { const removed = ids.map(id => cards.get(id)).filter(Boolean) as Card[]; for (const c of removed) { clearInterval(jobs.get(c.id)); jobs.delete(c.id); indexRemove(c); cards.delete(c.id); selected.delete(c.id); } if (removed.length > 16) void switchMode(mode, true); else for (const c of removed) adapter?.removeCard?.(c.id); if (focused && ids.includes(focused)) closeFocus(); select(selected); scheduleUI(); return removed; }
function removeSelected() { if (!selected.size) return; const ids = [...selected], removed = removeIds(ids).map(c => ({ ...c })); record(() => restore(removed), () => removeIds(ids)); }
function duplicate() { if (!selected.size || switching) return; const original = cards.get([...selected][0])!; const c = { ...original, id: String(nextId++), label: `${original.label}_copy`, x: original.x + 55, y: original.y + 55 }; cards.set(c.id, c); indexAdd(c); adapter?.addCard?.(c); select([c.id]); record(() => removeIds([c.id]), () => restore([c])); }
function add(kind: 'image' | 'video', render = true) { if (switching) return; const p = worldPoint(width / 2 + (width > 900 ? 100 : 0), height / 2); const c = makeCard(nextId++, p.x - 150, p.y - 115, kind); c.label = kind === 'video' ? `video_take_${c.id}` : `new_image_${c.id}`; c.status = ''; c.state = undefined; c.progress = undefined; cards.set(c.id, c); indexAdd(c); adapter?.addCard?.(c); select([c.id]); record(() => removeIds([c.id]), () => restore([c])); if (render) simulate(c.id); else openFocus(c.id); scheduleUI(); }
function simulate(id: string) { const c = cards.get(id); if (!c) return; clearInterval(jobs.get(id)); let step = 0; patch(id, { state: 'rendering', progress: 0, status: 'Preparing local preview…' }); const timer = window.setInterval(() => { if (!cards.has(id)) { clearInterval(timer); return; } step++; patch(id, { progress: step / 20, status: `Denoising ${c.kind === 'video' ? 'Video' : 'Image'} · Step ${step} / 20` }); if (step >= 20) { clearInterval(timer); jobs.delete(id); patch(id, { state: 'ready', progress: undefined, status: '', version: (c.version || 1) + 1, asset: c.kind === 'video' ? 5000 : (c.asset + 37) % 5000 }); if (focused === id) updateFocusMedia(); ui.toast('Local sample ready · no model job was started'); } }, 110); jobs.set(id, timer); }
function closeMenu() { contextMenu?.remove(); contextMenu = null; }
function menu(event: MouseEvent) { event.preventDefault(); closeMenu(); const c = pick(event.clientX, event.clientY); if (!c) return; if (!selected.has(c.id)) select([c.id]); const el = document.createElement('div'); el.className = 'demo-context-menu'; Object.assign(el.style, { position: 'fixed', left: `${Math.min(event.clientX, width - 190)}px`, top: `${Math.min(event.clientY, height - 150)}px`, zIndex: '5000' }); for (const [label, action] of [['Open', () => openFocus(c.id)], ['Duplicate', duplicate], ['Delete', removeSelected]] as const) { const b = document.createElement('button'); b.textContent = label; b.addEventListener('click', () => { closeMenu(); action(); }); el.append(b); } document.body.append(el); contextMenu = el; }

let hoverTimer = 0, hoverId: string | null = null, playing: HTMLVideoElement | null = null;
function stopHover() { clearTimeout(hoverTimer); hoverTimer = 0; hoverId = null; if (playing) { playing.pause(); playing.removeAttribute('src'); playing.load(); playing.remove(); playing = null; } }
function hover(card: Card | null) { const id = card?.kind === 'video' ? card.id : null; if (id === hoverId) return; stopHover(); if (!id || !card || focused) return; hoverId = id; hoverTimer = window.setTimeout(() => { if (hoverId !== id || !cards.has(id)) return; const v = document.createElement('video'); v.muted = true; v.loop = true; v.playsInline = true; v.src = assets.get(card.asset)?.video || '/motion.mp4'; Object.assign(v.style, { position: 'absolute', left: `${card.x}px`, top: `${card.y + 32}px`, width: `${card.width}px`, height: `${Math.round(card.width * 9 / 16)}px`, objectFit: 'cover', background: '#161616' }); hoverLayer.append(v); playing = v; void v.play().catch(() => {}); }, 250); }
function closeFocus() { const v = focusPanel?.querySelector('video'); if (v) { v.pause(); v.removeAttribute('src'); v.load(); } focusPanel?.remove(); focusPanel = null; focused = null; }
function updateFocusMedia() { if (!focusPanel || !focused) return; const c = cards.get(focused); if (!c) return; const host = focusPanel.querySelector<HTMLElement>('[data-focus-media]')!; const old = host.querySelector('video'); if (old) { old.pause(); old.removeAttribute('src'); old.load(); } host.replaceChildren(); if (c.kind === 'video') { const v = document.createElement('video'); v.controls = true; v.playsInline = true; v.src = assets.get(c.asset)?.video || '/motion.mp4'; host.append(v); } else { const img = document.createElement('img'); img.src = assets.get(c.asset)?.full || assetUrl(c.asset, 512); img.alt = c.label; host.append(img); } focusPanel.querySelector<HTMLElement>('[data-focus-version]')!.textContent = `v${c.version || 1}`; }
function openFocus(id: string) {
  const c = cards.get(id); if (!c || switching) return; stopHover(); closeFocus(); select([id]); focused = id;
  const panel = document.createElement('section'); panel.className = 'demo-focus'; panel.setAttribute('role', 'dialog'); panel.setAttribute('aria-label', 'Focused asset');
  panel.innerHTML = `<header><button data-close>‹ Back to canvas</button><strong data-title></strong><span data-focus-version></span></header><div class="demo-focus-body"><div class="demo-focus-viewer" data-focus-media></div><aside><label>Name<input data-name aria-label="Asset name"></label><label>Prompt<textarea data-prompt aria-label="Prompt" rows="6"></textarea></label><label>Model<select data-model aria-label="Model"><option>Qwen Image 2.1 · Image</option><option>Flux Krea · Image</option><option>MiniMax H3 · Video</option></select></label><div class="demo-focus-versions"><button data-prev>← Previous take</button><button data-next>Next take →</button></div><button data-render>Render sample</button><p>Local sample project. Rendering and edits stay in this page.</p></aside></div>`;
  panel.querySelector<HTMLElement>('[data-title]')!.textContent = c.label;
  const name = panel.querySelector<HTMLInputElement>('[data-name]')!; name.value = c.label;
  const prompt = panel.querySelector<HTMLTextAreaElement>('[data-prompt]')!; prompt.value = prompts.get(id) || 'Soft morning light, detailed composition, natural colors.';
  const model = panel.querySelector<HTMLSelectElement>('[data-model]')!; model.value = c.tool || 'Qwen Image 2.1 · Image';
  name.addEventListener('change', () => { const before = c.label, after = name.value.trim() || before; patch(id, { label: after }); panel.querySelector<HTMLElement>('[data-title]')!.textContent = after; record(() => patch(id, { label: before }), () => patch(id, { label: after })); });
  prompt.addEventListener('input', () => prompts.set(id, prompt.value)); model.addEventListener('change', () => patch(id, { tool: model.value }));
  panel.querySelector('[data-close]')!.addEventListener('click', closeFocus);
  panel.querySelector('[data-render]')!.addEventListener('click', () => simulate(id));
  panel.querySelector('[data-prev]')!.addEventListener('click', () => { patch(id, { asset: c.kind === 'video' ? 5000 : (c.asset + 4999) % 5000, version: Math.max(1, (c.version || 1) - 1) }); updateFocusMedia(); });
  panel.querySelector('[data-next]')!.addEventListener('click', () => { patch(id, { asset: c.kind === 'video' ? 5000 : (c.asset + 1) % 5000, version: (c.version || 1) + 1 }); updateFocusMedia(); });
  document.body.append(panel); focusPanel = panel; updateFocusMedia();
}

let press: null | { id: number; point: { x: number; y: number }; camera: Camera; card: Card | null; before: { id: string; x: number; y: number }[]; moved: boolean; box: boolean; action: boolean; additive: boolean } = null;
const touches = new Map<number, { x: number; y: number }>(); let pinch: null | { distance: number; center: { x: number; y: number }; camera: Camera } = null;
surface.addEventListener('pointerdown', event => {
  closeMenu(); if (switching || event.button > 1) return; stopHover();
  const point = localPoint(event.clientX, event.clientY); const card = pick(event.clientX, event.clientY); const wp = worldPoint(event.clientX, event.clientY);
  if (event.pointerType === 'touch') { touches.set(event.pointerId, point); if (touches.size === 2) { const [a, b] = [...touches.values()]; pinch = { distance: Math.hypot(a.x - b.x, a.y - b.y), center: { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 }, camera: { ...camera } }; press = null; surface.setPointerCapture(event.pointerId); return; } }
  const pan = space || event.button === 1 || event.pointerType === 'touch';
  const trash = !!card && wp.y < card.y + 32 && wp.x > card.x + card.width - 25 && !pan;
  if (card && !pan && !trash) { if (event.shiftKey || event.metaKey || event.ctrlKey) { const next = new Set(selected); next.has(card.id) ? next.delete(card.id) : next.add(card.id); select(next); } else if (!selected.has(card.id)) select([card.id]); }
  const box = !card && event.shiftKey && !pan;
  press = { id: event.pointerId, point, camera: { ...camera }, card: pan ? null : card, before: [...selected].map(id => ({ id, x: cards.get(id)!.x, y: cards.get(id)!.y })), moved: false, box, action: trash, additive: event.shiftKey };
  surface.setPointerCapture(event.pointerId); event.preventDefault();
});
surface.addEventListener('pointermove', event => {
  const point = localPoint(event.clientX, event.clientY);
  if (touches.has(event.pointerId)) touches.set(event.pointerId, point);
  if (pinch && touches.size === 2) { const [a, b] = [...touches.values()], center = { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 }, z = Math.max(.015, Math.min(4, pinch.camera.zoom * Math.hypot(a.x - b.x, a.y - b.y) / pinch.distance)), ratio = z / pinch.camera.zoom; setCamera({ x: center.x - (pinch.center.x - pinch.camera.x) * ratio, y: center.y - (pinch.center.y - pinch.camera.y) * ratio, zoom: z }); return; }
  if (!press) { hover(pick(event.clientX, event.clientY)); return; }
  const dx = point.x - press.point.x, dy = point.y - press.point.y; if (Math.hypot(dx, dy) > 3) press.moved = true; if (!press.moved || press.action) return;
  if (press.box) { const x = Math.min(point.x, press.point.x), y = Math.min(point.y, press.point.y), w = Math.abs(dx), h = Math.abs(dy); marquee.hidden = false; Object.assign(marquee.style, { left: `${x}px`, top: `${y}px`, width: `${w}px`, height: `${h}px` }); }
  else if (press.card) move(press.before.map(c => ({ id: c.id, x: c.x + dx / camera.zoom, y: c.y + dy / camera.zoom })));
  else setCamera({ ...press.camera, x: press.camera.x + dx, y: press.camera.y + dy });
});
function endPointer(event: PointerEvent) {
  touches.delete(event.pointerId); if (pinch) { pinch = null; touches.clear(); press = null; return; } if (!press) return;
  const ending = press; press = null; marquee.hidden = true;
  if (ending.box && ending.moved) { const p = localPoint(event.clientX, event.clientY), x = (Math.min(p.x, ending.point.x) - camera.x) / camera.zoom, y = (Math.min(p.y, ending.point.y) - camera.y) / camera.zoom, w = Math.abs(p.x - ending.point.x) / camera.zoom, h = Math.abs(p.y - ending.point.y) / camera.zoom; const ids = [...cards.values()].filter(c => c.x < x + w && c.x + c.width > x && c.y < y + h && c.y + c.height > y).map(c => c.id); select(ending.additive ? [...selected, ...ids] : ids); }
  else if (ending.action && ending.card && !ending.moved) { select([ending.card.id]); removeSelected(); }
  else if (ending.card && ending.moved) { const before = ending.before.map(c => ({ ...c })), after = before.map(c => ({ id: c.id, x: cards.get(c.id)!.x, y: cards.get(c.id)!.y })); record(() => move(before), () => move(after)); }
  else if (!ending.card && !ending.moved && !ending.additive) select([]);
  scheduleUI();
}
surface.addEventListener('pointerup', endPointer); surface.addEventListener('pointercancel', event => { touches.delete(event.pointerId); pinch = null; press = null; marquee.hidden = true; });
surface.addEventListener('pointerleave', () => { if (!press) stopHover(); });
surface.addEventListener('dblclick', event => { const card = pick(event.clientX, event.clientY); if (card) openFocus(card.id); });
surface.addEventListener('contextmenu', menu);
surface.addEventListener('wheel', event => { event.preventDefault(); cancelTour(); stopHover(); if (event.ctrlKey || event.metaKey) zoomAt(Math.exp(-event.deltaY * .005), event.clientX, event.clientY); else { const p = event.shiftKey ? { x: event.deltaY || event.deltaX, y: 0 } : { x: event.deltaX, y: event.deltaY }; setCamera({ ...camera, x: camera.x - p.x, y: camera.y - p.y }); } }, { passive: false });

function cancelTour() { if (tourFrame) cancelAnimationFrame(tourFrame); tourFrame = 0; touring = false; }
function tour() { if (touring) return cancelTour(); home(); touring = true; const start = performance.now(), initial = { ...camera }; function frame(now: number) { if (!touring) return; const t = Math.max(0, Math.min(1, (now - start) / 7000)), u = Math.sin(t * Math.PI); const z = .85 + .4 * u; setCamera({ x: initial.x - 1250 * u, y: initial.y - 400 * Math.sin(t * Math.PI * 2), zoom: z }); if (t < 1) tourFrame = requestAnimationFrame(frame); else { touring = false; tourFrame = 0; } } tourFrame = requestAnimationFrame(frame); }
async function toggleModel(enabled: boolean) { viewerOn = enabled; if (!enabled) { viewer?.dispose(); viewer = null; viewerHost?.remove(); viewerHost = null; scheduleUI(); return; } const host = document.createElement('div'); host.className = 'demo-model-viewer'; Object.assign(host.style, { position: 'fixed', right: '20px', bottom: '95px', width: '360px', height: '230px', background: '#16181de8', border: '1px solid #414248', borderRadius: '10px', zIndex: '120' }); document.body.append(host); viewerHost = host; try { const next = await (await import('../src/three-demo')).createThreeDemo(host, 'webgl'); if (!viewerOn || viewerHost !== host) return next.dispose(); viewer = next; if (!matchMedia('(prefers-reduced-motion: reduce)').matches) viewer.start(); } catch (e) { ui.toast(`Model viewer: ${String(e)}`); viewerOn = false; host.remove(); } scheduleUI(); }
function toggleStats(enabled: boolean) { statsOn = enabled; cancelAnimationFrame(statsFrame); statsFrame = 0; intervals.length = 0; lastStats = performance.now(); lastFrame = 0; if (enabled) { const tick = (now: number) => { if (!statsOn) return; if (lastFrame) intervals.push(now - lastFrame); lastFrame = now; if (now - lastStats > 500) { const sorted = [...intervals].sort((a, b) => a - b); fps = 1000 / (intervals.reduce((a, b) => a + b, 0) / intervals.length || 1); p95 = sorted[Math.floor(sorted.length * .95)] || 0; intervals.length = 0; lastStats = now; scheduleUI(); } statsFrame = requestAnimationFrame(tick); }; statsFrame = requestAnimationFrame(tick); } scheduleUI(); }

const picker = document.createElement('input'); picker.type = 'file'; picker.multiple = true; picker.accept = 'image/*,video/*'; picker.hidden = true; document.body.append(picker);
async function blobFromCanvas(canvas: HTMLCanvasElement) { return new Promise<Blob>((resolve, reject) => canvas.toBlob(blob => blob ? resolve(blob) : reject(new Error('Could not create thumbnail')), 'image/webp', .88)); }
async function importFiles(files: FileList | File[]) {
  for (const file of [...files]) { if (!file.type.startsWith('image/') && !file.type.startsWith('video/')) continue;
    const full = URL.createObjectURL(file); blobs.push(full); let source: CanvasImageSource, release: () => void;
    const video = file.type.startsWith('video/');
    if (video) { const v = document.createElement('video'); v.muted = true; v.preload = 'auto'; v.src = full; await new Promise<void>((resolve, reject) => { v.onloadeddata = () => resolve(); v.onerror = () => reject(new Error('Could not read video')); }); source = v; release = () => { v.removeAttribute('src'); v.load(); }; }
    else { const bitmap = await createImageBitmap(file); source = bitmap; release = () => bitmap.close(); }
    const thumbs: string[] = [];
    for (const size of [256, 512]) { const cv = document.createElement('canvas'); cv.width = size; cv.height = Math.round(size * 9 / 16); const ctx = cv.getContext('2d')!; const sw = video ? (source as HTMLVideoElement).videoWidth : (source as ImageBitmap).width, sh = video ? (source as HTMLVideoElement).videoHeight : (source as ImageBitmap).height, ratio = 16 / 9, cw = Math.min(sw, sh * ratio), ch = cw / ratio; ctx.drawImage(source, (sw - cw) / 2, (sh - ch) / 2, cw, ch, 0, 0, cv.width, cv.height); const url = URL.createObjectURL(await blobFromCanvas(cv)); blobs.push(url); thumbs.push(url); } release();
    const asset = nextAsset++; assets.set(asset, { small: thumbs[0], large: thumbs[1], full, video: video ? full : undefined }); add(video ? 'video' : 'image', false); const c = cards.get([...selected][0])!; patch(c.id, { asset, label: file.name.replace(/\.[^.]+$/, ''), state: undefined, status: '', version: 1, tool: 'Imported · ' + (video ? 'Video' : 'Image') }); openFocus(c.id);
  }
}
picker.addEventListener('change', () => { if (picker.files) void importFiles(picker.files).catch(e => ui.toast(String(e))); picker.value = ''; });
surface.addEventListener('dragover', e => { e.preventDefault(); }); surface.addEventListener('drop', e => { e.preventDefault(); if (e.dataTransfer?.files) void importFiles(e.dataTransfer.files).catch(error => ui.toast(String(error))); });
const ui = createDemoUI({ mode: next => void switchMode(next), count: count => { if (switching) return; closeFocus(); baseCount = count; seed(count); undo.length = 0; redo.length = 0; home(); void switchMode(mode, true); }, home, fit, zoom: direction => zoomAt(1.25 ** direction), undo: undoOne, redo: redoOne,
  addImage: () => add('image'), addVideo: () => add('video'), files: () => picker.click(), duplicate, remove: removeSelected, tour,
  tilt: enabled => { tilted = enabled; perspective.set(enabled ? 15 : 0, enabled ? -18 : 0); scheduleUI(); }, model: enabled => void toggleModel(enabled), stats: toggleStats,
  help: () => {}, reset: () => { if (switching) return; closeFocus(); seed(baseCount); undo.length = 0; redo.length = 0; home(); void switchMode(mode, true); ui.toast('Sample project reset'); } });
function scheduleUI() { if (!uiFrame) uiFrame = requestAnimationFrame(updateUI); }
function updateUI() { uiFrame = 0; const s = adapter?.stats() || {}; const admitted = Number(s.visibleCards ?? s.visible ?? s.mountedCards ?? 0); const readyImages = Number(s.loadedImageElements ?? s.requestedTierReady ?? 0); const state: DemoUIState = { mode, total: cards.size, visible: admitted, selected: selected.size, zoom: camera.zoom, ready: admitted ? Math.min(1, readyImages / admitted) : 1,
  canUndo: !!undo.length, canRedo: !!redo.length, busy: switching, stats: statsOn, tilt: tilted, model: viewerOn, fps, p95, backend: mode === 'pixi' ? String(s.graphicsApi || 'WebGL') : 'Browser DOM compositor' }; ui.update(state); }
window.addEventListener('keydown', e => {
  if (e.key === 'Escape') { closeMenu(); closeFocus(); if (touring) cancelTour(); else select([]); return; }
  const target = e.target as HTMLElement; if (target.matches('input,textarea,select') || target.isContentEditable) return;
  if (e.code === 'Space' && !(target instanceof HTMLButtonElement)) { space = true; surface.style.cursor = 'grab'; e.preventDefault(); }
  if (e.key === '1') void switchMode('react-dom'); if (e.key === '2') void switchMode('pixi'); if (e.key === '3') void switchMode('reactflow');
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'z') { e.preventDefault(); e.shiftKey ? redoOne() : undoOne(); }
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'a') { e.preventDefault(); select(cards.keys()); }
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'd') { e.preventDefault(); duplicate(); }
  if (e.key === 'Delete' || e.key === 'Backspace') { e.preventDefault(); removeSelected(); }
  if (e.key === 'Enter' && selected.size === 1) openFocus([...selected][0]);
  if (e.key.toLowerCase() === 'f') fit(); if (e.key === '+' || e.key === '=') zoomAt(1.25); if (e.key === '-') zoomAt(.8);
});
window.addEventListener('keyup', e => { if (e.code === 'Space') { space = false; surface.style.cursor = ''; } });
window.addEventListener('blur', () => { space = false; press = null; stopHover(); });
document.addEventListener('pointerdown', e => { if (contextMenu && !contextMenu.contains(e.target as Node)) closeMenu(); });
document.addEventListener('visibilitychange', () => { if (document.hidden) { stopHover(); viewer?.stop(); } else if (viewerOn && !matchMedia('(prefers-reduced-motion: reduce)').matches) viewer?.start(); });
window.addEventListener('resize', () => { const oldW = width, oldH = height; width = innerWidth; height = innerHeight; camera.x += (width - oldW) / 2; camera.y += (height - oldH) / 2; perspective = createPerspective(surface, width, height); perspective.set(tilted ? 15 : 0, tilted ? -18 : 0); if (options) { options.width = width; options.height = height; } adapter?.resize?.(width, height); setCamera({ ...camera }); });
(window as any).canvasDemo = { state: () => ({ mode, total: cards.size, selected: [...selected], camera: { ...camera }, switching, ready: adapter?.readyFraction() ?? 0, stats: adapter?.stats(), focused, tilt: tilted, model: viewerOn }), cards: () => [...cards.values()].map(c => ({ ...c })), setMode: switchMode, home, fit, select, openFocus, closeFocus, add, remove: removeSelected, duplicate,
  project: (id: string) => { const c = cards.get(id)!; return perspective.project((c.x + c.width / 2) * camera.zoom + camera.x, (c.y + 32 + Math.round(c.width * 9 / 16) / 2) * camera.zoom + camera.y); } };
const logo = document.createElement('img'); logo.src = '/demo/cozy-icon.png'; logo.alt = 'Cozy'; logo.width = 38; logo.height = 38; document.querySelector('.demo-brand')?.replaceChildren(logo);
seed(baseCount); drawGrid(); void switchMode(mode, true);
window.addEventListener('pagehide', () => { cancelTour(); stopHover(); toggleStats(false); viewer?.dispose(); adapter?.destroy(); for (const t of jobs.values()) clearInterval(t); for (const url of blobs) URL.revokeObjectURL(url); });
