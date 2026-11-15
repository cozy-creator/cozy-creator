import * as React from 'react';
import { createRoot } from 'react-dom/client';
import type { Adapter, BenchOptions, Camera, Card } from './types';

interface MediaProps {
  card: Card;
  source: string;
  lod: 0 | 1 | 2;
  selected: boolean;
  registerImage(id: string, image: HTMLImageElement | null): void;
  imageLoaded(): void;
  imageFailed(): void;
}

// Same media-card JSX/CSS as the React Flow control, with only world positioning added.
const MediaCard = React.memo(function MediaCard({ card, source, lod, selected, registerImage, imageLoaded, imageFailed }: MediaProps) {
  const imageRef = React.useCallback((image: HTMLImageElement | null) => registerImage(card.id, image), [card.id, registerImage, source]);
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
  const cards = options.cards.map(card => ({ ...card }));
  const indices = new Map(cards.map((card, index) => [card.id, index]));
  const images = new Map<string, HTMLImageElement>();
  const requestedSources = new Set<string>();
  let camera = { ...options.camera };
  let tier = options.tier(camera), lod = options.lod(camera);
  let selected: string | null = null;
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
    shown: Card[]; atTier: number; atLod: 0 | 1 | 2; chosen: string | null; generation: number;
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
      source={options.assetUrl(card.asset, atTier)} lod={atLod} selected={card.id === chosen}
      registerImage={registerImage} imageLoaded={imageLoaded} imageFailed={imageFailed} />)}</>;
  }

  function renderCards() {
    if (destroyed) return;
    requestedGeneration++;
    renderRequests++;
    root.render(<Scene shown={visibleCards} atTier={tier} atLod={lod} chosen={selected} generation={requestedGeneration} />);
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
    setSelected(id) {
      if (destroyed || selected === id) return;
      const previous = selected;
      selected = id;
      if (visibleCards.some(card => card.id === previous || card.id === id)) renderCards();
    },
    moveCard(id, x, y) {
      if (destroyed) return;
      const index = indices.get(id);
      if (index === undefined) return;
      const previous = cards[index];
      if (previous.x === x && previous.y === y) return;
      cards[index] = { ...previous, x, y };
      if (syncVisible()) renderCards();
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
