import * as React from 'react';
import { createRoot } from 'react-dom/client';
import { ReactFlow, type Node, type NodeProps, type ReactFlowInstance } from '@xyflow/react';
import '@xyflow/react/dist/style.css';
import type { Adapter, BenchOptions, Camera, Card } from './types';

type MediaData = {
  card: Card;
  source: string;
  lod: 0 | 1 | 2;
  selected: boolean;
  registerImage(id: string, image: HTMLImageElement | null): void;
  imageLoaded(): void;
};
type MediaNode = Node<MediaData, 'media'>;

function useMediaCardElement({ card, source, lod, selected, registerImage, imageLoaded }: MediaData) {
  const imageRef = React.useCallback((image: HTMLImageElement | null) => registerImage(card.id, image), [card.id, registerImage, source]);
  return <div data-bench-card={card.id} data-card-id={card.id} data-lod={lod}
    className={`bench-card${selected ? ' bench-selected' : ''}`}
    style={{ width: card.width, height: card.height }}>
    <div className="bench-header"><span className="bench-name">{card.label}</span><span className="bench-version">v1</span></div>
    <img className="bench-image" ref={imageRef} src={source} onLoad={imageLoaded} alt="" draggable={false} />
    <div className="bench-status">{card.status}</div>
  </div>;
}

const MediaCard = React.memo(function MediaCard({ data }: NodeProps<MediaNode>) {
  return useMediaCardElement(data);
});

const nodeTypes = { media: MediaCard };
const edges: [] = [];
const proOptions = { hideAttribution: true };
const OVERSCAN = 64;

type StoredMediaData = Pick<MediaData, 'card' | 'registerImage' | 'imageLoaded'>;
type StoredMediaNode = Node<StoredMediaData, 'media'>;
interface DisplaySettings {
  tier: number;
  lod: 0 | 1 | 2;
  selected: string | null;
  assetUrl: BenchOptions['assetUrl'];
  generation: number;
}
const DisplayContext = React.createContext<DisplaySettings | null>(null);
const MountedMediaCard = React.memo(function MountedMediaCard({ data }: NodeProps<StoredMediaNode>) {
  const display = React.useContext(DisplayContext);
  if (!display) throw new Error('React Flow card is missing its display provider');
  return useMediaCardElement({
    ...data, source: display.assetUrl(data.card.asset, display.tier), lod: display.lod,
    selected: data.card.id === display.selected,
  });
});
const mountedNodeTypes = { media: MountedMediaCard };

function applyViewportLayerHint(view: HTMLElement, enabled: boolean) {
  if (!enabled) return false;
  const viewport = view.querySelector<HTMLElement>('.react-flow__viewport');
  if (!viewport) return false;
  viewport.style.willChange = 'transform';
  return true;
}

/** Production-style uncontrolled store by default; retain the original controlled comparison. */
export async function createReactFlowAdapter(host: HTMLElement, options: BenchOptions): Promise<Adapter> {
  if (new URLSearchParams(location.search).get('rfMode') === 'controlled') return createControlledReactFlowAdapter(host, options);
  return createUncontrolledReactFlowAdapter(host, options);
}

async function createUncontrolledReactFlowAdapter(host: HTMLElement, options: BenchOptions): Promise<Adapter> {
  const view = document.createElement('div');
  Object.assign(view.style, {
    position: 'absolute', left: `${-OVERSCAN}px`, top: `${-OVERSCAN}px`,
    width: `${options.width + OVERSCAN * 2}px`, height: `${options.height + OVERSCAN * 2}px`,
    pointerEvents: 'none',
  });
  host.appendChild(view);
  const root = createRoot(view);
  const cards = new Map(options.cards.map(card => [card.id, { ...card }]));
  const images = new Map<string, HTMLImageElement>();
  const requestedSources = new Set<string>();
  const layerHintRequested = new URLSearchParams(location.search).get('rfLayer') === '1';
  const explicitEmptyHandles = new URLSearchParams(location.search).get('rfHandles') !== 'unknown';
  let layerHintApplied = false;
  let camera = { ...options.camera }, tier = options.tier(camera), lod = options.lod(camera);
  let selected: string | null = null;
  let destroyed = false, draws = 0, displayCommits = 0, displayUpdates = 0, movedNodeUpdates = 0;
  let displayGeneration = 0, committedDisplayGeneration = -1;
  let flow: ReactFlowInstance<StoredMediaNode> | undefined;
  let updateDisplay: (display: DisplaySettings) => void = () => {};
  let initialized!: () => void;
  const ready = new Promise<void>(resolve => { initialized = resolve; });

  function drawn() {
    if (destroyed) return;
    draws++;
    options.onDraw();
  }
  const registerImage = (id: string, image: HTMLImageElement | null) => {
    if (image) { images.set(id, image); requestedSources.add(image.src); }
    else images.delete(id);
  };
  const initialNodes: StoredMediaNode[] = [...cards.values()].map(card => ({
    id: card.id, type: 'media', position: { x: card.x, y: card.y },
    width: card.width, height: card.height, initialWidth: card.width, initialHeight: card.height,
    // Known card bounds and absent ports permit culling before any offscreen DOM mounts.
    measured: { width: card.width, height: card.height },
    handles: explicitEmptyHandles ? [] : undefined,
    style: { width: card.width, height: card.height },
    data: { card, registerImage, imageLoaded: drawn },
  }));
  const viewport = (at: Camera) => ({ x: at.x + OVERSCAN, y: at.y + OVERSCAN, zoom: at.zoom });
  const initialDisplay: DisplaySettings = { tier, lod, selected, assetUrl: options.assetUrl, generation: 0 };

  // This exact element is retained across display-band changes. Context reaches mounted
  // cards without recreating the Flow element or copying every document node's data.
  const flowElement = <ReactFlow<StoredMediaNode>
    defaultNodes={initialNodes}
    defaultEdges={edges}
    nodeTypes={mountedNodeTypes}
    defaultViewport={viewport(camera)}
    width={options.width + OVERSCAN * 2}
    height={options.height + OVERSCAN * 2}
    minZoom={0.01}
    maxZoom={8}
    onlyRenderVisibleElements
    nodesDraggable={false}
    nodesConnectable={false}
    nodesFocusable={false}
    edgesFocusable={false}
    elementsSelectable={false}
    selectionOnDrag={false}
    selectNodesOnDrag={false}
    panOnDrag={false}
    panOnScroll={false}
    zoomOnScroll={false}
    zoomOnPinch={false}
    zoomOnDoubleClick={false}
    autoPanOnNodeDrag={false}
    autoPanOnNodeFocus={false}
    preventScrolling={false}
    deleteKeyCode={null}
    selectionKeyCode={null}
    multiSelectionKeyCode={null}
    panActivationKeyCode={null}
    zoomActivationKeyCode={null}
    disableKeyboardA11y
    proOptions={proOptions}
    onInit={instance => { flow = instance; layerHintApplied = applyViewportLayerHint(view, layerHintRequested); initialized(); }}
  />;

  function Scene() {
    const [display, setDisplay] = React.useState(initialDisplay);
    updateDisplay = setDisplay;
    React.useLayoutEffect(() => {
      committedDisplayGeneration = display.generation;
      displayCommits++;
      drawn();
    }, [display]);
    return <DisplayContext.Provider value={display}>{flowElement}</DisplayContext.Provider>;
  }
  function publishDisplay() {
    displayUpdates++;
    displayGeneration++;
    updateDisplay({ tier, lod, selected, assetUrl: options.assetUrl, generation: displayGeneration });
  }
  root.render(<Scene />);
  await ready;

  return {
    setCamera(next) {
      if (destroyed || (next.x === camera.x && next.y === camera.y && next.zoom === camera.zoom)) return;
      camera = { ...next };
      const nextTier = options.tier(camera), nextLod = options.lod(camera);
      if (tier !== nextTier || lod !== nextLod) { tier = nextTier; lod = nextLod; publishDisplay(); }
      void flow?.setViewport(viewport(camera), { duration: 0 }).then(drawn);
    },
    setSelected(id) {
      if (destroyed || selected === id) return;
      selected = id;
      publishDisplay();
    },
    moveCard(id, x, y) {
      if (destroyed) return;
      const previous = cards.get(id);
      if (!previous || (previous.x === x && previous.y === y)) return;
      const card = { ...previous, x, y };
      cards.set(id, card);
      flow?.updateNode(id, node => ({ position: { x, y }, data: { ...node.data, card } }));
      movedNodeUpdates++;
      drawn();
    },
    readyFraction() {
      if (displayGeneration !== committedDisplayGeneration) return 0;
      let visible = 0, loaded = 0;
      for (const card of cards.values()) {
        if (!options.visible(card, camera)) continue;
        visible++;
        const image = images.get(card.id);
        if (image?.isConnected && image.complete && image.naturalWidth > 0 && (image.currentSrc || image.src) === options.assetUrl(card.asset, tier)) loaded++;
      }
      return visible ? loaded / visible : 1;
    },
    stats() {
      return {
        backend: 'React Flow uncontrolled store / mounted-card display context / DOM transforms',
        rfMode: 'optimized', rfLayer: Number(layerHintRequested), viewportLayerHintApplied: layerHintApplied,
        rfHandles: explicitEmptyHandles ? 'empty' : 'unknown', initialBoundsMeasured: true,
        emptyHandlesOptimization: 'explicit no-port metadata; additional to current Creator measured bounds',
        drawProxy: 'mounted display commits, viewport mutations, moved-node updates and image load events',
        draws, sceneCards: cards.size, mountedCards: images.size,
        domNodes: host.querySelectorAll('*').length, imageElements: images.size,
        loadedImageElements: [...images.values()].filter(image => image.complete && image.naturalWidth > 0).length,
        distinctRequestedSourceUrls: requestedSources.size, builtInVisibleElementCulling: true,
        overscanScreenPixels: OVERSCAN, tier, lod, displayUpdates, displayCommits, movedNodeUpdates,
        displayGeneration, committedDisplayGeneration, allNodeDataCopiesOnDisplayChange: false,
      };
    },
    destroy() {
      if (destroyed) return;
      destroyed = true;
      root.unmount();
      view.remove();
      images.clear();
      requestedSources.clear();
      cards.clear();
    },
  };
}

/** Original controlled-node implementation, preserved for explicitly labeled comparisons. */
export async function createControlledReactFlowAdapter(host: HTMLElement, options: BenchOptions): Promise<Adapter> {
  const view = document.createElement('div');
  Object.assign(view.style, {
    position: 'absolute', left: `${-OVERSCAN}px`, top: `${-OVERSCAN}px`,
    width: `${options.width + OVERSCAN * 2}px`, height: `${options.height + OVERSCAN * 2}px`,
    pointerEvents: 'none',
  });
  host.appendChild(view);
  const images = new Map<string, HTMLImageElement>();
  const requestedSources = new Set<string>();
  let camera = { ...options.camera };
  let tier = options.tier(camera);
  let lod = options.lod(camera);
  let selected: string | null = null;
  let destroyed = false;
  let draws = 0;
  const layerHintRequested = new URLSearchParams(location.search).get('rfLayer') === '1';
  let layerHintApplied = false;
  let flow: ReactFlowInstance<MediaNode> | undefined;
  let updateNodes: (nodes: MediaNode[]) => void = () => {};
  let initialized!: () => void;
  const ready = new Promise<void>(resolve => { initialized = resolve; });
  const root = createRoot(view);

  function drawn() {
    if (destroyed) return;
    draws++;
    options.onDraw();
  }
  const registerImage = (id: string, image: HTMLImageElement | null) => {
    if (image) {
      images.set(id, image);
      requestedSources.add(image.src);
    } else images.delete(id);
  };
  const source = (card: Card) => options.assetUrl(card.asset, tier);
  let nodes: MediaNode[] = options.cards.map(original => {
    const card = { ...original };
    return {
      id: card.id, type: 'media', position: { x: card.x, y: card.y },
      width: card.width, height: card.height,
      initialWidth: card.width, initialHeight: card.height,
      style: { width: card.width, height: card.height },
      data: { card, source: source(card), lod, selected: false, registerImage, imageLoaded: drawn },
    };
  });
  const viewport = (at: Camera) => ({ x: at.x + OVERSCAN, y: at.y + OVERSCAN, zoom: at.zoom });
  const initialViewport = viewport(camera);

  function Scene() {
    const [shownNodes, setShownNodes] = React.useState(nodes);
    updateNodes = setShownNodes;
    React.useLayoutEffect(drawn, [shownNodes]);
    return <ReactFlow<MediaNode>
      nodes={shownNodes}
      edges={edges}
      nodeTypes={nodeTypes}
      defaultViewport={initialViewport}
      width={options.width + OVERSCAN * 2}
      height={options.height + OVERSCAN * 2}
      minZoom={0.01}
      maxZoom={8}
      onlyRenderVisibleElements
      nodesDraggable={false}
      nodesConnectable={false}
      nodesFocusable={false}
      edgesFocusable={false}
      elementsSelectable={false}
      selectionOnDrag={false}
      selectNodesOnDrag={false}
      panOnDrag={false}
      panOnScroll={false}
      zoomOnScroll={false}
      zoomOnPinch={false}
      zoomOnDoubleClick={false}
      autoPanOnNodeDrag={false}
      autoPanOnNodeFocus={false}
      preventScrolling={false}
      deleteKeyCode={null}
      selectionKeyCode={null}
      multiSelectionKeyCode={null}
      panActivationKeyCode={null}
      zoomActivationKeyCode={null}
      disableKeyboardA11y
      proOptions={proOptions}
      onInit={instance => { flow = instance; layerHintApplied = applyViewportLayerHint(view, layerHintRequested); initialized(); }}
    />;
  }

  root.render(<Scene />);
  await ready;

  return {
    setCamera(next) {
      if (destroyed || (next.x === camera.x && next.y === camera.y && next.zoom === camera.zoom)) return;
      camera = { ...next };
      const nextTier = options.tier(camera);
      const nextLod = options.lod(camera);
      if (tier !== nextTier || lod !== nextLod) {
        tier = nextTier;
        lod = nextLod;
        nodes = nodes.map(node => ({ ...node, data: { ...node.data, source: source(node.data.card), lod } }));
        updateNodes(nodes);
      }
      void flow?.setViewport(viewport(camera), { duration: 0 }).then(drawn);
    },
    setSelected(id) {
      if (destroyed || selected === id) return;
      const previous = selected;
      selected = id;
      nodes = nodes.map(node => node.id === previous || node.id === id
        ? { ...node, data: { ...node.data, selected: node.id === id } }
        : node);
      updateNodes(nodes);
    },
    moveCard(id, x, y) {
      if (destroyed) return;
      let changed = false;
      const moved = nodes.map(node => {
        if (node.id !== id || (node.position.x === x && node.position.y === y)) return node;
        changed = true;
        return { ...node, position: { x, y }, data: { ...node.data, card: { ...node.data.card, x, y } } };
      });
      if (changed) {
        nodes = moved;
        updateNodes(nodes);
      }
    },
    stats() {
      return {
        backend: 'React Flow / DOM / CSS compositor transforms',
        rfMode: 'controlled', rfLayer: Number(layerHintRequested), viewportLayerHintApplied: layerHintApplied,
        drawProxy: 'React node-list commits, viewport mutations and image load events',
        draws,
        sceneCards: nodes.length,
        mountedCards: images.size,
        domNodes: host.querySelectorAll('*').length,
        imageElements: images.size,
        loadedImageElements: [...images.values()].filter(image => image.complete && image.naturalWidth > 0).length,
        distinctRequestedSourceUrls: requestedSources.size,
        builtInVisibleElementCulling: true,
        overscanScreenPixels: OVERSCAN,
        tier,
        lod,
      };
    },
    readyFraction() {
      let visible = 0;
      let loaded = 0;
      for (const node of nodes) {
        if (!options.visible(node.data.card, camera)) continue;
        visible++;
        const image = images.get(node.id);
        if (image?.complete && image.naturalWidth > 0 && (image.currentSrc || image.src) === node.data.source) loaded++;
      }
      return visible ? loaded / visible : 1;
    },
    destroy() {
      if (destroyed) return;
      destroyed = true;
      root.unmount();
      view.remove();
      images.clear();
      requestedSources.clear();
    },
  };
}
