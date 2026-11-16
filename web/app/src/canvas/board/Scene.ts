import { Color, Container, Graphics } from "pixi.js"
import type { Client } from "../../api/client"
import type { CanvasStore } from "../CanvasStore"
import { carried } from "../checkpoints"
import { GLIDE_MS } from "../clear"
import type { Box } from "../graph"
import { bandOf, hexOf, nameZoomOf, shownOf } from "../shown"
import type { Board, CanvasNode } from "./Board"
import type { Viewport } from "./Camera"
import { BODY, CardView, type CardEnv, type CardState, type Hit, type Palette, TITLE } from "./CardView"
import type { Media } from "./Media"
import { pictureBox, present, type Presentation, type RunContext, type SceneNode, type ScenePicture } from "./presentation"
import { fontEpoch } from "./text"

export interface Frame { moving: boolean; camera: Viewport; width: number; height: number; dpr: number; settled: boolean; theme: string; now: number; hidden: ReadonlySet<string>; offering: boolean }
interface Held { view: CardView; node?: SceneNode; show?: Presentation; key?: unknown[]; glide?: { from: { x: number; y: number }; began: number }; landing?: number; seen?: number }

const MARGIN = 160
/** A box's name sits this far over it (app.css): a ring that would reach it is drawn inside the box's edge. */
const NAME_FOOT = 12
const ease = (t: number) => 1 - (1 - t) ** 3

/** The cards in view, each with its own CardView; virtualization over the board's grid. Every card's work is isolated. */
export class Scene {
  readonly layer = new Container({ eventMode: "passive", sortableChildren: true })
  private held = new Map<string, Held>()
  private spare: CardView[] = []
  private hover: Hit | null = null
  private resize: { name: string; label: string } | null = null
  private marquee = new Graphics()
  private rings = new Graphics()
  private ringed = new Map<string, { kind: "selected" | "hovered"; px: number; inside: boolean }>()
  private zoom = 1
  private palette?: { theme: string; colors: Palette }
  private faults = new Set<string>()
  private resolution = 1
  private settledZoom = 1
  private landed = new Set<string>()
  /** After the last update: boxes in view, those built, those with a picture to show, and those whose picture is drawn or said unavailable. */
  coverage = { inView: 0, built: 0, wanting: 0, pictured: 0 }
  env?: CardEnv

  constructor(private o: { world: Container; overlay: Container; board: Board; store: CanvasStore; client: Client; media: Media; software: boolean; invalidate(): void; contextOf(view: SceneNode["view"]): RunContext | undefined }) {
    this.marquee.eventMode = "none"
    this.rings.eventMode = "none"
    o.world.addChild(this.layer, this.marquee)
    o.overlay.addChild(this.rings)
  }

  setHover(hit: Hit | null) { if (hit?.name !== this.hover?.name || hit?.part !== this.hover?.part) { this.hover = hit; this.o.invalidate() } }
  setResize(name: string | null, label?: string) { this.resize = name ? { name, label: label ?? "" } : null; this.o.invalidate() }
  setMarquee(box: Box | null) {
    this.marquee.clear()
    if (box && this.palette) this.marquee.rect(box.x, box.y, box.w, box.h).fill({ color: this.palette.colors.select, alpha: 0.1 }).stroke({ width: 1 / this.zoom, color: this.palette.colors.select })
    this.o.invalidate()
  }
  get hovered() { return this.hover }
  get resizing() { return this.resize?.name ?? null }

  /** One frame's scene work. Returns whether a card animates (another frame) and when a clock next needs a repaint. */
  update(frame: Frame): { animating: boolean; wake: number | null } {
    const { camera, now } = frame
    const zoom = this.zoom = Math.max(0.0001, camera.zoom)
    if (frame.settled || !this.env) { this.settledZoom = zoom; this.resolution = Math.max(1, Math.min(4, 2 ** Math.ceil(Math.log2(Math.max(0.001, zoom * frame.dpr))))) }
    if (this.palette?.theme !== frame.theme) this.palette = { theme: frame.theme, colors: paletteOf() }
    const env = this.env = this.environment(frame, zoom)
    const margin = MARGIN / zoom
    const view = { x: -camera.x / zoom - margin, y: -camera.y / zoom - margin, w: frame.width / zoom + margin * 2, h: frame.height / zoom + margin * 2 }
    const live = new Set<string>()
    let animating = false, wake: number | null = null
    const tools = this.o.store.tools.get()
    const going = carried.get()
    const visit = (node: CanvasNode) => {
      if (live.has(node.id)) return
      const viewOf = this.o.store.get(node.data.name)
      if (!viewOf) return
      live.add(node.id)
      let held = this.held.get(node.id)
      if (!held) {
        const card = this.spare.pop() ?? new CardView()
        this.layer.addChild(card.root)
        held = { view: card }
        this.held.set(node.id, held)
      }
      try {
        // While something is carried, every box that would take it says so (AddOns' outline, now drawn at rest).
        const takes = Boolean(going && viewOf.asset && !going.whyNot(node.data.name, viewOf, tools))
        animating = this.card(held, node, viewOf, frame, env, tools, takes) || animating
        const show = held.show
        if (show?.progress?.time) wake = Math.min(wake ?? 1000, 1000 - (Date.now() % 1000))
      } catch (error) {
        held.view.fault(held.node ?? { id: node.id, name: node.data.name, box: node.bounds, view: viewOf, selected: Boolean(node.selected) }, env)
        // The broken tile stands in its own box, never where its view last was (a new view sits at 0,0, over the box there).
        const root = held.view.root
        root.scale.set(1); root.alpha = 1; root.zIndex = 0
        root.position.set(node.bounds.x, node.bounds.y)
        root.visible = !frame.hidden.has(node.data.name)
        const message = `${node.id}:${error instanceof Error ? error.message : String(error)}`
        if (!this.faults.has(message)) { this.faults.add(message); console.error(`Couldn't draw ${node.data.name}:`, error) }
      }
    }
    for (const node of this.o.board.query(view)) visit(node)
    // A card still gliding toward a place out of view keeps drawing until it has left.
    for (const [id, held] of this.held) if (!live.has(id) && held.glide && now - held.glide.began < GLIDE_MS) { const node = this.o.board.getNode(id); if (node) visit(node) }
    for (const [id, held] of this.held) if (!live.has(id)) this.release(id, held)
    let wanting = 0, pictured = 0
    for (const held of this.held.values()) {
      if (!held.show?.picture) continue
      wanting++
      const handle = held.view.picture?.handle
      if (handle?.texture || handle?.failed) pictured++
    }
    this.coverage = { inView: live.size, built: this.held.size, wanting, pictured }
    this.drawRings(frame, zoom)
    return { animating, wake }
  }

  /** The selection and the hover as Figma draws them (Paul, 11-16): one line in --select on the box's edge, 2 px selected and 1 px under
   *  the pointer, in screen pixels at every zoom and on device pixels. Where it would reach the name over the box, just inside its edge.
   *  One Graphics in screen space holds every ring, redrawn with the frame: a zoom re-inks no card. */
  private drawRings(frame: Frame, zoom: number) {
    const rings = this.rings, colors = this.palette?.colors
    rings.clear()
    this.ringed.clear()
    if (!colors) return
    const { camera, dpr } = frame
    const snap = (value: number) => Math.round(value * dpr) / dpr
    for (const held of this.held.values()) {
      const node = held.node, root = held.view.root
      if (!node || !root.visible) continue
      const hovered = !node.selected && this.hover?.name === node.name && !node.view.hot
      if (!node.selected && !hovered) continue
      const px = node.selected ? 2 : 1, inside = NAME_FOOT * zoom < px + 1
      const body = pictureBox(node), scale = root.scale.x
      const x = (root.x + (body.x - node.box.x) * scale) * zoom + camera.x, y = (root.y + (body.y - node.box.y) * scale) * zoom + camera.y
      const l = snap(x), t = snap(y), r = snap(x + body.w * scale * zoom), b = snap(y + body.h * scale * zoom)
      if (inside) rings.rect(l, t, r - l, px).rect(l, b - px, r - l, px).rect(l, t + px, px, b - t - 2 * px).rect(r - px, t + px, px, b - t - 2 * px)
      else rings.rect(l - px, t - px, r - l + 2 * px, px).rect(l - px, b, r - l + 2 * px, px).rect(l - px, t, px, b - t).rect(r, t, px, b - t)
      this.ringed.set(node.name, { kind: node.selected ? "selected" : "hovered", px, inside })
    }
    if (this.ringed.size) rings.fill(colors.select)
  }

  /** The ring drawn on a box in the last frame, if any. */
  ringOf(name: string) { return this.ringed.get(name) ?? null }

  private environment(frame: Frame, zoom: number): CardEnv {
    const band = bandOf(zoom)
    return {
      palette: this.palette!.colors, theme: frame.theme, zoom, band, titleSize: Math.max(TITLE, BODY * nameZoomOf(zoom)),
      resolution: this.resolution, epoch: fontEpoch(), moving: frame.moving, media: this.o.media, reduced: matchMedia("(prefers-reduced-motion: reduce)").matches, software: this.o.software,
      invalidate: this.o.invalidate,
      request: (picture, width) => this.request(picture, width, frame.dpr),
      onRevealed: (name, aspect, local) => {
        if (!local) this.o.store.showThumb(name)
        const view = this.o.store.get(name)
        if (aspect && view && !view.aspect && !view.asset?.width && !view.entity && !view.asset?.timeline) this.o.store.setAspect(name, aspect)
      },
    }
  }

  private card(held: Held, node: CanvasNode, view: NonNullable<ReturnType<CanvasStore["get"]>>, frame: Frame, env: CardEnv, tools: unknown, takes: boolean): boolean {
    const store = this.o.store
    const name = node.data.name
    const dragging = store.isDragging(name)
    const runContext = this.o.contextOf(view)
    const previous = held.node
    const offering = dragging && frame.offering
    // A pan touches no card's objects: the same inputs keep the same node, and no per-card allocation happens.
    const scene: SceneNode = previous && previous.box === node.bounds && previous.view === view && previous.selected === Boolean(node.selected) && previous.gliding === node.gliding
      && previous.dragging === dragging && previous.offering === offering && previous.runContext === runContext
      ? previous : { id: node.id, name, box: node.bounds, view, selected: Boolean(node.selected), gliding: node.gliding, dragging, offering, runContext }
    const second = held.show?.progress?.time ? Math.floor(Date.now() / 1000) : 0
    const key = held.key ?? (held.key = [])
    const inputs = [view, tools, runContext?.run, runContext?.machine, runContext?.on, runContext?.installing, second, store.names.get()] as const
    let stale = key.length !== inputs.length
    for (let i = 0; i < inputs.length; i++) if (key[i] !== inputs[i]) { key[i] = inputs[i]; stale = true }
    if (stale || !held.show) held.show = present(store, scene)
    held.node = scene
    const card = held.view
    const state: CardState = {
      hover: this.hover?.name === name ? this.hover.part : null, dragging, offering: scene.offering ?? false,
      resizing: this.resize?.name === name ? this.resize.label : null, dropTarget: takes && !dragging, broken: false,
    }
    card.update(scene, held.show!, state, env)
    // Position: the store's box, slid there when the store says it glides, never re-laid out by a move alone.
    const now = frame.now
    if (previous && (previous.box.x !== node.bounds.x || previous.box.y !== node.bounds.y)) {
      held.glide = node.gliding && !dragging && !env.reduced ? { from: { x: card.root.x, y: card.root.y }, began: now } : undefined
    }
    let animating = false
    let x = node.bounds.x, y = node.bounds.y
    if (held.glide) {
      const t = Math.min(1, (now - held.glide.began) / GLIDE_MS)
      if (t >= 1) held.glide = undefined
      else { const p = ease(t); x = held.glide.from.x + (x - held.glide.from.x) * p; y = held.glide.from.y + (y - held.glide.from.y) * p; animating = true }
    }
    let scale = dragging ? (scene.offering ? 0.7 : 1.015) : 1
    let alpha = dragging ? (scene.offering ? 0.55 : 0.94) : 1
    if (view.landing && !view.placing && !view.preview && !this.landed.has(node.id) && !env.reduced) { this.landed.add(node.id); held.landing = now }
    if (held.landing !== undefined) {
      const t = (now - held.landing) / 260
      if (t >= 1) { held.landing = undefined; if (store.get(name) === view && view.landing) store.patch(name, { landing: undefined }) }
      else { scale *= t < 0.6 ? 0.82 + 0.22 * ease(t / 0.6) : 1.04 - 0.04 * ease((t - 0.6) / 0.4); alpha *= t < 0.6 ? 0.4 + 0.6 * ease(t / 0.6) : 1; animating = true }
    }
    card.root.scale.set(scale)
    card.root.position.set(x + node.bounds.w * (1 - scale) / 2, y + node.bounds.h * (1 - scale) / 2)
    card.root.alpha = alpha
    card.root.zIndex = dragging ? 3 : state.hover ? 2 : node.selected ? 1 : 0
    card.root.visible = !frame.hidden.has(name)
    return card.animate(now, env) || animating
  }

  private request(picture: ScenePicture, width: number, dpr: number) {
    const project = this.o.store.projectId
    const size = Math.max(256, Math.min(4096, 2 ** Math.ceil(Math.log2(Math.max(1, width * this.settledZoom * dpr)))))
    if (picture.url) return { source: { kind: "local" as const, url: picture.url }, width: size, local: true }
    if (!project) return null
    if (picture.glimpse) return { source: { kind: "output" as const, run: picture.glimpse.run, output: picture.glimpse.output, index: picture.glimpse.index, rev: picture.glimpse.rev }, width: size, local: false }
    if (!picture.sha256) return null
    const sha256 = hexOf(picture.sha256)
    if (size > 512 && picture.kind === "image" && picture.path) return { source: { kind: "file" as const, sha256, path: picture.path }, width: size, local: false }
    return { source: { kind: "thumb" as const, sha256 }, width: Math.min(size, 512), local: false }
  }

  /** After a frame is presented: what a person has now seen loses its unseen dot (cl-928). */
  didRender() {
    const store = this.o.store
    if (!store.active) return
    for (const held of this.held.values()) {
      const node = held.node, view = node?.view
      const unseen = view?.asset?.unseen
      if (!node || !view || !unseen?.length || !held.view.root.visible || store.get(node.name) !== view) continue
      const shown = shownOf(view)
      if (shown?.number === undefined || !unseen.includes(shown.number) || held.seen === shown.number) continue
      const ready = held.view.picture?.sprite.visible || (shown.field === "text" && Boolean(held.show?.words))
      if (ready) { held.seen = shown.number; store.markSeen(node.name, shown.number) }
    }
  }

  /** Context restored: every resident card draws again at once. */
  redrawAll() { for (const held of this.held.values()) { held.key = undefined; held.view.refresh() } this.faults.clear(); this.o.invalidate() }

  views(): Iterable<[string, Held]> { return this.held }
  viewOf(name: string): Held | undefined { const id = this.o.board.byName(name)?.id; return id ? this.held.get(id) : undefined }
  get settled() { return this.settledZoom }

  private release(id: string, held: Held) {
    this.held.delete(id)
    held.view.destroy()
  }

  destroy() {
    for (const [id, held] of [...this.held]) this.release(id, held)
    for (const view of this.spare) view.destroy()
    this.spare = []
    this.layer.destroy({ children: true })
    this.marquee.destroy()
    this.rings.destroy()
  }
}

/** The page's tokens, once per theme. */
function paletteOf(): Palette {
  const css = getComputedStyle(document.documentElement)
  const read = (name: string, fallback: string) => new Color(css.getPropertyValue(name).trim() || fallback)
  const num = (name: string, fallback: string) => read(name, fallback).toNumber()
  const edge = read("--box-edge", "#ffffff40"), shade = read("--box-shade", "#0000008c")
  const mark = read("--mark", "#ffffff38"), mark2 = read("--mark2", "#ffffff70")
  return {
    card: num("--card", "#17181b"), well: num("--well", "#0a0a0b"), line: num("--line", "#2b2d32"), ink: num("--ink", "#e8e6e1"), mute: num("--mute", "#a3a5ac"),
    select: num("--select", "#0d99ff"),
    queued: num("--state-queued", "#9a9ca3"), running: num("--state-running", "#5aa9ff"), unseen: num("--state-unseen", "#4ade80"), failed: num("--state-failed", "#f0625d"),
    ember: num("--ember", "#ff8a3d"), ember2: num("--ember2", "#ffb37d"),
    line2: num("--line2", "#3a3d44"), mark: mark.toNumber(), markAlpha: mark.alpha, mark2: mark2.toNumber(), mark2Alpha: mark2.alpha,
    box: num("--box", "#202226"), edge: edge.toNumber(), edgeAlpha: edge.alpha, shade: shade.toNumber(), shadeAlpha: shade.alpha,
    check: css.getPropertyValue("--check").trim() || "#fbfbfb", check2: css.getPropertyValue("--check2").trim() || "#e8e8e8",
  }
}
