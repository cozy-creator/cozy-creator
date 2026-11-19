// All canvas input, on Pixi's federated events: hover, taps, selection, group drag, marquee, corner resize, pans, keys and the context
// menu. One press at a time, owned from its press to its release wherever the pointer goes (Pixi delivers `pointerupoutside`).
import type { Container, FederatedPointerEvent } from "pixi.js"
import { takesKeys } from "../../ui/press"
import type { Viewport as PixiViewport } from "pixi-viewport"
import type { ToolField } from "../../api/calcifer"
import type { CanvasStore, ElementNode } from "../CanvasStore"
import { fieldsOf, shapeNear } from "../controls"
import type { Box } from "../graph"
import { ratioWords } from "../ratios"
import { aspectOf, bodyBox, bodyWidth, LOWEST, NARROWEST, reshapes, watched, WIDEST } from "../shown"
import type { Board } from "./Board"
import type { Camera } from "./Camera"
import type { StageProps } from "./Stage"

export type Part = "body" | "title" | "delete" | "cancel" | "corner" | "untool" | "dot" | "waits"
export interface Hit { name: string; part: Part }

export interface InteractionOptions {
  /** `.canvas-root`: keyboard focus. Keys are heard on window, as the page's other shortcuts are. */
  root: HTMLElement
  /** `eventMode: "static"`, its hit area the screen: every card part's events bubble to it. */
  stage: Container
  /** Background pan, pinch and wheel: the camera's. */
  viewport: PixiViewport
  /** `hold`s the viewport's drag while a press is this module's. */
  camera: Camera
  board: Board
  store: CanvasStore
  /** The card part a display object belongs to, or null for the background. */
  hitOf(target: Container | null): Hit | null
  scene: { setHover(hit: Hit | null): void; setResize(name: string | null, label?: string): void; setMarquee(box: Box | null): void }
  /** The surface's latest props. */
  props(): StageProps
  invalidate(): void
}

export interface Interaction {
  cancel(): void
  destroy(): void
  state(): { pointerDown: boolean; dragging: boolean; marquee: boolean; resizing: string | null; hover: Hit | null; locked: boolean }
}

/** A press that moved less than this, in screen pixels, is a tap. */
const TAP = 4
/** A card drag begins this far from its press, in screen pixels. */
const DRAG = 3
const DOUBLE_MS = 500
const DOUBLE_PX = 16

type Point = { x: number; y: number }
type Press =
  | { kind: "card"; id: number; hit: Hit; node: ElementNode; at: Point; from: Point; nodes: ElementNode[]; drags: boolean; pans: boolean; begun: boolean }
  | { kind: "background"; id: number; at: Point; writing: boolean }
  | { kind: "marquee"; id: number; at: Point; from: Point; additive: Set<string> }
  | { kind: "resize"; id: number; name: string; at: Point; zoom: number; w: number; h: number; control?: ToolField; pulled?: { w: number; h: number; shape?: string } }
  | { kind: "pan"; id: number; at: Point; camera: { x: number; y: number; zoom: number } }

export function createInteraction(o: InteractionOptions): Interaction {
  const { root, stage, camera, board, store, scene } = o
  let press: Press | null = null
  let hover: Hit | null = null
  /** What is under the pointer now, heard even while a press holds the hover still. */
  let under: Hit | null = null
  let space = false
  /** The press under way is a finger's. */
  let finger = false
  let lastTap: { name: string; at: number; x: number; y: number } | null = null
  let lastPaneTap: { at: number; x: number; y: number } | null = null
  const touches = new Set<number>()

  const native = (e: FederatedPointerEvent) => e.nativeEvent as PointerEvent
  const client = (e: { clientX: number; clientY: number }): Point => ({ x: e.clientX, y: e.clientY })
  const moved = (a: Point, b: Point) => Math.hypot(a.x - b.x, a.y - b.y)
  const modified = (e: { shiftKey: boolean; ctrlKey: boolean; metaKey: boolean }) => e.shiftKey || e.ctrlKey || e.metaKey
  const locked = () => Boolean(o.props().locked)
  const selectedIds = () => new Set(board.getNodes().filter((node) => node.selected).map((node) => node.id))

  /** The viewport's drag sits out a press this module owns. A finger on a card not selected pans, as one on the background does.
   *  Locked (returning from Focus) is disabled too: the Stage turns the camera's gestures off for both. */
  // A finger's flick carries the canvas on as it's let go; a mouse's or a pen's pan stops where it's let go, the canvas under the pointer
  // (move: "the canvas pans with the pointer"; Firefox's last moves were read as a throw).
  const sync = () => camera.hold(press !== null && press.kind !== "background" && !(press.kind === "card" && press.pans), !press || finger)

  const setHover = (hit: Hit | null) => {
    if (hover?.name === hit?.name && hover?.part === hit?.part) return
    hover = hit
    scene.setHover(hit)
    o.invalidate()
  }

  const over = (e: FederatedPointerEvent) => {
    under = o.hitOf(e.target as Container | null)
    // A press keeps the card it began on until it's let go (click-off-needs-press-outside).
    if (!press && e.pointerType === "mouse") setHover(under)
  }
  const out = (e: FederatedPointerEvent) => {
    if (o.hitOf(e.target as Container | null)?.name === under?.name) under = null
    if (!press && e.pointerType === "mouse") setHover(under)
  }

  const down = (e: FederatedPointerEvent) => {
    if (e.button > 1) return
    const wasWriting = writing(document.activeElement)
    root.focus({ preventScroll: true })
    if (!press) finger = e.pointerType === "touch"
    if (e.pointerType === "touch") {
      touches.add(e.pointerId)
      // A second finger is the viewport's pinch: whatever the first began is let go.
      if (touches.size > 1) { abandon(); lastTap = lastPaneTap = null; sync(); return }
    }
    if (press) return
    const at = client(e)
    const hit = o.hitOf(e.target as Container | null)
    const node = hit ? board.byName(hit.name) : undefined
    if (space || e.button === 1) {
      press = locked() || o.props().disabled ? null : { kind: "pan", id: e.pointerId, at, camera: { ...camera.getViewport() } }
      if (press) capture(e)
    } else if (hit && node && hit.part === "corner" && !locked()) {
      press = resizeFrom(e, hit.name, at)
    } else if (hit && node) {
      const touch = e.pointerType === "touch"
      if (modified(e)) {
        const selected = selectedIds()
        if (selected.has(node.id)) selected.delete(node.id)
        else selected.add(node.id)
        board.select(selected)
      } else if (!node.selected) board.select([node.id])
      const picked = board.getNodes().filter((held) => held.selected)
      // A finger on a card not selected pans; on a selected one it carries it. Returning from Focus, cards don't move.
      const pans = touch && !node.selected
      const drags = !modified(e) && hit.part !== "delete" && hit.part !== "cancel" && hit.part !== "untool" && hit.part !== "waits" && !locked() && !pans
      press = { kind: "card", id: e.pointerId, hit, node, at, from: camera.screenToWorld(at), drags, pans, begun: false,
        nodes: picked.map((held) => ({ ...held, position: { ...held.position }, bounds: { ...held.bounds } })) }
      if (drags) capture(e)
    } else if (e.shiftKey && !locked() && e.pointerType !== "touch") {
      press = { kind: "marquee", id: e.pointerId, at, from: camera.screenToWorld(at), additive: e.ctrlKey || e.metaKey ? selectedIds() : new Set() }
      capture(e)
    } else {
      press = { kind: "background", id: e.pointerId, at, writing: wasWriting }
    }
    if (press?.kind !== "background") lastPaneTap = null
    sync()
    if (press) window.addEventListener("pointermove", move)
    if (press && press.kind !== "background") e.preventDefault()
    o.invalidate()
  }

  const resizeFrom = (e: FederatedPointerEvent, name: string, at: Point): Press | null => {
    const view = store.get(name)
    const pose = store.poseOf(name)
    if (!view || !pose) return null
    const control = view.asset && reshapes(view) ? fieldsOf(view.asset, store.tools.get()).find((field) => field.type === "shape") : undefined
    capture(e)
    if (store.inPlace) store.inPlace.pulled = true
    scene.setResize(name, control ? ratioWords(aspectOf(view)) : undefined)
    return { kind: "resize", id: e.pointerId, name, at, zoom: camera.getViewport().zoom, w: pose.w, h: bodyBox(view, pose).w / aspectOf(view), control }
  }

  // Moves are the window's own, heard only while a press is held: past the canvas and the window edge too, and nothing walks the
  // scene on every move (Pixi's globalpointermove visits every interactive object).
  const move = (e: PointerEvent) => {
    if (!press || e.pointerId !== press.id) return
    const at = client(e)
    if (press.kind === "card" && moved(at, press.at) >= TAP) lastTap = null
    switch (press.kind) {
      case "pan": {
        void camera.setViewport({ ...press.camera, x: press.camera.x + at.x - press.at.x, y: press.camera.y + at.y - press.at.y })
        return
      }
      case "marquee": {
        const to = camera.screenToWorld(at), from = press.from
        const box = { x: Math.min(from.x, to.x), y: Math.min(from.y, to.y), w: Math.abs(to.x - from.x), h: Math.abs(to.y - from.y) }
        board.select([...press.additive, ...board.query(box).map((node) => node.id)])
        scene.setMarquee(box)
        o.invalidate()
        return
      }
      case "resize": {
        pull(press, at)
        return
      }
      case "card": {
        if (!press.drags || touches.size > 1) return
        const to = camera.screenToWorld(at), dx = to.x - press.from.x, dy = to.y - press.from.y
        if (!press.begun && moved(at, press.at) < DRAG) return
        const props = o.props()
        if (!press.begun) { press.begun = true; props.onDragStart(e, press.nodes[0], press.nodes) }
        board.upsertNodes(press.nodes.map((node) => ({ ...node, position: { x: node.position.x + dx, y: node.position.y + dy }, bounds: { ...node.bounds, x: node.bounds.x + dx, y: node.bounds.y + dy } })))
        const now = press.nodes.map((node) => board.getNode(node.id)).filter((node): node is ElementNode => Boolean(node))
        props.onDrag(e, now[0], now)
        o.invalidate()
        return
      }
    }
  }

  /** The corner under the pointer: free within its tool's bounds, snapped to the nearest ratio it makes, drawn at once and kept on release. */
  const pull = (held: Extract<Press, { kind: "resize" }>, at: Point) => {
    const view = store.get(held.name)
    if (!view) return
    const dx = (at.x - held.at.x) / held.zoom, dy = (at.y - held.at.y) / held.zoom
    const clamp = (value: number, low: number, high: number) => Math.min(high, Math.max(low, value))
    if (!held.control) {
      const w = clamp(held.w * Math.max(1 + dx / bodyWidth(view, held.w), 1 + dy / held.h), NARROWEST, WIDEST)
      held.pulled = { w, h: w / aspectOf(view) }
      store.setWidth(held.name, w, false)
      scene.setResize(held.name)
    } else {
      const w = clamp(held.w + dx, NARROWEST, WIDEST)
      const shape = shapeNear(held.control, w / clamp(held.h + dy, LOWEST, WIDEST))
      const [across, down] = shape.split(":").map(Number)
      const h = (w * down) / across
      held.pulled = { w, h, shape }
      store.drawShape(held.name, w, h)
      scene.setResize(held.name, ratioWords(shape))
    }
    o.invalidate()
  }

  const up = (e: FederatedPointerEvent) => {
    if (e.pointerType === "touch") touches.delete(e.pointerId)
    const held = press
    if (!held || e.pointerId !== held.id) { sync(); return }
    press = null
    window.removeEventListener("pointermove", move)
    release(e)
    const at = client(e)
    const props = o.props()
    switch (held.kind) {
      case "resize": {
        const size = held.pulled
        if (size && held.control) store.setShape(held.name, held.control, size.w, size.shape ?? shapeNear(held.control, size.w / size.h))
        else if (size) store.setWidth(held.name, size.w)
        scene.setResize(null)
        refit()
        break
      }
      case "marquee":
        scene.setMarquee(null)
        break
      case "background":
        if (moved(at, held.at) < TAP && !modified(e)) { board.select([]); props.onPaneClick(); paneTap(e, at, held.writing) }
        else lastPaneTap = null
        break
      case "card": {
        const node = board.getNode(held.node.id)
        if (held.begun) { props.onDragStop(native(e), held.nodes.map((one) => board.getNode(one.id) ?? one)[0], held.nodes.map((one) => board.getNode(one.id) ?? one)); break }
        if (!node || moved(at, held.at) >= TAP) break
        tap(e, node, held.hit.part)
        break
      }
      case "pan":
        break
    }
    sync()
    setHover(e.pointerType === "mouse" ? under : null)
    o.invalidate()
  }

  /** A tap on a card: it's clicked, then its part acts, and a second tap soon after on the same card opens it in Focus. */
  const tap = (e: FederatedPointerEvent, node: ElementNode, part: Part) => {
    const props = o.props(), event = native(e), name = node.data.name
    props.onNodeClick(event, node)
    if (part === "delete") { store.remove([name]); return }
    if (part === "untool") { store.takeToolOff(name); return }
    // A box waiting on a download: its words lead to it on its machine (cl-1205).
    if (part === "waits") {
      const run = store.get(name)?.run, download = run?.progress?.waiting_on?.[0]
      if (run?.machine && download) store.showDownload(run.machine, download)
      return
    }
    if (part === "cancel") {
      const view = store.get(name), take = view && watched(view)
      if (view?.placing) store.cancel(name)
      else if (take) store.cancelTake(name, { id: take.id, run: take.run })
      else store.cancelRender(name)
      return
    }
    if (part === "title" && !modified(e)) store.openName(name)
    const now = performance.now()
    if (lastTap?.name === name && now - lastTap.at < DOUBLE_MS && Math.hypot(e.clientX - lastTap.x, e.clientY - lastTap.y) < DOUBLE_PX) {
      lastTap = null
      props.onNodeDoubleClick(event, node)
    } else lastTap = { name, at: now, x: e.clientX, y: e.clientY }
  }

  /** A second tap on open canvas soon after and near the first: a double click there (Paul, 11-16: "you double-click, and it'll create a text
   *  node"). Not where a box keeps its room (its name, its lines under it), nor the tap that leaves a field being typed in. */
  const paneTap = (e: FederatedPointerEvent, at: Point, leftField: boolean) => {
    const now = performance.now(), last = lastPaneTap
    lastPaneTap = leftField ? null : { at: now, x: at.x, y: at.y }
    if (leftField || !last || now - last.at >= DOUBLE_MS || moved(at, last) >= DOUBLE_PX) return
    lastPaneTap = null
    const props = o.props()
    if (!props.active || props.disabled || locked()) return
    const point = camera.screenToWorld(at)
    const onBox = board.getNodes().some((node) => {
      const box = store.boxOf(node.data.name)
      return box && point.x >= box.x && point.x <= box.x + box.w && point.y >= box.y && point.y <= box.y + box.h
    })
    if (!onBox) props.onPaneDoubleClick(native(e), point)
  }

  /** A box opened in place on the canvas fits itself again once its corner is let go, as the card's own corner did. */
  const refit = () => {
    const owner = store.inPlace
    if (!owner) return
    owner.pulled = false
    requestAnimationFrame(() => { if (store.inPlace === owner) store.fitHere(true) })
  }

  /** What a press began is undone: a drag goes back, a pull goes back, a marquee keeps what it selected. */
  const abandon = () => {
    const held = press
    press = null
    window.removeEventListener("pointermove", move)
    if (!held) return
    if (held.kind === "card" && held.begun) o.props().onDragCancel(held.nodes)
    if (held.kind === "resize") {
      store.cancelPull(held.name, held.w)
      scene.setResize(null)
      refit()
    }
    if (held.kind === "marquee") scene.setMarquee(null)
    if (held.kind === "pan") void camera.setViewport(held.camera)
  }

  const capture = (e: FederatedPointerEvent) => {
    const target = e.nativeEvent.target
    if (target instanceof Element && e.pointerType !== "touch") target.setPointerCapture?.(e.pointerId)
  }
  const release = (e: FederatedPointerEvent) => {
    const target = e.nativeEvent.target
    if (target instanceof Element && target.hasPointerCapture?.(e.pointerId)) target.releasePointerCapture(e.pointerId)
  }

  const cancel = () => {
    abandon()
    touches.clear(); lastTap = lastPaneTap = null; space = false
    root.style.cursor = ""
    sync()
    setHover(null)
    o.invalidate()
  }

  /** Where typed keys belong to what has the focus (`takesKeys`). */
  const writing = takesKeys
  const keyDown = (event: KeyboardEvent) => {
    if (writing(event.target)) return
    if (event.key === "Escape" && press) { event.preventDefault(); cancel(); return }
    const props = o.props()
    if (!props.active || props.disabled) return
    if (event.code === "Space") {
      space = true
      root.style.cursor = "grab"
      if (event.target === root || event.target === document.body) event.preventDefault()
      return
    }
    if (event.target !== root) return
    const anchor = anchorNode()
    if (event.key === "Enter") {
      if (anchor) { event.preventDefault(); event.stopImmediatePropagation(); store.openHere(anchor.data.name) }
      return
    }
    // The menu key, or Shift+F10, opens the chosen box's menu where it is drawn.
    if (event.key === "ContextMenu" || (event.key === "F10" && event.shiftKey)) {
      if (!anchor) return
      event.preventDefault(); event.stopImmediatePropagation()
      const at = camera.worldToScreen({ x: anchor.bounds.x + anchor.bounds.w / 2, y: anchor.bounds.y + anchor.bounds.h / 2 })
      props.onNodeContextMenu(new MouseEvent("contextmenu", { clientX: at.x, clientY: at.y, cancelable: true }), anchor)
      return
    }
    const step = ({ ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 } as Record<string, number>)[event.key]
    if (!step) return
    event.preventDefault(); event.stopImmediatePropagation()
    const order = reading(board.getNodes())
    const at = anchor ? order.findIndex((node) => node.id === anchor.id) : -1
    const next = at < 0 ? order[0] : order[Math.max(0, Math.min(order.length - 1, at + step))]
    if (next) { board.select([next.id]); void camera.fitView({ bounds: [next.bounds], maxZoom: camera.getViewport().zoom }) }
  }
  const keyUp = (event: KeyboardEvent) => { if (event.code === "Space") { space = false; root.style.cursor = "" } }

  const anchorNode = () => board.getNodes().find((node) => node.selected)

  /** Every box in reading order, which the arrows walk: rows top to bottom, each left to right. A box starting above the middle of a row's
   *  first box is in that row. Right and Down go on, Left and Up back, so every box is reached and the way back is the way there. */
  const reading = (nodes: readonly ElementNode[]) => {
    const rows: ElementNode[][] = []
    for (const node of [...nodes].sort((a, b) => a.bounds.y - b.bounds.y || a.bounds.x - b.bounds.x)) {
      const row = rows.at(-1)
      if (row && node.bounds.y < row[0].bounds.y + row[0].bounds.h / 2) row.push(node)
      else rows.push([node])
    }
    return rows.flatMap((row) => row.sort((a, b) => a.bounds.x - b.bounds.x))
  }

  const menu = (event: MouseEvent) => {
    const props = o.props()
    if (!props.active || locked() || !under) return
    const node = board.byName(under.name)
    if (node) props.onNodeContextMenu(event, node)
  }

  const cancelled = (e: FederatedPointerEvent) => {
    if (e.pointerType === "touch") touches.delete(e.pointerId)
    if (press?.id === e.pointerId) cancel()
  }

  stage.on("pointerdowncapture", down)
  stage.on("pointerup", up)
  stage.on("pointerupoutside", up)
  stage.on("pointercancel", cancelled)
  stage.on("pointerover", over)
  stage.on("pointerout", out)
  root.addEventListener("contextmenu", menu)
  // Heard as they start down the page, not as they bubble back: a control that keeps Space or Enter to itself (the zoom control stops
  // them) would otherwise hide Space held for a pan. What isn't the canvas's is only looked at, never stopped.
  window.addEventListener("keydown", keyDown, true)
  window.addEventListener("keyup", keyUp, true)
  window.addEventListener("blur", cancel)
  sync()

  return {
    cancel,
    destroy() {
      cancel()
      stage.off("pointerdowncapture", down)
      window.removeEventListener("pointermove", move)
      stage.off("pointerup", up)
      stage.off("pointerupoutside", up)
      stage.off("pointercancel", cancelled)
      stage.off("pointerover", over)
      stage.off("pointerout", out)
      root.removeEventListener("contextmenu", menu)
      window.removeEventListener("keydown", keyDown, true)
      window.removeEventListener("keyup", keyUp, true)
      window.removeEventListener("blur", cancel)
    },
    state: () => ({
      pointerDown: press !== null,
      dragging: press?.kind === "card" && press.begun,
      marquee: press?.kind === "marquee",
      resizing: press?.kind === "resize" ? press.name : null,
      hover,
      locked: locked(),
    }),
  }
}
