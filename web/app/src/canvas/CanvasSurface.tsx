// Pixi's CSP-safe uniform synchronization; this does not grant eval permission.
import "pixi.js/unsafe-eval"
import { Application, Container, EventsTicker, type WebGLRenderer } from "pixi.js"
import { Viewport as PixiViewport } from "pixi-viewport"
import { type ReactNode, useEffect, useId, useLayoutEffect, useRef, useState } from "react"
import { useQueryClient } from "@tanstack/react-query"
import { useConnection } from "../app/Connection"
import { machinesQuery, runFullQuery } from "../api/queries"
import { ownActivity, waitsOnOf } from "../panels/machines/activity"
import { useSignal, tabShown } from "../ui/signal"
import { theme } from "../ui/theme"
import { Card } from "./Card"
import { EntityCard } from "./EntityCard"
import { type CanvasStore, type ElementNode } from "./CanvasStore"
import { useCanvasViewport } from "./viewportContext"
import type { Viewport } from "./viewport"
import { createPixiScene, type SceneNode } from "./pixi/Scene"
import { pictureBox } from "./pixi/presentation"
import { nameZoomOf, HEAD, watched } from "./shown"
import { ZOOMS } from "./ZoomControl"

type Pointer = MouseEvent | PointerEvent
interface Props {
  store: CanvasStore
  active: boolean
  disabled: boolean
  locked?: boolean
  children?: ReactNode
  onReady?(): void
  onSelectionChange(value: { nodes: readonly ElementNode[] }): void
  onMoveStart?(): void
  onMoveEnd(event: unknown, view: Viewport): void
  onNodeClick(event: MouseEvent, node: ElementNode): void
  onNodeDoubleClick(event: MouseEvent, node: ElementNode): void
  onPaneClick(): void
  onNodeContextMenu(event: MouseEvent, node: ElementNode): void
  onDragStart(event: Pointer, node: ElementNode, nodes: ElementNode[]): void
  onDrag(event: Pointer, node: ElementNode, nodes: ElementNode[]): void
  onDragStop(event: Pointer, node: ElementNode, nodes: ElementNode[]): void
}

/** Rendering is independent of document edits. Only a card being interacted with owns a native overlay. */
export function CanvasSurface(props: Props) {
  const { store, active, disabled } = props
  const controller = useCanvasViewport()
  const projectId = store.projectId
  const { client, privateViewEpoch, signedOut } = useConnection()
  const queries = useQueryClient()
  const privateEpoch = useSignal(privateViewEpoch)
  const loggedOut = useSignal(signedOut)
  const helpId = useId()
  const root = useRef<HTMLDivElement>(null)
  const overlay = useRef<HTMLDivElement>(null)
  const selection = useRef<HTMLDivElement>(null)
  const callbacks = useRef(props)
  callbacks.current = props
  const [hovered, setHovered] = useState<string | null>(null)
  const [overlayVersion, setOverlayVersion] = useState(0)
  const [failed, setFailed] = useState<string | null>(null)
  const hover = useRef<string | null>(null)
  const update = useRef<() => void>(() => {})
  const isActive = useRef(active)
  isActive.current = active
  const isDisabled = useRef(disabled)
  isDisabled.current = disabled
  const focused = useSignal(store.focusedName)
  const entering = useSignal(store.focusEntering)
  const [accessible, setAccessible] = useState<string | null>(null)
  const accessibleRef = useRef(accessible)
  accessibleRef.current = accessible
  const enabled = useRef<() => void>(() => {})

  useLayoutEffect(() => {
    hover.current = hovered
    update.current()
  }, [hovered, focused, entering, overlayVersion])
  useEffect(() => { enabled.current(); update.current() }, [active, disabled])

  useEffect(() => {
    const element = root.current
    if (!element) return
    let graphicsLost = false
    let stopped = false, frame = 0, settle = 0, app: Application<WebGLRenderer> | null = null, viewport: PixiViewport | null = null
    let scene: ReturnType<typeof createPixiScene> | null = null
    let unbind = () => {}, resize: ResizeObserver | null = null
    const stops: (() => void)[] = []
    let width = element.clientWidth, height = element.clientHeight, pointerDown = false, space = false
    let marquee: { start: { x: number; y: number }; additive: Set<string> } | null = null
    let drag: { id: number; start: { x: number; y: number }; nodes: ElementNode[]; begun: boolean } | null = null
    let clicked: { id: number; node: ElementNode | null; part: string; x: number; y: number; touch: boolean } | null = null
    let lastTap: { name: string; at: number; x: number; y: number } | null = null
    const touches = new Set<number>()
    let multitouch = false
    let nativePan: { point: { x: number; y: number }; camera: Viewport } | null = null
    let hoveredBefore = ""
    let drawnSelection = ""
    let renderFrames = 0
    let graphics: { backend: string; renderer: string; version: string; software: boolean } | null = null
    let sceneWork = false
    const requestedRuns = new Set<number>()

    const visible = () => isActive.current && tabShown.get() && !graphicsLost
    const interactions = () => controller.setInteractionEnabled(visible() && !isDisabled.current && !drag && !marquee)
    enabled.current = () => { scene?.setVisible(visible()); interactions(); if (!visible()) { cancelAnimationFrame(frame); frame = 0 } }
    const inBounds = (x: number, y: number) => { const box = element.getBoundingClientRect(); return x >= box.left && x <= box.right && y >= box.top && y <= box.bottom }
    const nodeAt = (x: number, y: number, excluded = new Set<string>()) => {
      if (!inBounds(x, y)) return null
      const hit = scene?.hitTest(controller.screenToWorld({ x, y }), excluded)
      if (hit) return controller.getNodes().find(node => node.data.name === hit.name) ?? null
      const native = document.elementFromPoint(x, y)?.closest<HTMLElement>(".canvas-node[data-id]")
      const id = native?.dataset.id
      if (id && element.contains(native) && !excluded.has(id)) return controller.getNode(id) ?? null
      return null
    }
    const nativeInput = (target: EventTarget | null) => target instanceof Element && Boolean(target.closest("button,input,select,textarea,[contenteditable],.canvas-control"))
    const announceSelection = () => {
      const nodes = controller.getNodes().filter(node => node.selected)
      const key = nodes.map(node => node.id).join(" ")
      if (key !== drawnSelection) { drawnSelection = key; callbacks.current.onSelectionChange({ nodes }) }
    }
    const positionOverlay = () => {
      const view = controller.getViewport()
      if (overlay.current) {
        overlay.current.style.transform = `translate(${view.x}px,${view.y}px) scale(${view.zoom})`
        overlay.current.style.setProperty("--name-zoom", String(nameZoomOf(view.zoom)))
      }
    }
    const draw = (now: number) => {
      frame = 0
      if (stopped || !app || !viewport || !scene || !visible()) return
      const was = controller.getViewport()
      viewport.update(16.667)
      const camera = controller.getViewport()
      const moved = was.x !== camera.x || was.y !== camera.y || was.zoom !== camera.zoom
      if (moved) settle = 2
      const names: SceneNode[] = []
      sceneWork = false
      const machines = queries.getQueryData(machinesQuery(client).queryKey)?.machines ?? []
      for (const node of controller.getNodes()) {
        const view = store.get(node.data.name)
        if (!view) continue
        let run = view.run
        const box = node.bounds
        const onscreen = box.x * camera.zoom + camera.x < width && (box.x + box.w) * camera.zoom + camera.x > 0 && box.y * camera.zoom + camera.y < height && (box.y + box.h) * camera.zoom + camera.y > 0
        const rendering = view.asset?.standing === "making" || view.queued || view.asked !== undefined || Boolean(view.glimpse)
        if (onscreen && rendering) sceneWork = true
        if (run && store.projectId && onscreen && rendering) {
          const query = runFullQuery(client, store.projectId, run.number, 0)
          const full = queries.getQueryData(query.queryKey)
          if (full) run = { ...full, ...run, request: full.request }
          else if (!requestedRuns.has(run.number)) {
            requestedRuns.add(run.number)
            void queries.ensureQueryData(query).then(invalidate, () => {})
          }
        }
        const machine = run?.machine ? machines.find(m => m.name === run.machine) : undefined
        const on = run ? waitsOnOf(ownActivity(machine, store.projectId), run) : undefined
        const tool = view.asset?.tool ? store.tools.get().get(view.asset.tool) : undefined
        const pkg = tool?.installs ?? tool?.package
        const installing = !run && pkg ? machines.flatMap(m => ownActivity(m, store.projectId)).find(o => o.kind === "install" && o.subject === pkg) : undefined
        names.push({ id: node.id, name: node.data.name, box, view, selected: Boolean(node.selected), runContext: run || installing ? { run, machine, on, installing } : undefined })
      }
      const hiddenNames = new Set<string>()
      if (hover.current) hiddenNames.add(store.resolveName(hover.current))
      if (store.focusedName.get() && !store.focusEntering.get()) hiddenNames.add(store.focusedName.get()!)
      scene.update({ nodes: names, camera, screen: { width, height, dpr: devicePixelRatio || 1 }, theme: theme.get() === "dark" ? "dark" : "paper", hiddenNames })
      const animating = scene.advance(now)
      app.render()
      if (app.renderer.context.isLost) return
      scene.didRender()
      renderFrames++
      if (!controller.hasDrawn && ((store.hasRead && store.empty) || scene.resourceStats().visibleCards > 0)) controller.markDrawn()
      positionOverlay()
      if (animating || moved || settle-- > 0) invalidate()
    }
    const invalidate = () => { if (!stopped && !frame && visible()) frame = requestAnimationFrame(draw) }
    update.current = invalidate
    store.canvasAt = (x, y) => {
      if (!visible()) return null
      const top = document.elementFromPoint(x, y)
      if (!top || !element.contains(top)) return null
      const excluded = new Set(controller.getNodes().filter(n => store.isDragging(n.data.name)).map(n => n.id))
      return nodeAt(x, y, excluded)?.data.name ?? null
    }

    const changeHover = (name: string | null) => {
      if (hover.current === name) return
      // Keep a native name field mounted until the name operation ends.
      if (hover.current && store.get(hover.current)?.draft !== undefined) return
      hover.current = name
      setHovered(name)
      invalidate()
    }
    const down = (event: PointerEvent) => {
      if (!visible() || callbacks.current.locked || event.button > 1) return
      if (event.pointerType === "touch") {
        touches.add(event.pointerId)
        if (touches.size > 1) {
          multitouch = true; clicked = null
          if (drag?.begun) store.dragDropped(drag.nodes)
          drag = null; marquee = null; interactions(); invalidate(); return
        }
      }
      pointerDown = true
      settle = 2
      const node = nodeAt(event.clientX, event.clientY)
      const hit = scene?.hitTest(controller.screenToWorld({ x: event.clientX, y: event.clientY }))
      const nativeTitle = event.target instanceof Element && Boolean(event.target.closest(".nm"))
      clicked = { id: event.pointerId, node, part: nativeTitle ? "title" : hit?.part ?? "body", x: event.clientX, y: event.clientY, touch: event.pointerType === "touch" }
      if (nativeInput(event.target)) return
      if (space || event.button === 1) clicked = null
      if (!isDisabled.current && (space || event.button === 1) && event.target !== app?.canvas) {
        nativePan = { point: { x: event.clientX, y: event.clientY }, camera: controller.getViewport() }
        controller.setInteractionEnabled(false); element.setPointerCapture(event.pointerId); event.preventDefault(); return
      }
      if (space || event.button === 1 || (!node && !event.shiftKey) || (event.pointerType === "touch" && !node?.selected)) { invalidate(); return }
      if (!node) {
        marquee = { start: controller.screenToWorld({ x: event.clientX, y: event.clientY }), additive: new Set(event.ctrlKey || event.metaKey ? controller.getNodes().filter(n => n.selected).map(n => n.id) : []) }
      } else {
        const modifiers = event.shiftKey || event.metaKey || event.ctrlKey
        const selected = new Set(controller.getNodes().filter(n => n.selected).map(n => n.id))
        if (modifiers) { selected.has(node.id) ? selected.delete(node.id) : selected.add(node.id); controller.select(selected) }
        else if (!selected.has(node.id)) controller.select([node.id])
        if (!modifiers && hit?.part !== "delete" && hit?.part !== "cancel") drag = { id: event.pointerId, start: controller.screenToWorld({ x: event.clientX, y: event.clientY }), nodes: controller.getNodes().filter(n => n.selected).map(n => ({ ...n, position: { ...n.position }, bounds: { ...n.bounds } })), begun: false }
      }
      interactions()
      if (event.pointerType === "touch") viewport?.plugins.resume("pinch")
      element.setPointerCapture(event.pointerId)
      if (event.pointerType !== "touch") event.stopPropagation()
      event.preventDefault()
      invalidate()
    }
    const move = (event: PointerEvent) => {
      if (callbacks.current.locked) return
      if (nativePan) { void controller.setViewport({ ...nativePan.camera, x: nativePan.camera.x + event.clientX - nativePan.point.x, y: nativePan.camera.y + event.clientY - nativePan.point.y }); return }
      if (touches.size > 1) return
      if (drag && drag.id !== event.pointerId) return
      if (marquee) {
        const p = controller.screenToWorld({ x: event.clientX, y: event.clientY }), a = marquee.start
        const box = { x: Math.min(a.x, p.x), y: Math.min(a.y, p.y), w: Math.abs(p.x - a.x), h: Math.abs(p.y - a.y) }
        const ids = controller.getNodes().filter(n => n.bounds.x < box.x + box.w && n.bounds.x + n.bounds.w > box.x && n.bounds.y < box.y + box.h && n.bounds.y + n.bounds.h > box.y).map(n => n.id)
        controller.select([...marquee.additive, ...ids])
        const camera = controller.getViewport()
        if (selection.current) { selection.current.hidden = false; Object.assign(selection.current.style, { left: `${box.x * camera.zoom + camera.x}px`, top: `${box.y * camera.zoom + camera.y}px`, width: `${box.w * camera.zoom}px`, height: `${box.h * camera.zoom}px` }) }
        invalidate(); return
      }
      if (drag) {
        const p = controller.screenToWorld({ x: event.clientX, y: event.clientY }), dx = p.x - drag.start.x, dy = p.y - drag.start.y
        if (!drag.begun && Math.hypot(dx, dy) * controller.getViewport().zoom < 3) return
        if (!drag.begun) { drag.begun = true; callbacks.current.onDragStart(event, drag.nodes[0], drag.nodes) }
        controller.upsertNodes(drag.nodes.map(n => ({ ...n, position: { x: n.position.x + dx, y: n.position.y + dy }, bounds: { ...n.bounds, x: n.bounds.x + dx, y: n.bounds.y + dy } })))
        const current = drag.nodes.map(n => controller.getNode(n.id)!).filter(Boolean)
        callbacks.current.onDrag(event, current[0], current)
        invalidate(); return
      }
      if (!pointerDown && event.pointerType === "mouse") changeHover(nodeAt(event.clientX, event.clientY)?.data.name ?? null)
    }
    const up = (event: PointerEvent) => {
      touches.delete(event.pointerId)
      if (multitouch) { if (!touches.size) multitouch = false; clicked = null; pointerDown = touches.size > 0; interactions(); invalidate(); return }
      if (drag && drag.id !== event.pointerId) return
      const held = drag, started = clicked?.id === event.pointerId ? clicked : null
      nativePan = null
      drag = null; marquee = null; clicked = null; pointerDown = false
      if (selection.current) selection.current.hidden = true
      if (held?.begun) {
        const nodes = held.nodes.map(n => controller.getNode(n.id)!).filter(Boolean)
        callbacks.current.onDragStop(event, nodes[0], nodes)
      } else if (started && Math.hypot(event.clientX - started.x, event.clientY - started.y) < 4 && !nativeInput(event.target)) {
        const node = started.node && controller.getNode(started.node.id)
        if (node) {
          callbacks.current.onNodeClick(event, node)
          if (started.part === "title" && !event.shiftKey && !event.ctrlKey && !event.metaKey) { store.openName(node.data.name); changeHover(node.data.name) }
          else if (started.part === "delete") store.remove([node.data.name])
          else if (started.part === "cancel") {
            const view = store.get(node.data.name), take = view && watched(view)
            if (view?.placing) store.cancel(node.data.name)
            else if (take) store.cancelTake(node.data.name, { id: take.id, run: take.run })
            else store.cancelRender(node.data.name)
          }
          else if (started.touch) { const at = performance.now(); if (lastTap?.name === node.data.name && at - lastTap.at < 500) callbacks.current.onNodeDoubleClick(event, node); lastTap = { name: node.data.name, at, x: event.clientX, y: event.clientY } }
        } else if (!event.shiftKey && !event.ctrlKey && !event.metaKey) { controller.select([]); callbacks.current.onPaneClick() }
      }
      interactions(); settle = 2; invalidate()
    }
    const cancel = (event: Event) => { touches.clear(); multitouch = false; nativePan = null; space = false; element.style.cursor = ""; if (drag?.begun) store.dragDropped(drag.nodes); drag = null; marquee = null; clicked = null; pointerDown = false; if (selection.current) selection.current.hidden = true; interactions(); invalidate(); void event }
    const double = (event: MouseEvent) => { if (!visible() || callbacks.current.locked) return; const node = nodeAt(event.clientX, event.clientY); if (node && !nativeInput(event.target)) callbacks.current.onNodeDoubleClick(event, node) }
    const menu = (event: MouseEvent) => { if (!visible() || callbacks.current.locked) return; const node = nodeAt(event.clientX, event.clientY); if (node && !nativeInput(event.target)) callbacks.current.onNodeContextMenu(event, node) }
    const keyDown = (event: KeyboardEvent) => {
      if (!visible() || isDisabled.current || nativeInput(event.target)) return
      if (event.code === "Space") { space = true; element.style.cursor = "grab"; event.preventDefault() }
      if (event.target !== element || !["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Enter"].includes(event.key)) return
      const nodes = controller.getNodes(), index = nodes.findIndex(n => n.data.name === accessibleRef.current)
      if (event.key === "Enter") { const node = nodes[index]; if (node) store.openHere(node.data.name); return }
      const next = nodes[Math.max(0, Math.min(nodes.length - 1, index + (event.key === "ArrowLeft" || event.key === "ArrowUp" ? -1 : 1)))]
      if (next) { event.preventDefault(); controller.select([next.id]); setAccessible(next.data.name); void controller.fitView({ bounds: [next.bounds], maxZoom: controller.getViewport().zoom }) }
    }
    const keyUp = (event: KeyboardEvent) => { if (event.code === "Space") { space = false; element.style.cursor = "" } }
    const wheel = (event: WheelEvent) => {
      if (!visible() || isDisabled.current || nativeInput(event.target)) return
      if (event.target !== app?.canvas) {
        event.preventDefault()
        const at = controller.getViewport(), p = controller.screenToWorld({ x: event.clientX, y: event.clientY })
        const delta = event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? height : 1
        if (event.ctrlKey || event.metaKey) {
          const zoom = Math.max(ZOOMS[0], Math.min(ZOOMS[ZOOMS.length - 1], at.zoom * Math.exp(-event.deltaY * delta * .005)))
          void controller.setViewport({ x: at.x + p.x * (at.zoom - zoom), y: at.y + p.y * (at.zoom - zoom), zoom })
        } else void controller.setViewport({ ...at, x: at.x - event.deltaX * delta, y: at.y - event.deltaY * delta })
      }
      pointerDown = false; changeHover(null); settle = 3; invalidate()
    }
    const leave = () => { if (!pointerDown) changeHover(null) }
    const handlers = { pointerdown: down, pointermove: move, pointerup: up, pointercancel: cancel, dblclick: double, contextmenu: menu, wheel, pointerleave: leave }
    for (const [name, handler] of Object.entries(handlers)) element.addEventListener(name, handler as EventListener, { capture: name === "pointerdown", passive: false })
    window.addEventListener("blur", cancel)
    window.addEventListener("pointerup", up)
    window.addEventListener("pointercancel", cancel)
    window.addEventListener("keydown", keyDown)
    window.addEventListener("keyup", keyUp)

    const inspection = {
      camera: () => ({ ...controller.getViewport() }),
      hitTest: (point: { x: number; y: number }) => {
        const hit = scene?.hitTest(controller.screenToWorld(point))
        if (hit) return hit
        const node = nodeAt(point.x, point.y)
        return node ? { name: node.data.name, part: "body" } : null
      },
      selectedNames: () => controller.getNodes().filter(n => n.selected).map(n => n.data.name),
      names: () => controller.getNodes().map(n => n.data.name),
      card: (name: string) => {
        const node = controller.getNodes().find(n => n.data.name === name)
        if (!node) return null
        const box = node.bounds, a = controller.worldToScreen({ x: box.x, y: box.y }), zoom = controller.getViewport().zoom
        const view = store.get(name)
        const body = view ? pictureBox({ id: node.id, name, box, view, selected: Boolean(node.selected) }) : { x: box.x, y: box.y + HEAD, w: box.w, h: Math.max(0, box.h - HEAD) }
        const bodyAt = controller.worldToScreen(body)
        const drawn = scene?.inspect(name)
        const project = <T extends { x: number; y: number; width: number; height: number }>(box: T | null) => box && { ...box, ...controller.worldToScreen(box), width: box.width * zoom, height: box.height * zoom }
        const rendered = drawn ? { ...drawn, references: drawn.references.map(reference => project(reference)!), chips: drawn.chips.map(chip => project(chip)!), stateDot: project(drawn.stateDot), meter: project(drawn.meter), wave: project(drawn.wave), controls: drawn.controls.map(control => ({ ...control, ...controller.worldToScreen(control), width: control.width * zoom, height: control.height * zoom })), body: { ...drawn.body, ...controller.worldToScreen(drawn.body), width: drawn.body.width * zoom, height: drawn.body.height * zoom }, labels: Object.fromEntries(Object.entries(drawn.labels).map(([key, label]) => [key, { ...label, ...controller.worldToScreen(label), width: label.width * zoom, height: label.height * zoom }])) } : null
        return { rendered, position: { ...node.position }, bounds: { x: a.x, y: a.y, width: box.w * zoom, height: box.h * zoom }, body: { x: bodyAt.x, y: bodyAt.y, width: body.w * zoom, height: body.h * zoom }, title: { x: a.x, y: a.y, width: box.w * zoom, height: HEAD * zoom } }
      },
      resources: () => scene ? { ...scene.resourceStats(), graphics, frames: renderFrames, scheduledFrame: Boolean(frame), automaticTicker: false } : null,
    }
    Object.defineProperty(element, "canvasInspection", { value: inspection, configurable: true })
    stops.push(controller.subscribeCamera(() => { callbacks.current.onMoveStart?.(); positionOverlay(); settle = 2; invalidate() }), controller.subscribeCameraEnd(view => callbacks.current.onMoveEnd(null, view)), controller.subscribeNodes(() => { announceSelection(); setOverlayVersion(v => v + 1); invalidate() }), store.sceneRevision.subscribe(() => {
      const name = hover.current
      if (name && store.get(name)?.name !== hoveredBefore) { hoveredBefore = store.get(name)?.name ?? ""; setOverlayVersion(v => v + 1) }
      // A fresh import owns a native name field until it is accepted.
      if (!name) { const draft = controller.getNodes().find(n => store.get(n.data.name)?.draft !== undefined); if (draft) changeHover(draft.data.name) }
      invalidate()
    }), store.tools.subscribe(invalidate), theme.subscribe(invalidate), tabShown.subscribe(() => { scene?.setVisible(visible()); interactions(); if (!visible()) { cancel(new Event("visibilitychange")); cancelAnimationFrame(frame); frame = 0 } else invalidate() }), queries.getQueryCache().subscribe(event => {
      if (event.type !== "updated" || event.action.type !== "success") return
      const key = event.query.queryKey
      if ((key[0] === "project-run" && key[1] === store.projectId) || (key[0] === "machines" && sceneWork)) invalidate()
    }))

    void (async () => {
      const created = new Application<WebGLRenderer>()
      await created.init({ preference: ["webgl"], failIfMajorPerformanceCaveat: true, width: Math.max(1, width), height: Math.max(1, height), resolution: devicePixelRatio || 1, autoDensity: true, autoStart: false, sharedTicker: false, backgroundAlpha: 0, antialias: true, gcActive: false })
      if (stopped) { created.destroy(true, { children: true }); return }
      app = created
      const gl = created.renderer.gl, info = gl.getExtension("WEBGL_debug_renderer_info")
      const backend = String(info ? gl.getParameter(info.UNMASKED_RENDERER_WEBGL) : gl.getParameter(gl.RENDERER))
      graphics = { backend: "webgl", renderer: backend, version: String(gl.getParameter(gl.VERSION)), software: /swiftshader|llvmpipe|lavapipe|softpipe|swrast|software/i.test(backend) }
      EventsTicker.removeTickerListener()
      // The scene owns resource eviction; no scheduled renderer GC needs a perpetual system ticker.
      created.renderer.scheduler.destroy()
      created.canvas.className = "canvas-pixels canvas-pane"
      const lost = (event: Event) => {
        event.preventDefault(); graphicsLost = true; cancelAnimationFrame(frame); frame = 0
        scene?.setVisible(false); interactions(); setFailed("Graphics context lost; waiting for the browser to restore it.")
      }
      const restored = () => { graphicsLost = false; setFailed(null); scene?.setVisible(visible()); interactions(); invalidate() }
      created.canvas.addEventListener("webglcontextlost", lost)
      created.canvas.addEventListener("webglcontextrestored", restored)
      stops.push(() => { created.canvas.removeEventListener("webglcontextlost", lost); created.canvas.removeEventListener("webglcontextrestored", restored) })
      element.prepend(created.canvas)
      viewport = new PixiViewport({ screenWidth: width, screenHeight: height, events: created.renderer.events, noTicker: true, passiveWheel: false, ignoreWheelFromOtherElements: true })
      viewport.drag({ wheel: true }).pinch().wheel({ wheelZoom: false, trackpadPinch: true, smooth: false }).clampZoom({ minScale: ZOOMS[0], maxScale: ZOOMS[ZOOMS.length - 1] })
      created.stage.addChild(viewport)
      const world = new Container({ eventMode: "none", interactiveChildren: false })
      viewport.addChild(world)
      scene = createPixiScene({ world, store, client, invalidate })
      scene.setVisible(visible())
      unbind = controller.bindCamera(element, viewport)
      callbacks.current.onReady?.()
      interactions()
      resize = new ResizeObserver(() => { width = element.clientWidth; height = element.clientHeight; if (width > 0 && height > 0 && app && viewport) { app.renderer.resize(width, height, devicePixelRatio || 1); viewport.resize(width, height); invalidate() } })
      resize.observe(element)
      setFailed(null)
      invalidate()
    })().catch(error => { if (!stopped) setFailed(error instanceof Error ? error.message : String(error)) })

    return () => {
      stopped = true; cancelAnimationFrame(frame); resize?.disconnect(); unbind(); for (const stop of stops) stop()
      for (const [name, handler] of Object.entries(handlers)) element.removeEventListener(name, handler as EventListener, { capture: name === "pointerdown" })
      window.removeEventListener("blur", cancel)
      window.removeEventListener("pointerup", up); window.removeEventListener("pointercancel", cancel)
      window.removeEventListener("keydown", keyDown); window.removeEventListener("keyup", keyUp)
      scene?.destroy(); app?.destroy(true, { children: true })
      store.canvasAt = () => null
      delete (element as HTMLDivElement & { canvasInspection?: unknown }).canvasInspection
      update.current = () => {}; enabled.current = () => {}
    }
  }, [controller, store, projectId, client, queries, privateEpoch, loggedOut])

  const node = hovered ? controller.getNodes().find(n => n.data.name === store.resolveName(hovered)) : undefined
  const native = node && focused !== node.data.name ? node : null
  const view = native && store.get(native.data.name)
  return <div ref={root} className="canvas-root" tabIndex={0} role="region" aria-label="Project canvas" aria-describedby={helpId}>
    {props.children}
    <div ref={overlay} className="canvas-world">
      {native && view && <div className={`canvas-node ${native.selected ? "selected" : ""}`} data-id={native.id} style={{ left: native.bounds.x, top: native.bounds.y, width: native.bounds.w, height: native.bounds.h }}>
        {native.type === "entity" ? <EntityCard data={native.data} /> : <Card data={native.data} selected={native.selected} />}
      </div>}
    </div>
    <div ref={selection} className="canvas-selection" hidden />
    <span id={helpId} className="sr-only">Use arrow keys to select a card, Enter to open it, and Escape to return. Drag empty space to pan; Shift-drag selects a rectangle.</span>
    <span className="sr-only" aria-live="polite">{accessible ?? ""}</span>
    {failed && <div className="empty-canvas" role="alert"><p>Couldn't start the canvas graphics: {failed}</p></div>}
  </div>
}
