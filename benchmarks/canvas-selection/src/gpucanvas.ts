import {
  createGpuCanvas,
  supportsGpuCanvas,
  type GpuCanvas,
  type GpuCanvasStats,
} from 'gpucanvas';
import type { Adapter, BenchOptions, Camera, Card } from './types';

/** Does not request an adapter/device or substitute a DOM backend. */
export function probeGpuCanvasCompatibility(): Record<string, unknown> {
  const canvasPrototype = HTMLCanvasElement.prototype;
  const queuePrototype = (globalThis as unknown as {
    GPUQueue?: { prototype: object };
  }).GPUQueue?.prototype;
  const element = document.createElement('canvas');
  element.setAttribute('content', 'drawable');
  return {
    supportsGpuCanvas: supportsGpuCanvas(),
    secureContext: isSecureContext,
    navigatorGpu: 'gpu' in navigator,
    drawableContent: (element as HTMLCanvasElement & { content?: string }).content === 'drawable',
    requestPaint: 'requestPaint' in canvasPrototype,
    getElementTransform: 'getElementTransform' in canvasPrototype,
    updateElementGeometry: 'updateElementGeometry' in canvasPrototype,
    getElementImageDefaultSize: 'getElementImageDefaultSize' in canvasPrototype,
    drawElementImageToTexture: !!queuePrototype && 'drawElementImageToTexture' in queuePrototype,
    legacyCopyElementImageToTexture: !!queuePrototype && 'copyElementImageToTexture' in queuePrototype,
    userAgent: navigator.userAgent,
    devicePixelRatio,
  };
}

interface CardElements {
  card: Card;
  node: HTMLDivElement;
  body: HTMLDivElement;
  name: HTMLDivElement;
  version: HTMLDivElement;
  image: HTMLImageElement;
  footer: HTMLDivElement;
  active: boolean;
  loaded: boolean;
  url: string;
  sourceTier: number;
  /** Composite count before this card's most recent DOM/source-content change. */
  changedAtFrame: number;
}

interface RetainedSource {
  url: string;
  records: Set<CardElements>;
  bytes: number;
  used: number;
}

// The benchmark camera stores screen translation; gpucanvas stores a world-space view origin.
const gpuCamera = (camera: Camera): Camera => ({
  x: -camera.x / camera.zoom,
  y: -camera.y / camera.zoom,
  zoom: camera.zoom,
});

export async function createGpuCanvasAdapter(
  host: HTMLElement,
  options: BenchOptions,
): Promise<Adapter> {
  const compatibility = probeGpuCanvasCompatibility();
  if (!compatibility.supportsGpuCanvas) {
    throw new Error(`gpucanvas GPU backend unsupported: ${JSON.stringify(compatibility)}`);
  }
  const params = new URLSearchParams(location.search);
  const retention = params.get('retention') ?? 'strict';
  if (retention !== 'strict' && retention !== 'budget') throw new Error(`Unknown source retention policy: ${retention}`);
  const atlasParam = params.get('atlasSize');
  const atlasSize = atlasParam === null ? 4096 : Number(atlasParam);
  if (!Number.isInteger(atlasSize) || atlasSize < 256 || atlasSize > 8192 || (atlasSize & (atlasSize - 1)) !== 0) {
    throw new Error('atlasSize must be a power of two from 256 through 8192');
  }
  const sourceBudgetBytes = 64 * 1024 * 1024;
  // Retain package atlas defaults unless the separate atlasSize experiment is requested.
  // Forced GPU mode rejects adapter/context failures instead of silently measuring DOM.
  const canvas: GpuCanvas = await createGpuCanvas(host, {
    mode: 'gpu',
    background: '#111820',
    ...(atlasParam === null ? {} : { atlasSize }),
    camera: gpuCamera(options.camera),
  });
  if (canvas.mode !== 'gpu') {
    canvas.destroy();
    throw new Error(`gpucanvas returned unexpected backend: ${canvas.mode}`);
  }

  let destroyed = false;
  let camera = { ...options.camera };
  let selected: string | null = null;
  let lod = options.lod(camera);
  let observedFrames = 0;
  let observationFrame = 0;
  let observationTicks = 0;
  let imageLoadEvents = 0;
  let imageErrorEvents = 0;
  let sourceAttachments = 0;
  let sourceDetachments = 0;
  let sourceReplacements = 0;
  let sourceCacheHits = 0;
  let sourceCacheMisses = 0;
  let retainedReturnHits = 0;
  let sourceEvictions = 0;
  let sourceClock = 0;
  const sourceCache = new Map<string, RetainedSource>();
  const cards = new Map<string, CardElements>();
  const surfaceCanvas = canvas.surface.querySelector('canvas');

  const readStats = (): GpuCanvasStats => canvas.getStats();
  // gpucanvas exposes a composite counter, not a render callback. Observe actual counter
  // increments after paint and scheduled updates; do not count the observation RAF as a draw.
  const observeDraws = () => {
    const frameCount = readStats().frames ?? 0;
    while (observedFrames < frameCount) {
      observedFrames++;
      options.onDraw();
    }
  };
  const observationTick = () => {
    observationFrame = 0;
    if (destroyed) return;
    observeDraws();
    if (--observationTicks > 0) observationFrame = requestAnimationFrame(observationTick);
  };
  const observeScheduledDraws = () => {
    // Two checks also cover MutationObserver scheduling after this synchronous update.
    observationTicks = 2;
    if (!observationFrame) observationFrame = requestAnimationFrame(observationTick);
  };
  surfaceCanvas?.addEventListener('paint', observeDraws);

  const styleChrome = (record: CardElements, frame: number) => {
    const border = selected === record.card.id ? 2 : 1;
    record.body.style.border = `${border}px solid ${border === 2 ? '#65b8ff' : '#3e4957'}`;
    record.name.style.display = lod === 2 ? 'none' : '';
    record.name.style.top = `${-border}px`;
    record.name.style.left = `${8 - border}px`;
    record.name.style.right = `${(lod === 0 ? 28 : 8) - border}px`;
    record.version.style.display = lod === 0 ? '' : 'none';
    record.version.style.top = `${-border}px`;
    record.version.style.right = `${8 - border}px`;
    record.image.style.left = `${1 - border}px`;
    record.image.style.top = `${28 - border}px`;
    record.footer.style.display = lod === 0 ? '' : 'none';
    record.footer.style.left = `${8 - border}px`;
    record.footer.style.top = `${174 - border}px`;
    record.changedAtFrame = frame;
  };

  // Until an image loads, reserve its requested width and the shared thumbnail aspect ratio.
  // Loaded dimensions replace this estimate. These are logical RGBA bytes, not browser RSS.
  const estimatedSourceBytes = (tier: number) => tier * Math.max(1, Math.round(tier * 134 / 238)) * 4;
  const untrackSource = (record: CardElements) => {
    const entry = sourceCache.get(record.url);
    if (!entry) return;
    entry.records.delete(record);
    if (entry.records.size === 0) sourceCache.delete(entry.url);
  };
  const detachSource = (record: CardElements, frame: number) => {
    untrackSource(record);
    record.url = '';
    record.loaded = false;
    record.sourceTier = 0;
    record.changedAtFrame = frame;
    record.image.removeAttribute('src');
    sourceDetachments++;
  };
  const trimSources = (frame: number) => {
    if (retention !== 'budget') return;
    let bytes = [...sourceCache.values()].reduce((sum, entry) => sum + entry.bytes, 0);
    if (bytes <= sourceBudgetBytes) return;
    const candidates = [...sourceCache.values()]
      .filter((entry) => ![...entry.records].some((record) => record.active))
      .sort((a, b) => a.used - b.used);
    for (const entry of candidates) {
      if (bytes <= sourceBudgetBytes) break;
      bytes -= entry.bytes;
      for (const record of [...entry.records]) detachSource(record, frame);
      sourceEvictions++;
    }
  };

  const syncSources = () => {
    const frame = readStats().frames ?? 0;
    const nextLod = options.lod(camera);
    const changedLod = nextLod !== lod;
    lod = nextLod;
    const tier = options.tier(camera);
    for (const record of cards.values()) {
      if (changedLod) styleChrome(record, frame);
      const active = options.visible(record.card, camera);
      const wasActive = record.active;
      record.active = active;
      if (retention === 'budget') {
        // Only visibility/tier changes acquire a source. An offscreen card keeps its existing
        // img.src and browser-managed decoded source until unpinned LRU pressure evicts it.
        if (!active) continue;
        const url = options.assetUrl(record.card.asset, tier);
        if (record.url === url) {
          const entry = sourceCache.get(url);
          if (entry) entry.used = ++sourceClock;
          if (!wasActive) {
            sourceCacheHits++;
            retainedReturnHits++;
            // A source may have loaded while its page was parked: require a later composite.
            record.changedAtFrame = frame;
          }
          continue;
        }
        if (record.url) {
          untrackSource(record);
          sourceReplacements++;
        }
        let entry = sourceCache.get(url);
        if (entry) sourceCacheHits++;
        else {
          sourceCacheMisses++;
          entry = { url, records: new Set(), bytes: estimatedSourceBytes(tier), used: 0 };
          sourceCache.set(url, entry);
        }
        entry.records.add(record);
        entry.used = ++sourceClock;
        record.url = url;
        record.sourceTier = tier;
        record.loaded = false;
        record.changedAtFrame = frame;
        sourceAttachments++;
        record.image.src = url;
        continue;
      }
      // Original strict policy remains the default for comparison with earlier receipts.
      const url = active ? options.assetUrl(record.card.asset, tier) : '';
      if (record.url === url) continue;
      if (record.url && url) sourceReplacements++;
      record.url = url;
      record.sourceTier = url ? tier : 0;
      record.loaded = false;
      record.changedAtFrame = frame;
      if (url) {
        sourceAttachments++;
        record.image.src = url;
      } else {
        sourceDetachments++;
        record.image.removeAttribute('src');
      }
    }
    trimSources(frame);
    observeScheduledDraws();
  };

  try {
    for (const card of options.cards) {
      const node = document.createElement('div');
      const body = document.createElement('div');
      Object.assign(body.style, {
        position: 'relative', width: '100%', height: '100%', boxSizing: 'border-box',
        overflow: 'hidden', borderRadius: '8px', background: '#20262d',
        color: '#e5eaf1', font: '12px Arial, sans-serif',
      });
      const name = document.createElement('div');
      name.textContent = card.label;
      Object.assign(name.style, {
        position: 'absolute', height: '28px', lineHeight: '28px',
        overflow: 'hidden', whiteSpace: 'nowrap', textOverflow: 'ellipsis',
      });
      const version = document.createElement('div');
      version.textContent = 'v1';
      Object.assign(version.style, {
        position: 'absolute', height: '28px', lineHeight: '28px', color: '#a3afbd',
      });
      const image = document.createElement('img');
      image.alt = '';
      image.decoding = 'async';
      image.draggable = false;
      Object.assign(image.style, {
        position: 'absolute', width: `${card.width - 2}px`, height: '134px',
        objectFit: 'cover', display: 'block',
      });
      const footer = document.createElement('div');
      footer.textContent = card.status;
      Object.assign(footer.style, {
        position: 'absolute', color: '#a3afbd', height: '16px', lineHeight: '16px',
      });
      const record: CardElements = {
        card: { ...card }, node, body, name, version, image, footer,
        active: false, loaded: false, url: '', sourceTier: 0, changedAtFrame: 0,
      };
      image.onload = () => {
        if (destroyed || !record.url || !image.complete || image.naturalWidth === 0) return;
        if (image.currentSrc !== record.url) return;
        imageLoadEvents++;
        record.loaded = true;
        record.changedAtFrame = readStats().frames ?? 0;
        const cached = sourceCache.get(record.url);
        if (cached) cached.bytes = image.naturalWidth * image.naturalHeight * 4;
        trimSources(record.changedAtFrame);
        observeScheduledDraws();
      };
      image.onerror = () => {
        if (destroyed || !record.url) return;
        imageErrorEvents++;
        record.loaded = false;
        observeScheduledDraws();
      };
      body.append(name, version, image, footer);
      node.append(body);
      cards.set(card.id, record);
      styleChrome(record, 0);
      canvas.addNode(node, card);
    }
    syncSources();
  } catch (error) {
    surfaceCanvas?.removeEventListener('paint', observeDraws);
    cancelAnimationFrame(observationFrame);
    canvas.destroy();
    throw error;
  }

  // Opt-in correctness instrumentation for the separate, untimed live-video probe. A fresh
  // snapshot makes a page drawable, so callers must capture displayed pixels before using it.
  const videoProbe = {
    node(id: string) {
      const record = cards.get(id);
      if (!record) throw new Error(`Unknown gpucanvas card: ${id}`);
      return record.node;
    },
    image(id: string) {
      const record = cards.get(id);
      if (!record) throw new Error(`Unknown gpucanvas card: ${id}`);
      return record.image;
    },
    screenRect(id: string) {
      const record = cards.get(id);
      if (!record) throw new Error(`Unknown gpucanvas card: ${id}`);
      const origin = canvas.worldToScreen(record.card);
      const surface = canvas.surface.getBoundingClientRect();
      const zoom = canvas.getCamera().zoom;
      return {
        x: surface.x + origin.x, y: surface.y + origin.y,
        width: record.card.width * zoom, height: record.card.height * zoom, zoom,
      };
    },
    snapshot(id: string) {
      return canvas.snapshot(this.node(id), { scale: 1 });
    },
    stats: readStats,
  };
  const probeGlobal = globalThis as unknown as { __gpucanvasVideoProbe?: typeof videoProbe };
  if (new URLSearchParams(location.search).get('videoProbe') === '1') {
    probeGlobal.__gpucanvasVideoProbe = videoProbe;
  }

  return {
    setCamera(next) {
      camera = { ...next };
      canvas.setCamera(gpuCamera(camera));
      syncSources();
    },
    setSelected(id) {
      if (id === selected) return;
      const previous = selected;
      selected = id;
      const frame = readStats().frames ?? 0;
      for (const changed of [previous, id]) {
        const record = changed === null ? undefined : cards.get(changed);
        if (record) styleChrome(record, frame);
      }
      observeScheduledDraws();
    },
    moveCard(id, x, y) {
      const record = cards.get(id);
      if (!record) return;
      record.card.x = x;
      record.card.y = y;
      canvas.updateNode(record.node, record.card);
      syncSources();
    },
    stats() {
      observeDraws();
      const active = [...cards.values()].filter((record) => record.active);
      const retained = [...cards.values()].filter((record) => record.url);
      const uniqueSources = new Map<string, { bytes: number; pinned: boolean }>();
      for (const record of retained) {
        const previous = uniqueSources.get(record.url);
        const bytes = record.loaded
          ? record.image.naturalWidth * record.image.naturalHeight * 4
          : estimatedSourceBytes(record.sourceTier);
        uniqueSources.set(record.url, {
          bytes: Math.max(previous?.bytes ?? 0, sourceCache.get(record.url)?.bytes ?? bytes),
          pinned: record.active || !!previous?.pinned,
        });
      }
      const sourceBytes = [...uniqueSources.values()].reduce((sum, source) => sum + source.bytes, 0);
      const pinnedBytes = [...uniqueSources.values()].filter((source) => source.pinned).reduce((sum, source) => sum + source.bytes, 0);
      return {
        backend: 'gpucanvas-webgpu-html-in-canvas',
        compatibility,
        gpucanvas: readStats(),
        nominalAtlasBytes: atlasSize * atlasSize * 5 * 4,
        nominalAtlasDimensions: [atlasSize, atlasSize, 5],
        atlasMemoryMeaning: 'nominal RGBA8 texture dimensions; physical resident GPU memory is not measured',
        mountedCards: cards.size,
        domElementCount: host.querySelectorAll('*').length,
        activeImageElements: active.length,
        loadedActiveImageElements: active.filter((record) => record.loaded).length,
        uniqueActiveSourceUrls: new Set(active.map((record) => record.url).filter(Boolean)).size,
        imageLoadEvents,
        imageErrorEvents,
        sourceAttachments,
        sourceDetachments,
        sourceReplacements,
        sourceRetentionPolicy: retention,
        sourceBudgetBytes: retention === 'budget' ? sourceBudgetBytes : null,
        retainedSourceNominalBytes: sourceBytes,
        pinnedSourceNominalBytes: pinnedBytes,
        sourceBudgetOverflowBytes: retention === 'budget' ? Math.max(0, sourceBytes - sourceBudgetBytes) : 0,
        retainedImageElements: retained.length,
        retainedOffscreenImageElements: retained.filter((record) => !record.active).length,
        uniqueRetainedSourceUrls: uniqueSources.size,
        sourceCacheHits,
        sourceCacheMisses,
        retainedReturnHits,
        sourceEvictions,
        sourceMemoryMeaning: 'unique attached source RGBA estimates; requested dimensions until loaded; browser decoded residency/caches are not measured; excludes the atlas',
        sourceAdmission: retention === 'budget'
          ? 'all card DOM retained; visible sources pinned; offscreen img.src retained until 64MiB nominal LRU pressure; tier replacement releases the previous source'
          : 'all card DOM retained; image src attached only within shared visible+overscan region',
        drawCounter: 'gpucanvas composite counter observed after paint/RAF and stats; not GPU presentation timestamps',
        readiness: 'loaded sources plus later composite and global pending=0; package has no per-node snapshot-generation API',
      };
    },
    readyFraction() {
      const active = [...cards.values()].filter((record) => record.active);
      if (active.length === 0) return 1;
      const stats = readStats();
      // Source completion alone can precede the atlas update. Require the backend to finish
      // its visible raster queue and composite after each source/chrome content change.
      if (stats.pending !== 0) return 0;
      const frames = stats.frames ?? 0;
      return active.filter((record) => record.loaded && frames > record.changedAtFrame).length / active.length;
    },
    destroy() {
      destroyed = true;
      if (probeGlobal.__gpucanvasVideoProbe === videoProbe) delete probeGlobal.__gpucanvasVideoProbe;
      cancelAnimationFrame(observationFrame);
      surfaceCanvas?.removeEventListener('paint', observeDraws);
      for (const record of cards.values()) {
        record.image.onload = null;
        record.image.onerror = null;
        record.image.removeAttribute('src');
      }
      cards.clear();
      sourceCache.clear();
      canvas.destroy();
    },
  };
}
