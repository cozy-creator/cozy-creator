import type { Adapter, BenchOptions, Camera, Card } from './types';
import { makeCardContent, updateCardContent } from './card-dom';

/** A culled DOM baseline: one transformed world layer, with only visible cards mounted. */
export async function createDomAdapter(host: HTMLElement, options: BenchOptions): Promise<Adapter> {
  const cards = options.cards.map(card => ({ ...card }));
  const byId = new Map(cards.map(card => [card.id, card]));
  const mounted = new Map<string, HTMLElement>();
  const requestedSources = new Set<string>();
  const world = document.createElement('div');
  Object.assign(world.style, {
    position: 'absolute', left: '0', top: '0', transformOrigin: '0 0',
    willChange: 'transform',
  });
  host.appendChild(world);

  let camera = { ...options.camera };
  let tier = options.tier(camera);
  let lod = options.lod(camera);
  let selected: string | null = null;
  let draws = 0;
  let destroyed = false;

  function drawn() {
    if (destroyed) return;
    draws++;
    options.onDraw();
  }

  function url(card: Card) {
    const source = options.assetUrl(card.asset, tier);
    requestedSources.add(source);
    return source;
  }

  function place(element: HTMLElement, card: Card) {
    element.style.position = 'absolute';
    element.style.left = `${card.x}px`;
    element.style.top = `${card.y}px`;
    element.style.width = `${card.width}px`;
    element.style.height = `${card.height}px`;
    element.dataset.cardId = card.id;
  }

  function syncVisible(refreshContent = false) {
    let changed = false;
    for (const card of cards) {
      const element = mounted.get(card.id);
      if (!options.visible(card, camera)) {
        if (element) {
          element.remove();
          mounted.delete(card.id);
          changed = true;
        }
        continue;
      }
      if (!element) {
        const source = url(card);
        const next = makeCardContent(card, source, lod);
        place(next, card);
        if (selected === card.id) updateCardContent(next, card, source, lod, true);
        mounted.set(card.id, next);
        world.appendChild(next);
        changed = true;
      } else if (refreshContent) {
        updateCardContent(element, card, url(card), lod, selected === card.id);
        changed = true;
      }
    }
    return changed;
  }

  function transform() {
    world.style.transform = `translate3d(${camera.x}px, ${camera.y}px, 0) scale(${camera.zoom})`;
  }

  // Image load causes a real browser paint, even when no scene mutation is needed.
  const loaded = (event: Event) => {
    if (event.target instanceof HTMLImageElement) drawn();
  };
  world.addEventListener('load', loaded, true);
  transform();
  syncVisible();
  drawn();

  return {
    setCamera(next) {
      if (destroyed || (next.x === camera.x && next.y === camera.y && next.zoom === camera.zoom)) return;
      camera = { ...next };
      const nextTier = options.tier(camera);
      const nextLod = options.lod(camera);
      const refresh = nextTier !== tier || nextLod !== lod;
      tier = nextTier;
      lod = nextLod;
      transform();
      syncVisible(refresh);
      drawn();
    },
    setSelected(id) {
      if (destroyed || selected === id) return;
      const previous = selected;
      selected = id;
      let changed = false;
      for (const cardId of [previous, id]) {
        if (cardId === null) continue;
        const element = mounted.get(cardId);
        const card = byId.get(cardId);
        if (element && card) {
          updateCardContent(element, card, url(card), lod, selected === cardId);
          changed = true;
        }
      }
      if (changed) drawn();
    },
    moveCard(id, x, y) {
      if (destroyed) return;
      const card = byId.get(id);
      if (!card || (card.x === x && card.y === y)) return;
      card.x = x;
      card.y = y;
      const element = mounted.get(id);
      if (element) place(element, card);
      const changed = syncVisible();
      if (element || changed) drawn();
    },
    stats() {
      const images = [...world.querySelectorAll('img')];
      return {
        backend: 'DOM / CSS compositor transforms',
        drawProxy: 'scene DOM mutations and image load events',
        draws,
        sceneCards: cards.length,
        mountedCards: mounted.size,
        domNodes: host.querySelectorAll('*').length,
        imageElements: images.length,
        loadedImageElements: images.filter(image => image.complete && image.naturalWidth > 0).length,
        distinctRequestedSourceUrls: requestedSources.size,
        tier,
        lod,
      };
    },
    readyFraction() {
      if (!mounted.size) return 1;
      let ready = 0;
      for (const element of mounted.values()) {
        const image = element.querySelector('img');
        if (image?.complete && image.naturalWidth > 0) ready++;
      }
      return ready / mounted.size;
    },
    destroy() {
      if (destroyed) return;
      destroyed = true;
      world.removeEventListener('load', loaded, true);
      world.remove();
      mounted.clear();
      requestedSources.clear();
    },
  };
}
