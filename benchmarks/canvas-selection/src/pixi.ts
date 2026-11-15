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
  const world = new Container({ eventMode: 'none', interactiveChildren: false });
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
  let selected: string | null = null;
  let dead = false, frame = 0, active = 0, clock = 0, draws = 0;
  let requests = 0, decodes = 0, uploads = 0, failures = 0, evictions = 0, closedBitmaps = 0;
  let contextLost = false, contextLosses = 0;

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

  function show(view: CardView, media: Media) {
    if (dead || view.requested !== media || !media.texture || !media.bitmap) return;
    const previous = view.displayed;
    const { width, height } = media.bitmap;
    const ratio = (view.card.width - 2) / 134;
    const cropWidth = Math.min(width, height * ratio);
    const cropHeight = Math.min(height, width / ratio);
    view.crop?.destroy();
    view.crop = new Texture({
      source: media.texture.source,
      frame: new Rectangle((width - cropWidth) / 2, (height - cropHeight) / 2, cropWidth, cropHeight),
    });
    view.sprite.texture = view.crop;
    view.sprite.width = view.card.width - 2;
    view.sprite.height = 134;
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

  function chrome(view: CardView, lod: number) {
    const { width, height } = view.card;
    view.chrome.clear().roundRect(0, 0, width, height, 8).fill(0x20262d);
    view.border.clear().roundRect(0.5, 0.5, width - 1, height - 1, 8)
      .stroke({ width: selected === view.card.id ? 2 : 1, color: selected === view.card.id ? 0x65b8ff : 0x3e4957 });
    if (view.lod === lod) return;
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
        sprite.position.set(1, 28);
        sprite.visible = false;
        root.addChild(background, sprite, border);
        world.addChild(root);
        view = { card, root, chrome: background, border, sprite, lod: -1 };
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
    setSelected(id) {
      const previous = selected;
      selected = id;
      for (const key of [previous, selected]) { const view = key ? views.get(key) : undefined; if (view) chrome(view, view.lod); }
      invalidate();
    },
    moveCard(id, x, y) { const card = byId.get(id); if (card) { card.x = x; card.y = y; sync(); } },
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
        cards: cards.length, visible: views.size, lod: options.lod(camera), tier: options.tier(camera), draws,
        cacheEntries: cache.size, cachedImages: media.filter((entry) => entry.bitmap).length,
        mediaTextures: media.filter((entry) => entry.uploaded).length,
        mediaTextureNominalBytes: media.filter((entry) => entry.uploaded).reduce((sum, entry) => sum + entry.bytes, 0),
        retainedBitmapNominalBytes: bytes, cacheSourceBudgetBytes: budgetBytes,
        pinnedNominalBytes: pinned, pinnedOverflowBytes: Math.max(0, pinned - budgetBytes),
        pending: media.filter((entry) => entry.state === 'queued' || entry.state === 'loading').length,
        activeLoads: active, requests, decodes, uploads, failures, evictions, closedBitmaps,
        requestedTierReady: [...views.values()].filter((view) => view.requested?.state === 'ready').length,
        textObjects: [...views.values()].reduce((sum, view) => sum + Number(!!view.name) + Number(!!view.version) + Number(!!view.status), 0),
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
