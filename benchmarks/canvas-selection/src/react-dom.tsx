import * as React from 'react';
import { createRoot } from 'react-dom/client';
import type { Adapter, BenchOptions, Camera, Card } from './types';

interface MediaProps {
  card: Card;
  source: string;
  lod: 0 | 1 | 2;
  selected: boolean;
  appearance?: BenchOptions['appearance'];
  registerImage(id: string, image: HTMLImageElement | null): void;
  imageLoaded(): void;
  imageFailed(): void;
}

interface CreatorSurfaceProps {
  card: Card;
  source: string;
  lod: 0 | 1 | 2;
  selected: boolean;
  imageRef: React.Ref<HTMLImageElement>;
  imageLoaded(): void;
  imageFailed?(): void;
  positioned?: boolean;
}

/** Shared Creator JSX for both React controls; importing this creates no adapter or scene. */
export function CreatorCardSurface({ card, source, lod, selected, imageRef, imageLoaded, imageFailed, positioned = false }: CreatorSurfaceProps) {
    const mediaHeight = Math.round(card.width * 9 / 16);
    const dot = card.state === 'ready' ? '#55c98a' : card.state === 'rendering' ? '#66b6f4' : card.state === 'error' ? '#e66a64' : '#a0a3a8';
    return <div data-bench-card={card.id} data-card-id={card.id} data-lod={lod} data-state={card.state ?? 'idle'}
      className={`bench-card creator-card${selected ? ' bench-selected' : ''}`}
      style={{ left: positioned ? card.x : undefined, top: positioned ? card.y : undefined, width: card.width, height: 32 + mediaHeight + 40, background: 'transparent', border: 0, borderRadius: 0, outline: 'none', overflow: 'visible' }}>
      <div className="creator-head" style={{ height: 32, display: 'flex', alignItems: 'center', gap: 6, fontFamily: 'monospace', visibility: lod === 2 ? 'hidden' : undefined }}>
        <span className={`creator-dot ${card.state ?? 'idle'}`} style={{ width: 8, height: 8, borderRadius: '50%', background: dot, flex: '0 0 8px', position: 'relative', top: -1 }} />
        <span className="creator-name" style={{ fontSize: 21, fontWeight: 700, color: '#e7e7e7', whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis', minWidth: 0 }}>{card.label}</span>
        <span className="creator-version" style={{ fontSize: 14, color: '#aaa', flexShrink: 0, visibility: lod === 0 ? undefined : 'hidden' }}>v{card.version || 1}</span>
        <button type="button" className="creator-delete" data-demo-delete={card.id} aria-label={`Delete ${card.label}`} style={{ marginLeft: 'auto', width: 24, height: 24, padding: 4, background: 'none', border: 0, color: '#999', flexShrink: 0, visibility: lod === 0 ? undefined : 'hidden' }}>
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true"><path d="M3 6h18M9 6V4h6v2M6 6l1 14h10l1-14M10 10v6M14 10v6" /></svg>
        </button>
      </div>
      <div className="creator-image" style={{ position: 'relative', width: card.width, height: mediaHeight, boxSizing: 'border-box', border: '1px solid #414248', boxShadow: '0 4px 12px #0005', outline: selected ? '2px solid #ff9a5e' : undefined, outlineOffset: -2 }}>
        <img ref={imageRef} src={source} onLoad={imageLoaded} onError={imageFailed} alt="" draggable={false} style={{ width: '100%', height: '100%', display: 'block', objectFit: 'cover' }} />
        {card.kind === 'video' && <svg className="creator-play" viewBox="0 0 32 32" width="32" height="32" aria-hidden="true" style={{ position: 'absolute', left: '50%', top: '50%', transform: 'translate(-50%,-50%)', fill: '#ffffffe6', pointerEvents: 'none', filter: 'drop-shadow(0 1px 2px #0008)' }}><path d="M8 4L28 16L8 28Z" /></svg>}
      </div>
      <div className="creator-footer" style={{ position: 'relative', height: 40, fontFamily: 'monospace', fontSize: 14, lineHeight: '16px', color: '#aaa', visibility: lod === 0 ? undefined : 'hidden' }}>
        <span className="creator-tool" style={{ position: 'absolute', left: 0, top: 8, maxWidth: 'calc(100% - 46px)', whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}>{card.tool || 'Qwen Image 2.1 · Image'}</span>
        <span className="creator-ratio" style={{ position: 'absolute', right: 0, top: 8 }}>16:9</span>
        {(card.state === 'rendering' || card.state === 'error') && <span className="creator-status" style={{ position: 'absolute', left: 0, top: 24, color: dot }}>{card.status}</span>}
      </div>
    </div>;
}

// Same benchmark JSX/CSS as the React Flow control, with only world positioning added.
const MediaCard = React.memo(function MediaCard({ card, source, lod, selected, appearance, registerImage, imageLoaded, imageFailed }: MediaProps) {
  const imageRef = React.useCallback((image: HTMLImageElement | null) => registerImage(card.id, image), [card.id, registerImage, source]);
  if (appearance === 'creator') return <CreatorCardSurface card={card} source={source} lod={lod} selected={selected} imageRef={imageRef} imageLoaded={imageLoaded} imageFailed={imageFailed} positioned />;
  return <div data-bench-card={card.id} data-card-id={card.id} data-lod={lod}
    className={`bench-card${selected ? ' bench-selected' : ''}`}
    style={{ left: card.x, top: card.y, width: card.width, height: card.height }}>
    <div className="bench-header"><span className="bench-name">{card.label}</span><span className="bench-version">v1</span></div>
    <img className="bench-image" ref={imageRef} src={source} onLoad={imageLoaded} onError={imageFailed} alt="" draggable={false} />
    <div className="bench-status">{card.status}</div>
  </div>;
});

/** Visible React cards without a graph store or node wrappers; camera transforms bypass React. */
export async function createReactDomAdapter(host: HTMLElement, options: BenchOptions): Promise<Adapter> {
  const world = document.createElement('div');
  Object.assign(world.style, { position: 'absolute', left: '0', top: '0', transformOrigin: '0 0', willChange: 'transform' });
  host.appendChild(world);
  const root = createRoot(world);
  const normalizeCard = (card: Card): Card => ({ ...card, height: options.appearance === 'creator' ? 32 + Math.round(card.width * 9 / 16) + 40 : card.height });
  const cards = options.cards.map(normalizeCard);
  const indices = new Map(cards.map((card, index) => [card.id, index]));
  const images = new Map<string, HTMLImageElement>();
  const requestedSources = new Set<string>();
  let camera = { ...options.camera };
  let tier = options.tier(camera), lod = options.lod(camera);
  let selection = new Set<string>();
  let visibleCards: Card[] = [];
  let destroyed = false, draws = 0, commits = 0, renderRequests = 0, viewportMutations = 0, imageLoads = 0, imageErrors = 0;
  let requestedGeneration = 0, committedGeneration = 0, paintedGeneration = 0;
  let paintEpoch = 0, paintedEpoch = -1, firstFrame = 0, secondFrame = 0;
  let initialized!: () => void;
  const initialCommit = new Promise<void>(resolve => { initialized = resolve; });

  function drawn() {
    if (destroyed) return;
    draws++;
    options.onDraw();
  }

  function schedulePaintOpportunity() {
    if (destroyed || firstFrame || secondFrame) return;
    firstFrame = requestAnimationFrame(() => {
      firstFrame = 0;
      const epoch = paintEpoch, generation = committedGeneration;
      secondFrame = requestAnimationFrame(() => {
        secondFrame = 0;
        if (destroyed) return;
        if (epoch === paintEpoch && generation === committedGeneration && generation === requestedGeneration) {
          paintedEpoch = epoch;
          paintedGeneration = generation;
        } else schedulePaintOpportunity();
      });
    });
  }

  function invalidatePaint() {
    paintEpoch++;
    schedulePaintOpportunity();
  }

  const registerImage = (id: string, image: HTMLImageElement | null) => {
    if (image) { images.set(id, image); requestedSources.add(image.src); }
    else images.delete(id);
  };
  const imageLoaded = () => { if (!destroyed) { imageLoads++; invalidatePaint(); drawn(); } };
  const imageFailed = () => { if (!destroyed) imageErrors++; };

  function Scene({ shown, atTier, atLod, chosen, generation }: {
    shown: Card[]; atTier: number; atLod: 0 | 1 | 2; chosen: ReadonlySet<string>; generation: number;
  }) {
    React.useLayoutEffect(() => {
      if (destroyed) return;
      committedGeneration = generation;
      commits++;
      invalidatePaint();
      drawn();
      initialized();
    }, [generation]);
    return <>{shown.map(card => <MediaCard key={card.id} card={card}
      source={options.assetUrl(card.asset, atTier)} lod={atLod} selected={chosen.has(card.id)} appearance={options.appearance}
      registerImage={registerImage} imageLoaded={imageLoaded} imageFailed={imageFailed} />)}</>;
  }

  function renderCards() {
    if (destroyed) return;
    requestedGeneration++;
    renderRequests++;
    root.render(<Scene shown={visibleCards} atTier={tier} atLod={lod} chosen={selection} generation={requestedGeneration} />);
  }

  function syncVisible() {
    const next = cards.filter(card => options.visible(card, camera));
    const changed = next.length !== visibleCards.length || next.some((card, index) => card !== visibleCards[index]);
    if (changed) visibleCards = next;
    return changed;
  }

  function transform() {
    world.style.transform = `translate3d(${camera.x}px, ${camera.y}px, 0) scale(${camera.zoom})`;
    viewportMutations++;
    invalidatePaint();
    drawn();
  }

  function setSelection(ids: string[]) {
    if (destroyed) return;
    const next = new Set(ids.filter(id => indices.has(id)));
    if (next.size === selection.size && [...next].every(id => selection.has(id))) return;
    const previous = selection;
    selection = next;
    if (visibleCards.some(card => previous.has(card.id) !== next.has(card.id))) renderCards();
  }

  function moveCards(moves: { id: string; x: number; y: number }[]) {
    if (destroyed) return;
    let changed = false;
    for (const { id, x, y } of moves) {
      const index = indices.get(id);
      if (index === undefined) continue;
      const previous = cards[index];
      if (previous.x === x && previous.y === y) continue;
      cards[index] = { ...previous, x, y };
      changed = true;
    }
    if (changed && syncVisible()) renderCards();
  }

  transform();
  syncVisible();
  renderCards();
  await initialCommit;

  return {
    setCamera(next) {
      if (destroyed || (next.x === camera.x && next.y === camera.y && next.zoom === camera.zoom)) return;
      camera = { ...next };
      const nextTier = options.tier(camera), nextLod = options.lod(camera);
      const changedContent = nextTier !== tier || nextLod !== lod;
      tier = nextTier;
      lod = nextLod;
      transform();
      if (syncVisible() || changedContent) renderCards();
    },
    setSelected(id) { setSelection(id === null ? [] : [id]); },
    setSelection,
    moveCard(id, x, y) { moveCards([{ id, x, y }]); },
    moveCards,
    updateCard(id, patch) {
      if (destroyed) return;
      const index = indices.get(id);
      if (index === undefined) return;
      cards[index] = normalizeCard({ ...cards[index], ...patch, id });
      if (syncVisible()) renderCards();
    },
    addCard(card) {
      if (destroyed || indices.has(card.id)) return;
      indices.set(card.id, cards.length);
      cards.push(normalizeCard(card));
      if (syncVisible()) renderCards();
    },
    removeCard(id) {
      if (destroyed) return;
      const index = indices.get(id);
      if (index === undefined) return;
      cards.splice(index, 1);
      indices.delete(id);
      for (let at = index; at < cards.length; at++) indices.set(cards[at].id, at);
      selection = new Set([...selection].filter(chosen => chosen !== id));
      if (syncVisible()) renderCards();
    },
    resize(width, height) {
      if (destroyed) return;
      options.width = width;
      options.height = height;
      const nextTier = options.tier(camera), nextLod = options.lod(camera);
      const changed = nextTier !== tier || nextLod !== lod;
      tier = nextTier;
      lod = nextLod;
      if (syncVisible() || changed) renderCards();
      invalidatePaint();
      drawn();
    },
    readyFraction() {
      if (!visibleCards.length) return 1;
      if (requestedGeneration !== committedGeneration || paintedGeneration !== requestedGeneration || paintedEpoch !== paintEpoch) return 0;
      let loaded = 0;
      for (const card of visibleCards) {
        const image = images.get(card.id);
        if (image?.isConnected && image.complete && image.naturalWidth > 0 && (image.currentSrc || image.src) === options.assetUrl(card.asset, tier)) loaded++;
      }
      return loaded / visibleCards.length;
    },
    stats() {
      return {
        backend: 'Virtualized React DOM / CSS compositor transforms; no XYFlow',
        appearance: options.appearance ?? 'benchmark', selection: [...selection],
        drawProxy: 'React commits, viewport DOM mutations and image load events; not GPU presentation',
        paintReadinessProxy: 'two RAF callbacks after latest commit/load/transform; external screenshot pixel gate still required',
        sceneCards: cards.length, visibleCards: visibleCards.length, mountedCards: images.size,
        domNodes: host.querySelectorAll('*').length, imageElements: images.size,
        loadedImageElements: [...images.values()].filter(image => image.complete && image.naturalWidth > 0).length,
        distinctRequestedSourceUrls: requestedSources.size, cacheOwnership: 'browser-managed native images; no app decoded-image LRU',
        draws, commits, rootRenderRequests: renderRequests, viewportMutations, imageLoads, imageErrors,
        requestedGeneration, committedGeneration, paintedGeneration, paintEpoch, paintedEpoch,
        tier, lod, sharedVisibleOverscan: true, cameraTransformsBypassReact: true,
      };
    },
    destroy() {
      if (destroyed) return;
      destroyed = true;
      if (firstFrame) cancelAnimationFrame(firstFrame);
      if (secondFrame) cancelAnimationFrame(secondFrame);
      firstFrame = secondFrame = 0;
      root.unmount();
      world.remove();
      images.clear();
      requestedSources.clear();
      visibleCards = [];
    },
  };
}
