import type { Box } from "../graph"
import { Camera, type Fit, type Point, type Viewport } from "./Camera"

/** A card's logical geometry. It outlives its display objects: culling never loses a position, a selection or a drag. */
export interface CanvasNode {
  id: string
  type: "element" | "entity"
  data: { name: string; width: number }
  /** The canonical anchor the store saves. */
  position: Point
  /** The displayed envelope; its top left need not be the anchor. */
  bounds: Box
  selected?: boolean
  gliding?: boolean
}
type Patch = Partial<Omit<CanvasNode, "id">>
type Listener = (changed: ReadonlySet<string>) => void

const CELL = 1024
const same = (a: CanvasNode, b: CanvasNode) => a.type === b.type && a.data.name === b.data.name && a.data.width === b.data.width
  && a.position.x === b.position.x && a.position.y === b.position.y && a.bounds.x === b.bounds.x && a.bounds.y === b.bounds.y
  && a.bounds.w === b.bounds.w && a.bounds.h === b.bounds.h && Boolean(a.selected) === Boolean(b.selected) && Boolean(a.gliding) === Boolean(b.gliding)
const cellsOf = (box: Box) => {
  const keys: string[] = []
  for (let x = Math.floor(box.x / CELL); x <= Math.floor((box.x + box.w) / CELL); x++)
    for (let y = Math.floor(box.y / CELL); y <= Math.floor((box.y + box.h) / CELL); y++) keys.push(`${x},${y}`)
  return keys
}

/** The canvas the page talks to: every card's geometry and selection, a grid to find them by place, and the camera. */
export class Board {
  readonly camera: Camera
  private nodes: CanvasNode[] = []
  private byId = new Map<string, CanvasNode>()
  private names = new Map<string, string>()
  private grid = new Map<string, Set<string>>()
  private cells = new Map<string, string[]>()
  private listeners = new Set<Listener>()

  constructor(initial?: Viewport | null) {
    this.camera = new Camera(initial)
  }

  getNodes = (): readonly CanvasNode[] => this.nodes
  getNode = (id: string): CanvasNode | undefined => this.byId.get(id)
  /** The first card by this name, as the page has always looked one up (a transient duplicate never hides the first). */
  byName = (name: string): CanvasNode | undefined => {
    const id = this.names.get(name), node = id ? this.byId.get(id) : undefined
    return node && node.data.name === name && this.nodes.find((held) => held.data.name === name) === node ? node : this.nodes.find((held) => held.data.name === name)
  }
  selected = (): CanvasNode[] => this.nodes.filter((node) => node.selected)

  subscribeNodes = (listener: Listener) => { this.listeners.add(listener); return () => { this.listeners.delete(listener) } }

  /** The cards whose envelope meets `box`, in board order. */
  query(box: Box): CanvasNode[] {
    const ids = new Set<string>()
    for (const key of cellsOf(box)) for (const id of this.grid.get(key) ?? []) ids.add(id)
    return this.nodes.filter((node) => ids.has(node.id) && node.bounds.x <= box.x + box.w && node.bounds.x + node.bounds.w >= box.x
      && node.bounds.y <= box.y + box.h && node.bounds.y + node.bounds.h >= box.y)
  }

  replaceNodes(nodes: readonly CanvasNode[]) {
    const changed = new Set<string>()
    const next = new Map<string, CanvasNode>()
    for (const node of nodes) {
      const previous = this.byId.get(node.id)
      const kept = previous && same(previous, node) ? previous : node
      if (kept !== previous) changed.add(node.id)
      next.set(node.id, kept)
    }
    for (const id of this.byId.keys()) if (!next.has(id)) changed.add(id)
    const ordered = [...next.values()]
    if (!changed.size && ordered.length === this.nodes.length && ordered.every((node, i) => node === this.nodes[i])) return
    this.byId = next
    this.nodes = ordered
    this.names = new Map()
    for (const node of ordered) if (!this.names.has(node.data.name)) this.names.set(node.data.name, node.id)
    for (const id of changed) this.index(id)
    for (const listener of this.listeners) listener(changed)
  }

  upsertNodes(nodes: CanvasNode | readonly CanvasNode[]) {
    const list = "id" in nodes ? [nodes] : nodes
    const changed = new Set<string>()
    for (const node of list) {
      const previous = this.byId.get(node.id)
      if (previous && same(previous, node)) continue
      if (!previous) this.nodes = [...this.nodes, node]
      else this.nodes = this.nodes.map((held) => (held.id === node.id ? node : held))
      // A rename frees its old name only if that name still points here: another card may already hold it.
      if (previous && previous.data.name !== node.data.name && this.names.get(previous.data.name) === node.id) this.names.delete(previous.data.name)
      this.byId.set(node.id, node)
      if (!this.names.has(node.data.name)) this.names.set(node.data.name, node.id)
      this.index(node.id)
      changed.add(node.id)
    }
    if (changed.size) for (const listener of this.listeners) listener(changed)
  }

  /** A position alone moves the envelope with it; explicit bounds replace it. */
  updateNode(id: string, patch: Patch | ((node: CanvasNode) => Patch)) {
    const node = this.byId.get(id)
    if (!node) return
    const update = typeof patch === "function" ? patch(node) : patch
    const bounds = update.bounds ?? (update.position
      ? { ...node.bounds, x: node.bounds.x + update.position.x - node.position.x, y: node.bounds.y + update.position.y - node.position.y }
      : node.bounds)
    this.upsertNodes({ ...node, ...update, id, bounds })
  }

  removeNodes(ids: Iterable<string>) {
    const gone = new Set(ids)
    this.replaceNodes(this.nodes.filter((node) => !gone.has(node.id)))
  }

  select(ids: Iterable<string>) {
    const wanted = new Set(ids)
    const changed = this.nodes.filter((node) => Boolean(node.selected) !== wanted.has(node.id))
    if (changed.length) this.upsertNodes(changed.map((node) => ({ ...node, selected: wanted.has(node.id) })))
  }

  private index(id: string) {
    for (const key of this.cells.get(id) ?? []) this.grid.get(key)?.delete(id)
    const node = this.byId.get(id)
    if (!node) { this.cells.delete(id); return }
    const keys = cellsOf(node.bounds)
    this.cells.set(id, keys)
    for (const key of keys) {
      let cell = this.grid.get(key)
      if (!cell) this.grid.set(key, (cell = new Set()))
      cell.add(id)
    }
  }

  // The camera, for the page's existing callers.
  getViewport = () => this.camera.getViewport()
  setViewport = (view: Viewport, move?: { duration?: number }) => this.camera.setViewport(view, move)
  fitView = (fit: Partial<Fit> = {}) => this.camera.fitView({ ...fit, bounds: fit.bounds ?? this.nodes.map((node) => node.bounds) })
  screenToWorld = (point: Point) => this.camera.screenToWorld(point)
  worldToScreen = (point: Point) => this.camera.worldToScreen(point)
  subscribeCamera = (listener: (view: Viewport) => void) => this.camera.subscribeCamera(listener)
  subscribeCameraEnd = (listener: (view: Viewport) => void) => this.camera.subscribeCameraEnd(listener)
  subscribeDrawn = (listener: () => void) => this.camera.subscribeDrawn(listener)
  get hasDrawn() { return this.camera.hasDrawn }
}
