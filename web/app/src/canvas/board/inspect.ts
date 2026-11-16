import type { Renderer, Text } from "pixi.js"
import type { CanvasStore } from "../CanvasStore"
import type { Box } from "../graph"
import { HEAD } from "../shown"
import type { Board } from "./Board"
import { hitOf, type Part } from "./CardView"
import type { Media } from "./Media"
import { pictureBox } from "./presentation"
import type { Scene } from "./Scene"

interface Sources {
  board: Board; store: CanvasStore; media: Media
  scene(): Scene | null; renderer(): Renderer | null
  interaction(): { state(): { pointerDown: boolean; dragging: boolean; marquee: boolean; resizing: string | null; hover: { name: string; part: Part } | null; locked: boolean } } | null
  frames(): number; ticking(): boolean; software(): boolean
}

/** The canvas as tests read it, in screen pixels and by meaning: what each card says, where its parts are, whether the canvas is still. */
export function inspection(o: Sources) {
  const camera = o.board.camera
  const screen = (box: Box | null, origin = { x: 0, y: 0 }, scale = 1) => {
    if (!box) return null
    const at = camera.worldToScreen({ x: origin.x + box.x * scale, y: origin.y + box.y * scale })
    const zoom = camera.getViewport().zoom * scale
    return { x: at.x, y: at.y, width: box.w * zoom, height: box.h * zoom }
  }
  const rendered = (name: string) => {
    const held = o.scene()?.viewOf(name)
    if (!held?.node || !held.show) return null
    const card = held.view, root = card.root
    const origin = { x: root.x, y: root.y }, scale = root.scale.x
    const body = pictureBox(held.node)
    const main = card.picture
    const texture = main?.handle?.texture
    const [source, width] = main ? JSON.parse(main.key || "[{},0]") as [{ kind?: string; sha256?: string }, number] : [{}, 0]
    const labels = Object.fromEntries([...card.labelViews()].map(([key, view]) => {
      const text = view as Text
      const role = key.startsWith("words-") || key === "words" ? key : key
      return [role, { text: String(text.text), fullText: card.drawn.full.get(key) ?? String(text.text), fontSize: Number(text.style?.fontSize ?? 0) * scale * camera.getViewport().zoom,
        ...screen({ x: view.x, y: view.y, w: view.width, h: view.height }, origin, scale)!, ...(key === "title" ? { title: held.show!.title } : {}) }]
    }))
    return {
      body: screen({ x: body.x - held.node.box.x, y: body.y - held.node.box.y, w: body.w, h: body.h }, origin, scale),
      transform: { x: root.x, y: root.y, scaleX: root.scale.x, scaleY: root.scale.y, alpha: root.alpha },
      labels, words: held.show.words, state: held.show.state, unseen: held.show.unseen,
      progress: held.show.progress ? { ...held.show.progress } : null,
      chips: card.drawn.chips.map((chip) => ({ text: chip.text, missing: chip.struck, struck: chip.struck, ...screen(chip.box, origin, scale)! })),
      controls: card.drawn.controls.map((control) => ({ part: control.part, ...screen(control.box, origin, scale)! })),
      stateDot: screen(card.drawn.dot, origin, scale), meter: screen(card.drawn.meter, origin, scale),
      parts: Object.fromEntries([...card.drawn.parts].map(([part, box]) => [part, { ...screen(box, origin, scale)!, ...(part === "delete" ? { icon: "trash" } : part === "cancel" && held.node!.view.placing ? { icon: "close" } : {}) }])),
      preview: card.previewing,
      glide: held.glide ? { active: true } : null, landing: held.landing !== undefined ? { active: true } : null,
      loadingPulse: main && !texture && main.next && !main.next.failed && !main.local ? { active: true } : null,
      media: { ready: Boolean(texture), failed: Boolean(main?.handle?.failed ?? main?.next?.failed), local: Boolean(main?.local), width: texture?.width ?? 0, height: texture?.height ?? 0,
        requestedWidth: width, resolution: main?.local ? "local" : source.kind === "thumb" ? String(width) : source.kind ? "original" : "", sha: source.sha256, key: source.kind },
      broken: false,
    }
  }
  const card = (name: string) => {
    const node = o.board.byName(name)
    if (!node) return null
    const view = o.store.get(name)
    const zoom = camera.getViewport().zoom
    const at = camera.worldToScreen({ x: node.bounds.x, y: node.bounds.y })
    const body = view ? pictureBox({ id: node.id, name, box: node.bounds, view, selected: Boolean(node.selected) }) : { x: node.bounds.x, y: node.bounds.y + HEAD, w: node.bounds.w, h: node.bounds.h - HEAD }
    const bodyAt = camera.worldToScreen(body)
    return {
      rendered: rendered(name), position: { ...node.position },
      bounds: { x: at.x, y: at.y, width: node.bounds.w * zoom, height: node.bounds.h * zoom },
      body: { x: bodyAt.x, y: bodyAt.y, width: body.w * zoom, height: body.h * zoom },
      title: { x: at.x, y: at.y, width: node.bounds.w * zoom, height: HEAD * zoom },
    }
  }
  const hitTest = (point: { x: number; y: number }) => {
    const renderer = o.renderer()
    if (!renderer) return null
    const rect = renderer.canvas.getBoundingClientRect()
    const boundary = renderer.events.rootBoundary
    if (!renderer.lastObjectRendered) return null
    boundary.rootTarget = renderer.lastObjectRendered
    const target = boundary.hitTest(point.x - rect.left, point.y - rect.top)
    return hitOf(target)
  }
  const idleNow = () => {
    const state = o.interaction()?.state()
    return !o.ticking() && !camera.isMoving && !state?.pointerDown && o.media.stats().loading === 0
  }
  return {
    camera: () => ({ ...camera.getViewport() }),
    names: () => o.board.getNodes().map((node) => node.data.name),
    selectedNames: () => o.board.getNodes().filter((node) => node.selected).map((node) => node.data.name),
    identities: () => o.board.getNodes().map((node) => ({ id: node.id, key: node.data.name, name: o.store.get(node.data.name)?.name ?? node.data.name })),
    hitTest, card,
    cards: () => o.board.getNodes().map((node) => {
      const view = o.store.get(node.data.name)
      const held = o.scene()?.viewOf(node.data.name)
      const hover = o.interaction()?.state().hover
      return { name: node.data.name, shownName: view ? (view.placing ? o.store.shownName(o.store.resolveName(node.data.name)) : view.name) : node.data.name, selected: Boolean(node.selected), hovered: hover?.name === node.data.name,
        state: held?.show?.state ?? null, progress: held?.show?.progress ?? null, drawn: Boolean(held), ...card(node.data.name)! }
    }),
    pointFor: (name: string, part: Part) => {
      const held = o.scene()?.viewOf(name)
      const box = part === "body" ? card(name)?.body : held?.view.drawn.parts.get(part)
      if (!box) return null
      const shown = "width" in box ? box : screen(box, { x: held!.view.root.x, y: held!.view.root.y }, held!.view.root.scale.x)!
      return { x: shown.x + shown.width / 2, y: shown.y + shown.height / 2 }
    },
    interaction: () => {
      const state = o.interaction()?.state()
      return { active: Boolean(o.renderer()), pointerDown: Boolean(state?.pointerDown), dragging: Boolean(state?.dragging), marquee: Boolean(state?.marquee), resizing: state?.resizing ?? null,
        locked: Boolean(state?.locked), requestedHover: state?.hover?.name ?? null, nativeHover: null, focused: o.store.focusedName.get() }
    },
    idleNow,
    idle: () => new Promise<void>((resolve) => { const check = () => (idleNow() ? resolve() : requestAnimationFrame(check)); requestAnimationFrame(check) }),
    resources: () => {
      const scene = o.scene()
      const views = scene ? [...scene.views()] : []
      const renderer = o.renderer()
      return { ...o.media.stats(), frames: o.frames(), scheduledFrame: o.ticking(), automaticTicker: false, totalCards: o.board.getNodes().length, visibleCards: views.length,
        loadedCards: views.filter(([, held]) => held.view.picture?.handle?.texture).length,
        graphics: renderer ? { backend: "gl" in renderer ? "webgl" : "canvas", software: o.software() } : null }
    },
  }
}
