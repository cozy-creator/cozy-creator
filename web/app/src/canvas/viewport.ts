import type { Viewport as PixiViewport } from "pixi-viewport"
import type { Box } from "./graph"

export interface Point { x: number; y: number }
/** CSS-pixel translation from the canvas element's top left, and world-to-screen scale. */
export interface Viewport extends Point { zoom: number }

/** Logical geometry survives culling. The canonical anchor and the displayed envelope need not have the same top left. */
export interface CanvasNode {
  id: string
  type: "element" | "entity"
  data: { name: string; width: number }
  position: Point
  bounds: Box
  selected?: boolean
  gliding?: boolean
}

type NodePatch = Partial<Omit<CanvasNode, "id">>
type Listener = () => void
type CameraListener = (view: Viewport) => void
type Limits = { minZoom: number; maxZoom: number }
type Move = { duration?: number }
type Padding = { top?: number; right?: number; bottom?: number; left?: number }
type Fit = Move & Partial<Limits> & { bounds?: readonly Box[]; padding?: Padding }

const sameView = (a: Viewport, b: Viewport) => a.x === b.x && a.y === b.y && a.zoom === b.zoom
const validView = (view: Viewport) => Number.isFinite(view.x) && Number.isFinite(view.y) && Number.isFinite(view.zoom) && view.zoom > 0
const sameNode = (a: CanvasNode, b: CanvasNode) => a.id === b.id && a.type === b.type && a.data.name === b.data.name && a.data.width === b.data.width
  && a.position.x === b.position.x && a.position.y === b.position.y
  && a.bounds.x === b.bounds.x && a.bounds.y === b.bounds.y && a.bounds.w === b.bounds.w && a.bounds.h === b.bounds.h
  && Boolean(a.selected) === Boolean(b.selected) && Boolean(a.gliding) === Boolean(b.gliding)

/** CanvasStore owns document edits; this owns the camera and the small logical scene published to its renderer. */
export class CanvasController {
  private view: Viewport
  private moving = false
  private nodes: readonly CanvasNode[] = []
  private byId = new Map<string, CanvasNode>()
  private cameras = new Set<CameraListener>()
  private endings = new Set<CameraListener>()
  private scenes = new Set<Listener>()
  private drawings = new Set<Listener>()
  private drawn = false
  private enabled = true
  private binding: { element: HTMLElement; viewport: PixiViewport; stop: () => void } | null = null
  private animation: { frame: number; resolve: (landed: boolean) => void } | null = null

  constructor(initial: Viewport = { x: 0, y: 0, zoom: 1 }, private limits: Limits = { minZoom: 0.05, maxZoom: 4 }) {
    this.view = validView(initial) ? { ...initial } : { x: 0, y: 0, zoom: 1 }
  }

  getViewport = (): Viewport => this.view
  getNodes = (): readonly CanvasNode[] => this.nodes
  getNode = (id: string): CanvasNode | undefined => this.byId.get(id)

  subscribeCamera = (listener: CameraListener) => {
    this.cameras.add(listener)
    return () => { this.cameras.delete(listener) }
  }

  subscribeCameraEnd = (listener: CameraListener) => {
    this.endings.add(listener)
    return () => { this.endings.delete(listener) }
  }

  subscribeNodes = (listener: Listener) => {
    this.scenes.add(listener)
    return () => { this.scenes.delete(listener) }
  }

  subscribeDrawn = (listener: Listener) => {
    this.drawings.add(listener)
    return () => { this.drawings.delete(listener) }
  }

  get hasDrawn() { return this.drawn }

  /** Called after the renderer presents a frame, never merely because boxes were published. */
  markDrawn() { this.setDrawn(true) }

  private setDrawn(value: boolean) {
    if (this.drawn === value) return
    this.drawn = value
    for (const listener of this.drawings) listener()
  }

  replaceNodes(nodes: readonly CanvasNode[]) {
    const next = new Map<string, CanvasNode>()
    for (const node of nodes) {
      const previous = this.byId.get(node.id)
      next.set(node.id, previous && sameNode(previous, node) ? previous : node)
    }
    const values = [...next.values()]
    if (values.length === this.nodes.length && values.every((node, i) => node === this.nodes[i])) return
    this.byId = next
    this.nodes = values
    for (const listener of this.scenes) listener()
  }

  upsertNodes(nodes: CanvasNode | readonly CanvasNode[]) {
    const next = new Map(this.byId)
    for (const node of "id" in nodes ? [nodes] : nodes) next.set(node.id, node)
    this.replaceNodes([...next.values()])
  }

  /** A position alone translates the envelope; explicit bounds replace it. Data is a complete replacement. */
  updateNode(id: string, patch: NodePatch | ((node: CanvasNode) => NodePatch)) {
    const node = this.byId.get(id)
    if (!node) return
    const update = typeof patch === "function" ? patch(node) : patch
    const bounds = update.bounds ?? (update.position ? {
      ...node.bounds,
      x: node.bounds.x + update.position.x - node.position.x,
      y: node.bounds.y + update.position.y - node.position.y,
    } : node.bounds)
    const next = { ...node, ...update, id, bounds }
    if (sameNode(node, next)) return
    this.replaceNodes(this.nodes.map((node) => node.id === id ? next : node))
  }

  removeNodes(ids: Iterable<string>) {
    const removed = new Set(ids)
    this.replaceNodes(this.nodes.filter((node) => !removed.has(node.id)))
  }

  select(ids: Iterable<string>) {
    const selected = new Set(ids)
    this.replaceNodes(this.nodes.map((node) => Boolean(node.selected) === selected.has(node.id) ? node : { ...node, selected: selected.has(node.id) }))
  }

  /** Client coordinates include the page's canvas offset; world coordinates never do. */
  screenToWorld(point: Point): Point {
    const rect = this.binding?.element.getBoundingClientRect()
    return { x: (point.x - (rect?.left ?? 0) - this.view.x) / this.view.zoom, y: (point.y - (rect?.top ?? 0) - this.view.y) / this.view.zoom }
  }

  worldToScreen(point: Point): Point {
    const rect = this.binding?.element.getBoundingClientRect()
    return { x: point.x * this.view.zoom + this.view.x + (rect?.left ?? 0), y: point.y * this.view.zoom + this.view.y + (rect?.top ?? 0) }
  }

  private publish(view: Viewport) {
    if (sameView(view, this.view)) return
    this.view = { ...view }
    this.moving = true
    for (const listener of this.cameras) listener(this.view)
  }

  private place(view: Viewport) {
    const viewport = this.binding?.viewport
    if (viewport) {
      viewport.position.set(view.x, view.y)
      viewport.scale.set(view.zoom)
      viewport.dirty = true
    }
    this.publish(view)
  }

  private end() {
    if (this.animation || !this.moving) return
    this.moving = false
    for (const listener of this.endings) listener(this.view)
  }

  private cancelMove() {
    const held = this.animation
    if (!held) return
    this.animation = null
    cancelAnimationFrame(held.frame)
    held.resolve(false)
    this.end()
  }

  /** Programmatic Focus moves may exceed Wander's zoom limits. Only fitting and hand gestures clamp them. */
  setViewport(view: Viewport, { duration = 0 }: Move = {}): Promise<boolean> {
    if (!validView(view)) return Promise.resolve(false)
    this.cancelMove()
    const target = { ...view }
    if (sameView(this.view, target)) { this.end(); return Promise.resolve(true) }
    if (!this.binding || !Number.isFinite(duration) || duration <= 0) {
      this.place(target)
      this.end()
      return Promise.resolve(true)
    }
    const from = this.view
    const started = performance.now()
    return new Promise((resolve) => {
      const move = { frame: 0, resolve }
      this.animation = move
      const step = (now: number) => {
        if (this.animation !== move) return
        const t = Math.min(1, Math.max(0, (now - started) / duration))
        const ease = t * t * (3 - 2 * t)
        this.place(t === 1 ? target : { x: from.x + (target.x - from.x) * ease, y: from.y + (target.y - from.y) * ease, zoom: from.zoom + (target.zoom - from.zoom) * ease })
        if (this.animation !== move) return
        if (t < 1) move.frame = requestAnimationFrame(step)
        else {
          this.animation = null
          this.end()
          resolve(true)
        }
      }
      move.frame = requestAnimationFrame(step)
    })
  }

  fitView({ bounds = this.nodes.map((node) => node.bounds), padding = {}, minZoom = this.limits.minZoom, maxZoom = this.limits.maxZoom, duration }: Fit = {}): Promise<boolean> {
    const rect = this.binding?.element.getBoundingClientRect()
    if (!rect || rect.width <= 0 || rect.height <= 0) return Promise.resolve(false)
    const boxes = bounds.filter((box) => [box.x, box.y, box.w, box.h].every(Number.isFinite) && box.w >= 0 && box.h >= 0)
    if (!boxes.length) return Promise.resolve(false)
    let [left, top, right, bottom] = [Infinity, Infinity, -Infinity, -Infinity]
    for (const box of boxes) {
      left = Math.min(left, box.x); top = Math.min(top, box.y)
      right = Math.max(right, box.x + box.w); bottom = Math.max(bottom, box.y + box.h)
    }
    const positive = (value = 0) => Number.isFinite(value) ? Math.max(0, value) : 0
    let [pl, pt, pr, pb] = [positive(padding.left), positive(padding.top), positive(padding.right), positive(padding.bottom)]
    const across = Math.min(1, Math.max(0, rect.width - 1) / (pl + pr || 1))
    const down = Math.min(1, Math.max(0, rect.height - 1) / (pt + pb || 1))
    pl *= across; pr *= across; pt *= down; pb *= down
    const [width, height] = [rect.width - pl - pr, rect.height - pt - pb]
    const low = Number.isFinite(minZoom) && minZoom > 0 ? minZoom : this.limits.minZoom
    const high = Math.max(low, Number.isFinite(maxZoom) && maxZoom > 0 ? maxZoom : this.limits.maxZoom)
    const zoom = Math.max(low, Math.min(high, width / Math.max(1, right - left), height / Math.max(1, bottom - top)))
    return this.setViewport({ x: pl + width / 2 - (left + right) / 2 * zoom, y: pt + height / 2 - (top + bottom) / 2 * zoom, zoom }, { duration })
  }

  setInteractionEnabled(enabled: boolean) {
    this.enabled = enabled
    const plugins = this.binding?.viewport.plugins
    if (!plugins) return
    for (const name of ["drag", "pinch", "wheel", "decelerate"]) {
      const plugin = plugins.get(name)
      // Drag.resume clears its pointer anchor, so repeated enablement must not interrupt a hand still on the canvas.
      if (!plugin || plugin.paused === !enabled) continue
      if (enabled) plugins.resume(name)
      else plugins.pause(name)
    }
  }

  /** A pointer canceled outside the page has no up event. Retire its library gesture as well as the host's gesture. */
  cancelInteraction() {
    const viewport = this.binding?.viewport
    if (!viewport) return
    viewport.input.clear()
    // Drag.resume clears its anchor but retains its moved flag; Pinch has no public reset for its center.
    // Recreate only these input plugins through their public factories, keeping configuration and pause state.
    const drag = viewport.plugins.get("drag")
    if (drag) {
      viewport.drag(drag.options)
      if (drag.paused) viewport.plugins.pause("drag")
    }
    const pinch = viewport.plugins.get("pinch")
    if (pinch) {
      viewport.pinch(pinch.options)
      if (pinch.paused) viewport.plugins.pause("pinch")
    }
    const wheel = viewport.plugins.get("wheel")
    if (wheel) {
      viewport.wheel(wheel.options)
      if (wheel.paused) viewport.plugins.pause("wheel")
    }
    viewport.plugins.get("decelerate")?.reset()
    // This ends the hand gesture without stopping an independent Focus/Home camera animation.
    this.end()
  }

  /** The surface owns Pixi, resizing, its demand scheduler and destruction. This binding owns only camera synchronization. */
  bindCamera(element: HTMLElement, viewport: PixiViewport): () => void {
    this.binding?.stop()
    this.cancelMove()
    this.setDrawn(false)
    const read = () => {
      const view = { x: viewport.x, y: viewport.y, zoom: viewport.scale.x }
      if (!validView(view) || sameView(view, this.view)) return
      this.cancelMove()
      this.publish(view)
    }
    const end = () => { read(); this.end() }
    // pixi-viewport ends a move only on a frame after one that changed. A gesture whose events net to nothing within a frame (a wheel at a
    // zoom limit, which clamps back) changes no frame, so a still frame ends it too.
    const frame = () => { read(); if (!viewport.moving && !viewport.zooming) this.end() }
    const interrupt = () => { if (this.enabled) this.cancelMove() }
    const binding = { element, viewport, stop: () => {
      viewport.off("moved", read).off("frame-end", frame)
      viewport.off("moved-end", end).off("zoomed-end", end)
      element.removeEventListener("pointerdown", interrupt)
      element.removeEventListener("wheel", interrupt)
      if (this.binding !== binding) return
      this.cancelMove()
      this.binding = null
      this.setDrawn(false)
    } }
    this.binding = binding
    // Not on `zoomed`: a wheel or pinch emits it before clamp-zoom clamps, with a zoom past the limits. `moved` follows the clamp.
    viewport.on("moved", read).on("frame-end", frame)
    viewport.on("moved-end", end).on("zoomed-end", end)
    element.addEventListener("pointerdown", interrupt, { passive: true })
    element.addEventListener("wheel", interrupt, { passive: true })
    this.place(this.view)
    this.setInteractionEnabled(this.enabled)
    return binding.stop
  }
}
