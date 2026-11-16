import type { Viewport as PixiViewport } from "pixi-viewport"
import type { Box } from "../graph"

export interface Point { x: number; y: number }
/** CSS pixels from the canvas element's top left, and world-to-screen scale. */
export interface Viewport extends Point { zoom: number }
type Padding = { top?: number; right?: number; bottom?: number; left?: number }
export interface Fit { bounds: readonly Box[]; padding?: Padding; minZoom?: number; maxZoom?: number; duration?: number }

export const MIN_ZOOM = 0.05
export const MAX_ZOOM = 4

const same = (a: Viewport, b: Viewport) => a.x === b.x && a.y === b.y && a.zoom === b.zoom
const valid = (view: Viewport) => Number.isFinite(view.x) && Number.isFinite(view.y) && Number.isFinite(view.zoom) && view.zoom > 0

/** The one camera: pixi-viewport, read once per tick after its update. Until a renderer binds it, it holds the saved view. */
export class Camera {
  private view: Viewport
  private viewport: PixiViewport | null = null
  private element: HTMLElement | null = null
  private moving = false
  private still = 0
  private animation: { resolve(landed: boolean): void } | null = null
  private moves = new Set<(view: Viewport) => void>()
  private ends = new Set<(view: Viewport) => void>()
  private draws = new Set<() => void>()
  private drawn = false

  constructor(initial?: Viewport | null) {
    this.view = initial && valid(initial) ? { ...initial } : { x: 0, y: 0, zoom: 1 }
  }

  getViewport = (): Viewport => this.view
  get hasDrawn() { return this.drawn }
  get isMoving() { return this.moving || Boolean(this.animation) }

  subscribeCamera = (listener: (view: Viewport) => void) => { this.moves.add(listener); return () => { this.moves.delete(listener) } }
  subscribeCameraEnd = (listener: (view: Viewport) => void) => { this.ends.add(listener); return () => { this.ends.delete(listener) } }
  subscribeDrawn = (listener: () => void) => { this.draws.add(listener); return () => { this.draws.delete(listener) } }

  markDrawn(value = true) {
    if (this.drawn === value) return
    this.drawn = value
    for (const listener of this.draws) listener()
  }

  bind(element: HTMLElement, viewport: PixiViewport): () => void {
    this.element = element
    this.viewport = viewport
    this.place(this.view)
    return () => {
      if (this.viewport !== viewport) return
      this.finish(false)
      this.viewport = null
      this.element = null
      this.markDrawn(false)
    }
  }

  /** After each viewport.update: publish what moved; a gesture ends once two ticks pass with nothing moving. Returns whether to keep ticking. */
  sync(): boolean {
    const viewport = this.viewport
    if (!viewport) return false
    if (this.animation && !viewport.plugins.get("animate")) this.finish(false)
    const view = { x: viewport.x, y: viewport.y, zoom: viewport.scale.x }
    if (valid(view) && !same(view, this.view)) {
      this.view = view
      this.moving = true
      this.still = 0
      for (const listener of this.moves) listener(view)
      return true
    }
    if (!this.moving) return Boolean(this.animation)
    if (++this.still < 2 || this.animation || this.decelerating()) return true
    this.moving = false
    for (const listener of this.ends) listener(this.view)
    return false
  }

  private decelerating(): boolean {
    const plugin = this.viewport?.plugins.get("decelerate") as unknown as { x: number | null; y: number | null } | undefined
    return Boolean(plugin && (plugin.x || plugin.y))
  }

  screenToWorld(point: Point): Point {
    const rect = this.element?.getBoundingClientRect()
    return { x: (point.x - (rect?.left ?? 0) - this.view.x) / this.view.zoom, y: (point.y - (rect?.top ?? 0) - this.view.y) / this.view.zoom }
  }

  worldToScreen(point: Point): Point {
    const rect = this.element?.getBoundingClientRect()
    return { x: point.x * this.view.zoom + this.view.x + (rect?.left ?? 0), y: point.y * this.view.zoom + this.view.y + (rect?.top ?? 0) }
  }

  /** Programmatic moves (Focus, Home, Fit) may go past the hand-zoom limits; only gestures clamp. */
  setViewport(target: Viewport, { duration = 0 }: { duration?: number } = {}): Promise<boolean> {
    if (!valid(target)) return Promise.resolve(false)
    this.finish(false)
    const viewport = this.viewport
    if (!viewport || !(duration > 0) || same(target, this.view)) {
      this.place(target)
      return Promise.resolve(true)
    }
    viewport.plugins.pause("clamp-zoom")
    return new Promise((resolve) => {
      this.animation = { resolve }
      viewport.animate({
        position: { x: (viewport.screenWidth / 2 - target.x) / target.zoom, y: (viewport.screenHeight / 2 - target.y) / target.zoom },
        scale: target.zoom, time: duration, ease: "easeInOutSine", removeOnInterrupt: true,
        callbackOnComplete: () => { this.place(target); this.finish(true) },
      })
      viewport.dirty = true
    })
  }

  fitView({ bounds, padding = {}, minZoom = MIN_ZOOM, maxZoom = MAX_ZOOM, duration }: Fit): Promise<boolean> {
    const rect = this.element?.getBoundingClientRect()
    const boxes = bounds.filter((box) => [box.x, box.y, box.w, box.h].every(Number.isFinite))
    if (!rect || rect.width <= 0 || rect.height <= 0 || !boxes.length) return Promise.resolve(false)
    const left = Math.min(...boxes.map((box) => box.x)), top = Math.min(...boxes.map((box) => box.y))
    const right = Math.max(...boxes.map((box) => box.x + box.w)), bottom = Math.max(...boxes.map((box) => box.y + box.h))
    const room = (value = 0) => Math.max(0, Number.isFinite(value) ? value : 0)
    const [pl, pt, pr, pb] = [room(padding.left), room(padding.top), room(padding.right), room(padding.bottom)]
    const width = Math.max(1, rect.width - pl - pr), height = Math.max(1, rect.height - pt - pb)
    const zoom = Math.max(minZoom, Math.min(maxZoom, width / Math.max(1, right - left), height / Math.max(1, bottom - top)))
    return this.setViewport({ x: pl + width / 2 - (left + right) / 2 * zoom, y: pt + height / 2 - (top + bottom) / 2 * zoom, zoom }, { duration })
  }

  private place(view: Viewport) {
    const viewport = this.viewport
    if (viewport) {
      viewport.position.set(view.x, view.y)
      viewport.scale.set(view.zoom)
      viewport.dirty = true
    }
    if (same(view, this.view)) return
    this.view = { ...view }
    this.moving = true
    this.still = 0
    for (const listener of this.moves) listener(this.view)
    if (!viewport) { this.moving = false; for (const listener of this.ends) listener(this.view) }
  }

  private finish(landed: boolean) {
    const held = this.animation
    if (!held) return
    this.animation = null
    this.viewport?.plugins.remove("animate")
    this.viewport?.plugins.resume("clamp-zoom")
    held.resolve(landed)
  }
}
