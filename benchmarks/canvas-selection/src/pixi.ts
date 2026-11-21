import { Application, Container, Graphics, ImageSource, Rectangle, RendererType, Sprite, Text, Texture, WebGPURenderer } from 'pixi.js';
import type { Adapter, BenchOptions, Camera, Card } from './types';

const LOADS = 8;

interface Media {
  key: string;
  asset: number;
  tier: number;
  state: 'queued' | 'loading' | 'ready' | 'error';
  pins: Set<CardView>;
  used: number;
  bytes: number;
  uploaded: boolean;
  abort?: AbortController;
  bitmap?: ImageBitmap;
  texture?: Texture;
}

interface CardView {
  card: Card;
  root: Container;
  chrome: Graphics;
  border: Graphics;
  sprite: Sprite;
  name?: Text;
  version?: Text;
  status?: Text;
  tool?: Text;
  aspect?: Text;
  decorations?: Graphics;
  lod: number;
  requested?: Media;
  displayed?: Media;
  crop?: Texture;
  painted?: string;
}

/** Explicit WebGL baseline. No ticker: input, camera and completed loads invalidate one frame. */
export async function createPixiAdapter(host: HTMLElement, options: BenchOptions): Promise<Adapter> {
  return createAdapter(host, options, 'webgl');
}

/** Experimental lane, with exactly the same scene, source cache and redraw policy. */
export async function createPixiWebGPUAdapter(host: HTMLElement, options: BenchOptions): Promise<Adapter> {
  return createAdapter(host, options, 'webgpu');
}

async function createAdapter(host: HTMLElement, options: BenchOptions, backend: 'webgl' | 'webgpu'): Promise<Adapter> {
  const creator = options.appearance === 'creator';
  const measureContext = creator ? document.createElement('canvas').getContext('2d') : null;
  const requestedCacheMiB = Number(new URLSearchParams(location.search).get('cacheMiB') || 64);
  const cacheMiB = Number.isFinite(requestedCacheMiB) ? Math.min(1024, Math.max(1, requestedCacheMiB)) : 64;
  const budgetBytes = Math.round(cacheMiB * 1024 * 1024);
  const app = new Application();
  await app.init({
    width: options.width, height: options.height,
    resolution: window.devicePixelRatio || 1, autoDensity: true,
    preference: [backend], autoStart: false, sharedTicker: false,
    antialias: true, backgroundAlpha: 0, gcActive: false,
    eventFeatures: { move: false, globalMove: false, click: false, wheel: false },
  });
  app.stage.eventMode = 'none';
  app.stage.interactiveChildren = false;
  // The documented null target also removes Pixi's internal interaction ticker.
  app.renderer.events.setTargetElement(null!);
  app.canvas.style.pointerEvents = 'none';
  host.append(app.canvas);
  const world = new Container({ eventMode: 'none', interactiveChildren: false, sortableChildren: options.appearance === 'creator' });
  app.stage.addChild(world);
  const isWebGPU = app.renderer.type === RendererType.WEBGPU;
  const gl = isWebGPU ? null : app.canvas.getContext('webgl2') || app.canvas.getContext('webgl');
  if ((backend === 'webgpu') !== isWebGPU || (!isWebGPU && !gl)) {
    app.destroy(true, { children: true });
    throw new Error(`Pixi benchmark requires the requested ${backend} backend`);
  }
  const gpu = isWebGPU ? (app.renderer as WebGPURenderer).gpu : null;
  const adapterInfo = gpu?.adapter.info;
  const debug = gl?.getExtension('WEBGL_debug_renderer_info');
  const renderer = gl ? (debug ? gl.getParameter(debug.UNMASKED_RENDERER_WEBGL) : gl.getParameter(gl.RENDERER))
    : adapterInfo?.description || adapterInfo?.device || 'WebGPU adapter description unavailable';
  const vendor = gl ? (debug ? gl.getParameter(debug.UNMASKED_VENDOR_WEBGL) : gl.getParameter(gl.VENDOR)) : adapterInfo?.vendor;
  const graphicsApi = gl ? gl.getParameter(gl.VERSION) : 'WebGPU';
  const cards = options.cards.map((card) => ({ ...card }));
  const byId = new Map(cards.map((card) => [card.id, card]));
  const views = new Map<string, CardView>();
  const cache = new Map<string, Media>();
  const queue: Media[] = [];
  let camera = { ...options.camera };
  const selected = new Set<string>();
  let dead = false, frame = 0, active = 0, clock = 0, draws = 0;
  let requests = 0, decodes = 0, uploads = 0, failures = 0, evictions = 0, closedBitmaps = 0;
  let contextLost = false, contextLosses = 0;

  function mediaRect(card: Card) {
    return creator
      ? { x: 0, y: 32, width: card.width, height: Math.round(card.width * 9 / 16) }
      : { x: 1, y: 28, width: card.width - 2, height: 134 };
  }

  function measure(value: string, size: number, bold = false) {
    if (!measureContext) return Array.from(value).length * size * 0.61;
    measureContext.font = `${bold ? 700 : 400} ${size}px monospace`;
    return measureContext.measureText(value).width;
  }

  function clipped(value: string, width: number, size: number, bold = false) {
    if (measure(value, size, bold) <= width) return value;
    const characters = Array.from(value);
    let low = 0, high = characters.length;
    while (low < high) {
      const middle = Math.ceil((low + high) / 2);
      if (measure(`${characters.slice(0, middle).join('')}…`, size, bold) <= width) low = middle;
      else high = middle - 1;
    }
    return low ? `${characters.slice(0, low).join('')}…` : '';
  }

  function invalidate() {
    if (dead || frame || contextLost) return;
    frame = requestAnimationFrame(() => {
      frame = 0;
      if (dead || contextLost) return;
      app.render();
      for (const view of views.values()) {
        const media = view.displayed;
        if (!media) continue;
        view.painted = media.key;
        if (!media.uploaded) { media.uploaded = true; uploads++; }
      }
      draws++;
      options.onDraw();
    });
  }

  function pin(view: CardView, media?: Media) {
    if (!media) return;
    if (view.requested === media || view.displayed === media) media.pins.add(view);
    else media.pins.delete(view);
  }

  function discard(media: Media) {
    if (cache.get(media.key) !== media) return;
    cache.delete(media.key);
    media.abort?.abort();
    media.texture?.destroy(true);
    if (media.bitmap) { media.bitmap.close(); closedBitmaps++; }
    media.texture = undefined;
    media.bitmap = undefined;
    evictions++;
  }

  function trim() {
    // Obsolete requests should not decode unseen cards, even while below the byte cap.
    for (const media of cache.values()) {
      if (!media.pins.size && media.state !== 'ready') discard(media);
    }
    let bytes = [...cache.values()].reduce((sum, media) => sum + media.bytes, 0);
    if (bytes <= budgetBytes) return;
    const unused = [...cache.values()].filter((media) => !media.pins.size).sort((a, b) => a.used - b.used);
    for (const media of unused) {
      if (bytes <= budgetBytes) break;
      bytes -= media.bytes;
      discard(media);
    }
  }

  function show(view: CardView, media: Media, refreshDisplayedGeometry = false) {
    if (dead || (view.requested !== media && !(refreshDisplayedGeometry && view.displayed === media)) || !media.texture || !media.bitmap) return;
    const previous = view.displayed;
    const { width, height } = media.bitmap;
    const rect = mediaRect(view.card);
    const ratio = rect.width / rect.height;
    const cropWidth = Math.min(width, height * ratio);
    const cropHeight = Math.min(height, width / ratio);
    view.crop?.destroy();
    view.crop = new Texture({
      source: media.texture.source,
      frame: new Rectangle((width - cropWidth) / 2, (height - cropHeight) / 2, cropWidth, cropHeight),
    });
    view.sprite.texture = view.crop;
    view.sprite.position.set(rect.x, rect.y);
    view.sprite.width = rect.width;
    view.sprite.height = rect.height;
    view.sprite.visible = true;
    view.displayed = media;
    media.used = ++clock;
    pin(view, previous);
    pin(view, media);
    invalidate();
  }

  function pump() {
    while (!dead && active < LOADS && queue.length) {
      const media = queue.shift()!;
      if (cache.get(media.key) !== media || !media.pins.size) continue;
      media.state = 'loading';
      media.abort = new AbortController();
      active++;
      requests++;
      void (async () => {
        const response = await fetch(options.assetUrl(media.asset, media.tier), { signal: media.abort!.signal });
        if (!response.ok) throw new Error(`Thumbnail HTTP ${response.status}`);
        const bitmap = await createImageBitmap(await response.blob());
        decodes++;
        if (dead || cache.get(media.key) !== media) { bitmap.close(); closedBitmaps++; return; }
        media.bitmap = bitmap;
        media.bytes = bitmap.width * bitmap.height * 4;
        media.texture = new Texture({ source: new ImageSource({ resource: bitmap, autoGenerateMipmaps: false }) });
        media.state = 'ready';
        for (const view of media.pins) show(view, media);
        trim();
      })().catch((error: unknown) => {
        if (dead || cache.get(media.key) !== media) return;
        media.state = 'error';
        failures++;
        console.error('Pixi thumbnail failed', media.key, error);
      }).finally(() => { active--; pump(); });
    }
  }

  function request(view: CardView, tier: number) {
    const key = `${view.card.asset}:${tier}`;
    if (view.requested?.key === key) return;
    const previous = view.requested;
    let media = cache.get(key);
    if (!media) {
      media = { key, asset: view.card.asset, tier, state: 'queued', pins: new Set(), used: ++clock, bytes: 0, uploaded: false };
      cache.set(key, media);
      queue.push(media);
    }
    view.requested = media;
    pin(view, previous);
    pin(view, media);
    media.used = ++clock;
    if (media.state === 'ready') show(view, media);
  }

  type TextPart = 'name' | 'version' | 'status' | 'tool' | 'aspect';
  function removeText(view: CardView, part: TextPart) {
    view[part]?.destroy();
    view[part] = undefined;
  }

  function creatorText(view: CardView, part: TextPart, value: string, size: number, color: number, x: number, y: number, bold = false) {
    let text = view[part];
    if (!text) {
      text = new Text({ text: value, style: { fontFamily: part === 'tool' || part === 'status' ? ['Creator Rounded', 'Trebuchet MS', 'sans-serif'] : 'monospace', fontSize: size, fontWeight: bold ? '700' : '400', fill: color } });
      view[part] = text;
      view.root.addChild(text);
    } else {
      text.text = value;
      text.style.fill = color;
    }
    text.position.set(x, y);
    return text;
  }

  function creatorChrome(view: CardView, lod: number, forceText: boolean) {
    const card = view.card;
    const rect = mediaRect(card);
    const stateColor = card.state === 'ready' ? 0x55c98a : card.state === 'rendering' ? 0x66b6f4 : card.state === 'error' ? 0xe66a64 : 0xa0a3a8;
    const line = selected.has(card.id) ? 2 : 1;
    view.root.zIndex = selected.has(card.id) ? 1000 : 0;
    view.chrome.clear().rect(rect.x, rect.y, rect.width, rect.height).fill(0x1b1c20);
    view.border.clear().rect(line / 2, rect.y + line / 2, rect.width - line, rect.height - line)
      .stroke({ width: line, color: selected.has(card.id) ? 0xff9a5e : 0x414248 });
    const decoration = view.decorations!.clear();
    if (lod < 2) decoration.circle(4, 15, 4).fill(stateColor);
    if (lod === 0) {
      // The same 16px trash glyph as the DOM card's 24px button.
      const x = card.width - 20, y = 8, scale = 16 / 24;
      decoration.moveTo(x + 3 * scale, y + 6 * scale).lineTo(x + 21 * scale, y + 6 * scale)
        .moveTo(x + 9 * scale, y + 6 * scale).lineTo(x + 9 * scale, y + 4 * scale).lineTo(x + 15 * scale, y + 4 * scale).lineTo(x + 15 * scale, y + 6 * scale)
        .moveTo(x + 6 * scale, y + 6 * scale).lineTo(x + 7 * scale, y + 20 * scale).lineTo(x + 17 * scale, y + 20 * scale).lineTo(x + 18 * scale, y + 6 * scale)
        .moveTo(x + 10 * scale, y + 10 * scale).lineTo(x + 10 * scale, y + 16 * scale)
        .moveTo(x + 14 * scale, y + 10 * scale).lineTo(x + 14 * scale, y + 16 * scale)
        .stroke({ width: 1.5 * scale, color: 0x999999 });
    }
    if (card.kind === 'video') {
      const cx = rect.width / 2, cy = rect.y + rect.height / 2;
      decoration.poly([cx - 8, cy - 12, cx + 12, cy, cx - 8, cy + 12]).fill({ color: 0xffffff, alpha: 0.9 });
    }
    if (card.state === 'rendering' && card.progress !== undefined) {
      const progress = Math.min(1, Math.max(0, card.progress));
      if (progress > 0) decoration.rect(1, rect.y + 1, (rect.width - 2) * progress, 3).fill(0x66b6f4);
    }
    if (view.lod === lod && !forceText) return;
    if (lod < 2) {
      const version = `v${card.version ?? 1}`;
      const reserved = lod === 0 ? measure(version, 14) + 8 + 28 : 8;
      const name = clipped(card.label, card.width - 14 - reserved, 21, true);
      creatorText(view, 'name', name, 21, 0xe7e7e7, 14, 3, true);
      if (lod === 0) creatorText(view, 'version', version, 14, 0xaaaaaa, 14 + measure(name, 21, true) + 8, 8);
      else removeText(view, 'version');
    } else {
      removeText(view, 'name');
      removeText(view, 'version');
    }
    if (lod === 0) {
      const footerY = rect.y + rect.height + 8;
      creatorText(view, 'tool', clipped(card.tool ?? '', card.width - measure('16:9', 14) - 8, 14), 14, 0xaaaaaa, 0, footerY);
      creatorText(view, 'aspect', '16:9', 14, 0xaaaaaa, card.width - measure('16:9', 14), footerY);
      if (card.state === 'rendering' || card.state === 'error') {
        creatorText(view, 'status', clipped(card.status, card.width, 14), 14, stateColor, 0, rect.y + rect.height + 24);
      } else removeText(view, 'status');
    } else {
      removeText(view, 'tool');
      removeText(view, 'aspect');
      removeText(view, 'status');
    }
    view.lod = lod;
  }

  function chrome(view: CardView, lod: number, forceText = false) {
    if (creator) { creatorChrome(view, lod, forceText); return; }
    const { width, height } = view.card;
    view.chrome.clear().roundRect(0, 0, width, height, 8).fill(0x20262d);
    view.border.clear().roundRect(0.5, 0.5, width - 1, height - 1, 8)
      .stroke({ width: selected.has(view.card.id) ? 2 : 1, color: selected.has(view.card.id) ? 0x65b8ff : 0x3e4957 });
    if (view.lod === lod && !forceText) return;
    for (const text of [view.name, view.version, view.status]) text?.destroy();
    view.name = view.version = view.status = undefined;
    if (lod < 2) {
      view.name = new Text({ text: view.card.label, style: { fontFamily: 'Arial, sans-serif', fontSize: 12, fill: 0xe5eaf1 } });
      view.name.position.set(8, 7);
      view.root.addChild(view.name);
    }
    if (lod === 0) {
      view.version = new Text({ text: 'v1', style: { fontFamily: 'Arial, sans-serif', fontSize: 12, fill: 0xa3afbd } });
      view.version.position.set(width - 24, 7);
      view.status = new Text({ text: view.card.status, style: { fontFamily: 'Arial, sans-serif', fontSize: 12, fill: 0xa3afbd } });
      view.status.position.set(8, 174);
      view.root.addChild(view.version, view.status);
    }
    view.lod = lod;
  }

  function remove(view: CardView) {
    const requested = view.requested, displayed = view.displayed;
    view.requested = view.displayed = undefined;
    pin(view, requested);
    pin(view, displayed);
    view.crop?.destroy();
    view.root.destroy({ children: true });
    views.delete(view.card.id);
  }

  function sync() {
    if (dead) return;
    world.position.set(camera.x, camera.y);
    world.scale.set(camera.zoom);
    const tier = options.tier(camera), lod = options.lod(camera);
    for (const view of views.values()) if (!options.visible(view.card, camera)) remove(view);
    for (const card of cards) {
      if (!options.visible(card, camera)) continue;
      let view = views.get(card.id);
      if (!view) {
        const root = new Container({ eventMode: 'none', interactiveChildren: false });
        const background = new Graphics(), border = new Graphics(), sprite = new Sprite(Texture.EMPTY);
        const rect = mediaRect(card);
        sprite.position.set(rect.x, rect.y);
        sprite.visible = false;
        root.addChild(background, sprite, border);
        world.addChild(root);
        view = { card, root, chrome: background, border, sprite, lod: -1 };
        if (creator) {
          view.decorations = new Graphics();
          root.addChild(view.decorations);
        }
        views.set(card.id, view);
        chrome(view, lod);
      } else if (view.lod !== lod) chrome(view, lod);
      view.root.position.set(card.x, card.y);
      request(view, tier);
    }
    trim();
    pump();
    invalidate();
  }

  function setSelection(ids: string[]) {
    const next = new Set(ids.filter(id => byId.has(id)));
    const changed = [...new Set([...selected, ...next])].filter(id => selected.has(id) !== next.has(id));
    if (!changed.length) return;
    selected.clear();
    for (const id of next) selected.add(id);
    for (const id of changed) {
      const view = views.get(id);
      if (view) chrome(view, view.lod);
    }
    invalidate();
  }

  function moveCards(moves: { id: string; x: number; y: number }[]) {
    let changed = false;
    for (const { id, x, y } of moves) {
      const card = byId.get(id);
      if (!card || (card.x === x && card.y === y)) continue;
      card.x = x; card.y = y;
      changed = true;
    }
    if (changed) sync();
  }

  function updateCard(id: string, patch: Partial<Card>) {
    const card = byId.get(id);
    if (!card) return;
    const geometry = (patch.width !== undefined && patch.width !== card.width) || (patch.height !== undefined && patch.height !== card.height);
    const moved = (patch.x !== undefined && patch.x !== card.x) || (patch.y !== undefined && patch.y !== card.y);
    const assetChanged = patch.asset !== undefined && patch.asset !== card.asset;
    const textChanged = (['label', 'version', 'tool', 'state', 'status'] as const).some(key => key in patch && patch[key] !== card[key]);
    Object.assign(card, patch, { id });
    const view = views.get(id);
    if (view) {
      chrome(view, view.lod, textChanged || geometry);
      if (geometry && view.displayed) show(view, view.displayed, true);
    }
    if (geometry || moved) sync();
    else {
      if (assetChanged && view) { request(view, options.tier(camera)); trim(); pump(); }
      if (view) invalidate();
    }
  }

  const lost = (event: Event) => {
    event.preventDefault();
    contextLost = true;
    contextLosses++;
    for (const media of cache.values()) media.uploaded = false;
  };
  const restored = () => { contextLost = false; invalidate(); };
  app.canvas.addEventListener('webglcontextlost', lost);
  app.canvas.addEventListener('webglcontextrestored', restored);
  sync();

  return {
    setCamera(next) { camera = { ...next }; sync(); },
    setSelected(id) { setSelection(id === null ? [] : [id]); },
    setSelection,
    moveCard(id, x, y) { moveCards([{ id, x, y }]); },
    moveCards,
    updateCard,
    addCard(value) {
      if (byId.has(value.id)) { updateCard(value.id, value); return; }
      const card = { ...value };
      cards.push(card);
      byId.set(card.id, card);
      sync();
    },
    removeCard(id) {
      const card = byId.get(id);
      if (!card) return;
      const view = views.get(id);
      if (view) remove(view);
      byId.delete(id);
      selected.delete(id);
      cards.splice(cards.indexOf(card), 1);
      sync();
    },
    resize(width, height) {
      app.renderer.resize(width, height, window.devicePixelRatio || 1);
      sync();
    },
    readyFraction() {
      if (!views.size) return 1;
      return [...views.values()].filter((view) => view.requested && view.displayed === view.requested && view.painted === view.requested.key).length / views.size;
    },
    stats() {
      const media = [...cache.values()];
      const bytes = media.reduce((sum, entry) => sum + entry.bytes, 0);
      const pinned = media.filter((entry) => entry.pins.size).reduce((sum, entry) => sum + entry.bytes, 0);
      return {
        backend: `pixi-${backend}`, graphicsApi, renderer, vendor,
        webgpuAdapterInfo: adapterInfo ? { vendor: adapterInfo.vendor, architecture: adapterInfo.architecture, device: adapterInfo.device, description: adapterInfo.description, isFallbackAdapter: adapterInfo.isFallbackAdapter } : null,
        cards: cards.length, visible: views.size, appearance: creator ? 'creator' : 'benchmark', selected: selected.size, lod: options.lod(camera), tier: options.tier(camera), draws,
        cacheEntries: cache.size, cachedImages: media.filter((entry) => entry.bitmap).length,
        mediaTextures: media.filter((entry) => entry.uploaded).length,
        mediaTextureNominalBytes: media.filter((entry) => entry.uploaded).reduce((sum, entry) => sum + entry.bytes, 0),
        retainedBitmapNominalBytes: bytes, cacheSourceBudgetBytes: budgetBytes,
        pinnedNominalBytes: pinned, pinnedOverflowBytes: Math.max(0, pinned - budgetBytes),
        pending: media.filter((entry) => entry.state === 'queued' || entry.state === 'loading').length,
        activeLoads: active, requests, decodes, uploads, failures, evictions, closedBitmaps,
        requestedTierReady: [...views.values()].filter((view) => view.requested?.state === 'ready').length,
        textObjects: [...views.values()].reduce((sum, view) => sum + Number(!!view.name) + Number(!!view.version) + Number(!!view.status) + Number(!!view.tool) + Number(!!view.aspect), 0),
        memoryScope: 'RGBA8 media only; bitmap/source byte budget; excludes text textures, geometry, framebuffer and driver allocations',
        contextLost, contextLosses, scheduledFrame: !!frame, automaticTicker: false,
      };
    },
    destroy() {
      dead = true;
      if (frame) cancelAnimationFrame(frame);
      frame = 0;
      app.canvas.removeEventListener('webglcontextlost', lost);
      app.canvas.removeEventListener('webglcontextrestored', restored);
      for (const view of views.values()) remove(view);
      for (const media of cache.values()) discard(media);
      queue.length = 0;
      app.destroy(true, { children: true });
    },
  };
}
