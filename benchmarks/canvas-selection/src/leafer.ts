import { Group, Leafer, Platform, Rect, RenderEvent, Resource, Text } from 'leafer-ui';
import type { Adapter, BenchOptions, Camera, Card } from './types';

const LOADS = 8;

interface Media {
  key: string;
  url: string;
  state: 'queued' | 'loading' | 'ready' | 'error';
  pins: Set<CardView>;
  bytes: number;
  used: number;
  resource?: Awaited<ReturnType<typeof Resource.loadImage>>;
  held?: boolean;
}

interface CardView {
  card: Card;
  root: Group;
  image: Rect;
  border: Rect;
  lod: number;
  name?: Text;
  version?: Text;
  status?: Text;
  requested?: Media;
  displayed?: Media;
  painted?: string;
}

/** Reject a broken browser drawing pipeline before counters can masquerade as visible output. */
async function verifyCanvas2DOutput(options: BenchOptions) {
  const canvas = document.createElement('canvas');
  canvas.width = canvas.height = 16;
  let image: HTMLImageElement | undefined;
  try {
    // Use the renderer's normal Canvas2D path, never a software-oriented readback hint.
    const context = canvas.getContext('2d');
    if (!context) throw new Error('Browser Canvas2D pipeline unavailable');
    context.clearRect(0, 0, 16, 16);
    context.fillStyle = '#ff0000';
    context.fillRect(0, 0, 16, 16);
    const solid = [...context.getImageData(4, 4, 1, 1).data];
    if (solid.join(',') !== '255,0,0,255') {
      throw new Error(`Browser Canvas2D pipeline broken: solid draw/readback expected [255,0,0,255], received [${solid}]`);
    }
    const card = options.cards[0];
    if (!card) throw new Error('Canvas2D photo capability check requires a benchmark fixture');
    image = new window.Image();
    const tier = options.tier(options.camera);
    image.src = options.assetUrl(card.asset, tier);
    await image.decode();
    context.clearRect(0, 0, 16, 16);
    context.drawImage(image, 0, 0, 16, 16);
    const pixels = context.getImageData(0, 0, 16, 16).data;
    const colors = new Set<string>();
    let visiblePixels = 0;
    for (let offset = 0; offset < pixels.length; offset += 4) {
      if (!pixels[offset + 3]) continue;
      visiblePixels++;
      colors.add(`${pixels[offset]},${pixels[offset + 1]},${pixels[offset + 2]}`);
    }
    if (!visiblePixels || colors.size < 2) {
      throw new Error(`Browser Canvas2D pipeline broken: photographic fixture draw/readback has ${visiblePixels} visible pixels and ${colors.size} colors`);
    }
    return { solid, photoVisiblePixels: visiblePixels, photoColors: colors.size, fixtureAsset: card.asset, fixtureTier: tier, context: 'normal Canvas2D' };
  } finally {
    image?.removeAttribute('src');
    canvas.width = canvas.height = 0;
  }
}

/** Native Leafer scene graph and dirty renderer, with an app-owned source-image LRU. */
export async function createLeaferAdapter(host: HTMLElement, options: BenchOptions): Promise<Adapter> {
  const requestedCacheMiB = Number(new URLSearchParams(location.search).get('cacheMiB') || 64);
  const cacheMiB = Number.isFinite(requestedCacheMiB) ? Math.min(1024, Math.max(1, requestedCacheMiB)) : 64;
  const budgetBytes = Math.round(cacheMiB * 1024 * 1024);
  const canvasOutputCapability = await verifyCanvas2DOutput(options);
  // Preserve the common asset URL, including the HTTP cache identity, across backends.
  const suffix = Platform.image.suffix;
  Platform.image.suffix = '';
  const leafer = new Leafer({
    view: host, width: options.width, height: options.height,
    pixelRatio: window.devicePixelRatio || 1, hittable: false,
  });
  leafer.hitChildren = false;
  leafer.interaction?.stop();
  const world = new Group({ hittable: false, hitChildren: false });
  leafer.add(world);
  const cards = options.cards.map((card) => ({ ...card }));
  const byId = new Map(cards.map((card) => [card.id, card]));
  const views = new Map<string, CardView>();
  const cache = new Map<string, Media>();
  const queue: Media[] = [];
  let camera = { ...options.camera };
  let selected: string | null = null;
  let dead = false, active = 0, clock = 0, draws = 0;
  let requests = 0, loaded = 0, failures = 0, evictions = 0;

  function pin(view: CardView, media?: Media) {
    if (!media) return;
    if (view.requested === media || view.displayed === media) media.pins.add(view);
    else media.pins.delete(view);
    // Keep the app's LRU authoritative after the last scene paint releases this source.
    if (!media.pins.size && media.resource && !media.held) { media.resource.use++; media.held = true; }
  }

  function discard(media: Media) {
    if (cache.get(media.key) !== media) return;
    cache.delete(media.key);
    if (media.resource) {
      if (media.held) media.resource.use--;
      if (Resource.get(media.url) === media.resource) Resource.remove(media.url);
      else media.resource.destroy();
    }
    evictions++;
  }

  function trim() {
    // Native in-flight loads have no AbortSignal API; retire them once they settle.
    for (const media of cache.values()) {
      if (!media.pins.size && (media.state === 'queued' || media.state === 'error')) discard(media);
    }
    let bytes = [...cache.values()].reduce((sum, media) => sum + media.bytes, 0);
    const unused = [...cache.values()].filter((media) => !media.pins.size && media.state === 'ready' && (media.resource?.use ?? 0) <= 1).sort((a, b) => a.used - b.used);
    for (const media of unused) {
      if (bytes <= budgetBytes) break;
      bytes -= media.bytes;
      discard(media);
    }
  }

  function show(view: CardView, media: Media) {
    if (dead || view.requested !== media) return;
    const previous = view.displayed;
    view.image.fill = { type: 'image', url: media.url, mode: 'cover' };
    view.displayed = media;
    media.used = ++clock;
    pin(view, previous);
    pin(view, media);
  }

  function pump() {
    while (!dead && active < LOADS && queue.length) {
      const media = queue.shift()!;
      if (cache.get(media.key) !== media || !media.pins.size) continue;
      active++;
      requests++;
      media.state = 'loading';
      const loading = Resource.loadImage(media.url);
      media.resource = Resource.get(media.url);
      media.resource!.use++;
      media.held = true;
      void loading.then((image) => {
        loaded++;
        if (dead || cache.get(media.key) !== media) {
          // A newer owner of this same resource must not be invalidated by an old completion.
          if (Resource.get(media.url) === image) Resource.remove(media.url);
          return;
        }
        media.bytes = image.width * image.height * 4;
        media.state = 'ready';
        for (const view of media.pins) show(view, media);
        trim();
      }).catch((error: unknown) => {
        if (dead || cache.get(media.key) !== media) return;
        failures++;
        media.state = 'error';
        console.error('Leafer thumbnail failed', media.key, error);
      }).finally(() => { active--; pump(); });
    }
  }

  function request(view: CardView, tier: number) {
    const key = `${view.card.asset}:${tier}`;
    if (view.requested?.key === key) return;
    const previous = view.requested;
    let media = cache.get(key);
    if (!media) {
      media = { key, url: options.assetUrl(view.card.asset, tier), state: 'queued', pins: new Set(), bytes: 0, used: ++clock };
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
    view.border.stroke = selected === view.card.id ? '#65b8ff' : '#3e4957';
    view.border.strokeWidth = selected === view.card.id ? 2 : 1;
    if (view.lod === lod) return;
    for (const text of [view.name, view.version, view.status]) text?.destroy();
    view.name = view.version = view.status = undefined;
    if (lod < 2) {
      view.name = new Text({ x: 8, y: 7, text: view.card.label, fontFamily: 'Arial, sans-serif', fontSize: 12, fill: '#e5eaf1', hittable: false });
      view.root.add(view.name);
    }
    if (lod === 0) {
      view.version = new Text({ x: view.card.width - 24, y: 7, text: 'v1', fontFamily: 'Arial, sans-serif', fontSize: 12, fill: '#a3afbd', hittable: false });
      view.status = new Text({ x: 8, y: 174, text: view.card.status, fontFamily: 'Arial, sans-serif', fontSize: 12, fill: '#a3afbd', hittable: false });
      view.root.add(view.version);
      view.root.add(view.status);
    }
    view.lod = lod;
  }

  function remove(view: CardView) {
    const requested = view.requested, displayed = view.displayed;
    view.requested = view.displayed = undefined;
    pin(view, requested);
    pin(view, displayed);
    view.root.destroy();
    views.delete(view.card.id);
  }

  function sync() {
    if (dead) return;
    world.set({ x: camera.x, y: camera.y, scaleX: camera.zoom, scaleY: camera.zoom });
    const tier = options.tier(camera), lod = options.lod(camera);
    for (const view of views.values()) if (!options.visible(view.card, camera)) remove(view);
    for (const card of cards) {
      if (!options.visible(card, camera)) continue;
      let view = views.get(card.id);
      if (!view) {
        const root = new Group({ x: card.x, y: card.y, hittable: false, hitChildren: false });
        const background = new Rect({ width: card.width, height: card.height, cornerRadius: 8, fill: '#20262d', hittable: false });
        const image = new Rect({ x: 1, y: 28, width: card.width - 2, height: 134, hittable: false });
        const border = new Rect({ x: 0.5, y: 0.5, width: card.width - 1, height: card.height - 1, cornerRadius: 8, stroke: '#3e4957', strokeWidth: 1, hittable: false });
        root.add(background);
        root.add(image);
        root.add(border);
        world.add(root);
        view = { card, root, image, border, lod: -1 };
        views.set(card.id, view);
        chrome(view, lod);
      } else if (view.lod !== lod) chrome(view, lod);
      view.root.set({ x: card.x, y: card.y });
      request(view, tier);
    }
    trim();
    pump();
  }

  const rendered = () => {
    if (dead) return;
    for (const view of views.values()) if (view.displayed) view.painted = view.displayed.key;
    draws++;
    trim();
    options.onDraw();
  };
  const beforeRender = () => {
    // Once native paint owns the source, avoid an artificial second use and its extra pattern cache.
    for (const media of cache.values()) {
      if (media.pins.size && media.held && media.resource && media.resource.use > 1) {
        media.resource.use--;
        media.held = false;
      }
    }
  };
  leafer.on(RenderEvent.BEFORE, beforeRender);
  leafer.on(RenderEvent.AFTER, rendered);
  sync();

  return {
    setCamera(next) { camera = { ...next }; sync(); },
    setSelected(id) {
      const previous = selected;
      selected = id;
      for (const key of [previous, selected]) { const view = key ? views.get(key) : undefined; if (view) chrome(view, view.lod); }
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
        backend: 'leafer-canvas2d', graphicsApi: 'Canvas2D', renderer: 'browser Canvas2D; acceleration not inferred',
        canvasOutputCapability,
        cards: cards.length, visible: views.size, lod: options.lod(camera), tier: options.tier(camera), draws,
        cacheEntries: cache.size, cachedImages: media.filter((entry) => entry.state === 'ready').length,
        retainedImageNominalBytes: bytes, cacheSourceBudgetBytes: budgetBytes,
        pinnedNominalBytes: pinned, pinnedOverflowBytes: Math.max(0, pinned - budgetBytes),
        pending: media.filter((entry) => entry.state === 'queued' || entry.state === 'loading').length,
        activeLoads: active, requests, loaded, failures, evictions,
        requestedTierReady: [...views.values()].filter((view) => view.requested?.state === 'ready').length,
        sceneCanvases: host.querySelectorAll('canvas').length,
        canvasBackingNominalBytes: [...host.querySelectorAll('canvas')].reduce((sum, canvas) => sum + canvas.width * canvas.height * 4, 0),
        memoryScope: 'source RGBA8 estimate; native pattern/cache/Canvas2D GPU allocations opaque and not included',
        automaticDirtyRenderer: true,
      };
    },
    destroy() {
      dead = true;
      leafer.off(RenderEvent.AFTER, rendered);
      leafer.off(RenderEvent.BEFORE, beforeRender);
      for (const view of views.values()) remove(view);
      for (const media of cache.values()) discard(media);
      queue.length = 0;
      leafer.destroy();
      Platform.image.suffix = suffix;
    },
  };
}
