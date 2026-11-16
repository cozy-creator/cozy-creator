// CSP-safe shader uniforms (no eval), and Pixi's optional math methods, which its typings declare whether or not they're installed.
import "pixi.js/unsafe-eval"
import "pixi.js/math-extras"
import { autoDetectRenderer, Container, Graphics, Rectangle, type Renderer, Sprite, Text, Texture, Ticker } from "pixi.js"
import { Viewport as PixiViewport } from "pixi-viewport"
import { type ReactNode, useEffect, useId, useLayoutEffect, useRef, useState } from "react"
import { useQueryClient } from "@tanstack/react-query"
import { useConnection } from "../../app/Connection"
import { machinesQuery, runFullQuery } from "../../api/queries"
import { ownActivity, waitsOnOf } from "../../panels/machines/activity"
import { useSignal, tabShown } from "../../ui/signal"
import { theme } from "../../ui/theme"
import { Asking } from "../Asking"
import type { CanvasStore, ElementNode } from "../CanvasStore"
import { carried } from "../checkpoints"
import { NameField } from "../NameField"
import { HEAD, watched } from "../shown"
import { viewState } from "../StateDot"
import type { Viewport } from "./Camera"
import { MAX_ZOOM, MIN_ZOOM } from "./Camera"
import { hitOf } from "./CardView"
import { useBoard } from "./context"
import { inspection } from "./inspect"
import { createInteraction } from "./Interaction"
import { Media } from "./Media"
import { pictureBox, type RunContext } from "./presentation"
import { Scene } from "./Scene"
import { forgetMeasurements, onFontsLoaded } from "./text"

type Pointer = MouseEvent | PointerEvent

/** Drawn on the CPU: WebGL in software (SwiftShader, llvmpipe), or Pixi's canvas renderer. */
function softwareOf(renderer: Renderer): boolean {
  if (!("gl" in renderer)) return true
  const gl = (renderer as unknown as { gl: WebGLRenderingContext }).gl
  const info = gl.getExtension("WEBGL_debug_renderer_info")
  return /swiftshader|llvmpipe|lavapipe|softpipe|swrast|software/i.test(String(info ? gl.getParameter(info.UNMASKED_RENDERER_WEBGL) : gl.getParameter(gl.RENDERER)))
}

async function startRenderer(width: number, height: number): Promise<Renderer> {
  const make = (antialias: boolean) => {
    const options = { width: Math.max(1, width), height: Math.max(1, height), resolution: devicePixelRatio || 1, autoDensity: true, backgroundAlpha: 0, antialias, failIfMajorPerformanceCaveat: false }
    return autoDetectRenderer({ ...options, preference: ["webgl", "canvas"] }).catch(() => autoDetectRenderer({ ...options, preference: ["canvas"] }))
  }
  let made = await make(true)
  // A CPU rasterizes every sample of a multisampled frame (cl-1018: a third of each frame's time in CI's SwiftShader).
  if ("gl" in made && softwareOf(made)) { made.destroy({ removeView: true }); made = await make(false) }
  // Its shaders compiled now, while nothing waits on it: the first frame with cards stalled ~150 ms on them in software (cl-1018).
  const warm = new Container()
  warm.addChild(new Graphics().rect(0, 0, 2, 2).fill(0x808080), new Sprite(Texture.WHITE), new Text({ text: "a" }))
  made.render(warm)
  warm.destroy({ children: true })
  // ...and waited for here: left queued, the GPU was still compiling them when the canvas mounted, and the first call that waits on it
  // (its resize) held the page 180 ms then (e2e 43784566887).
  if ("gl" in made) (made as unknown as { gl: WebGLRenderingContext }).gl.getError()
  return made
}
/** A renderer begun as the page loads, before any project is read (cl-1018: one begun as the canvas mounts held the open's first frame
 *  60–90 ms past its state): the first canvas takes it. It holds nothing of any project until then. */
let early: Promise<Renderer> | null = startRenderer(innerWidth, innerHeight)
early.catch(() => {})
export interface StageProps {
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
  onDragCancel(nodes: ElementNode[]): void
}

/** The canvas: one renderer, one on-demand ticker, the scene, the camera, input, and DOM only where text is typed. */
export function Stage(props: StageProps) {
  const { store, active, disabled } = props
  const board = useBoard()
  const { client, bridge, privateViewEpoch, signedOut } = useConnection()
  const queries = useQueryClient()
  useSignal(privateViewEpoch)
  const projectEpoch = store.projectId ? bridge.projectEpoch(store.projectId) : 0
  const loggedOut = useSignal(signedOut)
  const helpId = useId()
  const root = useRef<HTMLDivElement>(null)
  const world = useRef<HTMLDivElement>(null)
  const latest = useRef(props)
  latest.current = props
  const live = useRef({ invalidate: () => {}, enable: () => {} })
  const [failed, setFailed] = useState<string | null>(null)
  const [lostGraphics, setLostGraphics] = useState(false)
  const [said, setSaid] = useState("")
  const [overlays, setOverlays] = useState<{ name: string; draft?: string; asking?: NonNullable<ReturnType<CanvasStore["get"]>>["asking"]; box: { x: number; y: number; w: number; under: number } }[]>([])
  const [cards, setCards] = useState<{ name: string; label: string; selected: boolean; cancel: boolean }[]>([])
  const focused = useSignal(store.focusedName)
  const entering = useSignal(store.focusEntering)
  useLayoutEffect(() => { live.current.enable(); live.current.invalidate() }, [active, disabled, focused, entering])

  useEffect(() => {
    const element = root.current
    if (!element) return
    let stopped = false, lost = false
    let renderer: Renderer | null = null, viewport: PixiViewport | null = null, scene: Scene | null = null
    let interaction: ReturnType<typeof createInteraction> | null = null
    let wakeTimer: ReturnType<typeof setTimeout> | undefined
    let dirty = true, animating = false, frames = 0, width = element.clientWidth, height = element.clientHeight
    const stops: (() => void)[] = []
    const stage = new Container({ eventMode: "static" })
    const ticker = new Ticker()
    ticker.autoStart = false
    const media = new Media({ client, project: store.projectId ?? "", changed: () => invalidate() })
    const visible = () => latest.current.active && tabShown.get() && !lost
    const invalidate = () => { dirty = true; if (!stopped && renderer && visible() && !ticker.started) ticker.start() }
    const enable = () => {
      const on = visible()
      media.setVisible(on)
      board.camera.setInteractionEnabled(on && !latest.current.disabled)
      if (!on) interaction?.cancel()
    }
    live.current = { invalidate, enable }
    const software = { value: false }

    // Run context for a card's progress line: its machine, what it waits on, an install for its tool (RunFace's inputs).
    const asked = new Set<number>()
    const merged = new WeakMap<object, { full: object; run: NonNullable<ReturnType<CanvasStore["get"]>>["run"] }>()
    // Memoized by its inputs' identities: a frame recomputes no card's context unless its view, the machines or its run's read changed.
    const contexts = new WeakMap<object, { machines: unknown; full: unknown; tools: unknown; value: RunContext | undefined }>()
    const contextOf = (view: NonNullable<ReturnType<CanvasStore["get"]>>): RunContext | undefined => {
      const machinesData = queries.getQueryData(machinesQuery(client).queryKey)
      const fullData = view.run && store.projectId ? queries.getQueryData(runFullQuery(client, store.projectId, view.run.number, 0).queryKey) : undefined
      const held = contexts.get(view)
      if (held && held.machines === machinesData && held.full === fullData && held.tools === store.tools.get()) return held.value
      const value = contextFresh(view)
      contexts.set(view, { machines: machinesData, full: fullData, tools: store.tools.get(), value })
      return value
    }
    const contextFresh = (view: NonNullable<ReturnType<CanvasStore["get"]>>): RunContext | undefined => {
      const machines = queries.getQueryData(machinesQuery(client).queryKey)?.machines ?? []
      let run = view.run
      const rendering = view.asset?.standing === "making" || view.queued || view.asked !== undefined || Boolean(view.glimpse)
      if (run && store.projectId && rendering) {
        const query = runFullQuery(client, store.projectId, run.number, 0)
        const full = queries.getQueryData(query.queryKey)
        // Only what the live run lacks: its request. The live run's own fields (queue position, stage) always win.
        if (full) {
          const held = merged.get(run)
          if (held?.full === full) run = held.run
          else { const live = run; run = { ...live, request: full.request }; merged.set(live, { full, run }) }
        }
        else if (!asked.has(run.number)) { asked.add(run.number); void queries.ensureQueryData(query).then(invalidate, () => {}) }
      }
      const machine = run?.machine ? machines.find((m) => m.name === run.machine) : undefined
      const on = run ? waitsOnOf(ownActivity(machine, store.projectId), run) : undefined
      const tool = view.asset?.tool ? store.tools.get().get(view.asset.tool) : undefined
      const pkg = tool?.installs ?? tool?.package
      const installing = !run && pkg ? machines.flatMap((m) => ownActivity(m, store.projectId)).find((o) => o.kind === "install" && o.subject === pkg) : undefined
      return run || installing ? { run, machine, on, installing } : undefined
    }

    const placeWorld = () => {
      const view = board.camera.getViewport()
      if (world.current) world.current.style.transform = `translate(${view.x}px,${view.y}px) scale(${view.zoom})`
    }

    const tick = () => {
      if (stopped || !renderer || !viewport || !scene) return
      if (!visible()) { ticker.stop(); return }
      let moving = false
      try { viewport.update(ticker.deltaMS); moving = board.camera.sync() } catch (error) { console.error("Camera:", error) }
      if (moving) placeWorld()
      if (dirty || moving || animating) {
        dirty = false
        try {
          const hidden = new Set<string>()
          const focus = store.focusedName.get()
          if (focus && !store.focusEntering.get()) hidden.add(store.resolveName(focus))
          const result = scene.update({ moving: board.camera.isMoving, camera: board.camera.getViewport(), width, height, dpr: devicePixelRatio || 1, settled: !board.camera.isMoving, theme: theme.get() === "dark" ? "dark" : "paper", now: performance.now(), hidden, offering: element.closest(".stage")?.hasAttribute("data-offering") ?? false })
          animating = result.animating
          clearTimeout(wakeTimer)
          if (result.wake !== null) wakeTimer = setTimeout(invalidate, result.wake)
        } catch (error) { console.error("Scene:", error) }
      }
      // A frame always presents, whatever a card did: the canvas never freezes.
      renderer.render(stage)
      frames++
      try { scene.didRender() } catch (error) { console.error("Seen:", error) }
      if (!board.camera.hasDrawn && ((store.hasRead && store.empty) || [...scene.views()].length > 0)) board.camera.markDrawn()
      if (!dirty && !moving && !animating && !board.camera.isMoving) ticker.stop()
    }
    ticker.add(tick)

    const ready = async () => {
      const taken = early
      early = null
      const made = await (taken ?? startRenderer(width, height)).catch((error) => { if (!taken) throw error; return startRenderer(width, height) })
      if (stopped) { made.destroy({ removeView: true }); return }
      if (taken) made.resize(Math.max(1, width), Math.max(1, height), devicePixelRatio || 1)
      renderer = made
      software.value = softwareOf(made)
      // No global move walks every card on every pointer move: Interaction listens to the window while it holds a press.
      made.events.features.globalMove = false
      made.canvas.className = "canvas-pixels canvas-pane"
      element.prepend(made.canvas)
      const contextLost = (event: Event) => { event.preventDefault(); lost = true; setLostGraphics(true); enable() }
      const contextRestored = () => { lost = false; setLostGraphics(false); enable(); scene?.redrawAll(); invalidate() }
      made.canvas.addEventListener("webglcontextlost", contextLost)
      made.canvas.addEventListener("webglcontextrestored", contextRestored)
      stops.push(() => { made.canvas.removeEventListener("webglcontextlost", contextLost); made.canvas.removeEventListener("webglcontextrestored", contextRestored) })
      viewport = new PixiViewport({ screenWidth: width, screenHeight: height, events: made.events, noTicker: true, passiveWheel: false, ignoreWheelFromOtherElements: true, disableOnContextMenu: true })
      viewport.enableRenderGroup()
      viewport.cursor = "grab"
      viewport.drag({ wheel: true }).pinch().wheel({ wheelZoom: false, trackpadPinch: true, smooth: false }).decelerate({ friction: 0.93 }).clampZoom({ minScale: MIN_ZOOM, maxScale: MAX_ZOOM })
      // A gesture moves the viewport between ticks; it starts one.
      viewport.on("moved", invalidate).on("zoomed", invalidate)
      stage.hitArea = new Rectangle(0, 0, width, height)
      stage.addChild(viewport)
      const layer = new Container({ eventMode: "passive" })
      viewport.addChild(layer)
      scene = new Scene({ world: layer, board, store, client, media, software: software.value, invalidate, contextOf })
      board.camera.wake = invalidate
      stops.push(board.camera.bind(element, viewport), () => { board.camera.wake = () => {} })
      interaction = createInteraction({
        root: element, stage, viewport, camera: board.camera, board, store, hitOf,
        scene: {
          setHover: (hit) => {
            scene?.setHover(hit)
            // A truncated name reads whole on hover (zoom-never-moves-nodes): the browser's own tooltip.
            const view = hit?.part === "title" ? store.get(hit.name) : undefined
            if (view) element.title = view.name
            else element.removeAttribute("title")
          }, setResize: (name, label) => scene?.setResize(name, label), setMarquee: (box) => scene?.setMarquee(box) },
        props: () => latest.current, invalidate,
      })
      const resize = new ResizeObserver(() => {
        width = element.clientWidth; height = element.clientHeight
        board.camera.measure()
        if (width > 0 && height > 0 && renderer && viewport) { renderer.resize(width, height, devicePixelRatio || 1); viewport.resize(width, height); stage.hitArea = new Rectangle(0, 0, width, height); invalidate() }
      })
      resize.observe(element)
      stops.push(() => resize.disconnect())
      // A move to a screen of another density re-renders sharp.
      let density: MediaQueryList | null = null
      const watchDensity = () => {
        density?.removeEventListener("change", densityChanged)
        density = matchMedia(`(resolution: ${devicePixelRatio || 1}dppx)`)
        density.addEventListener("change", densityChanged)
      }
      const densityChanged = () => { renderer?.resize(width, height, devicePixelRatio || 1); watchDensity(); invalidate() }
      watchDensity()
      stops.push(() => density?.removeEventListener("change", densityChanged))
      enable()
      // One frame now: Pixi's events hit-test what was last rendered, and a pointer can move before the first tick.
      renderer.render(stage)
      latest.current.onReady?.()
      setFailed(null)
      invalidate()
    }
    void ready().catch((error) => { if (!stopped) setFailed(error instanceof Error ? error.message : String(error)) })

    // What the page shows outside the GPU: the name field and Asking over their boxes, and the accessible list of every card.
    let listKey = ""
    const publishDom = () => {
      const next: typeof overlays = []
      for (const node of board.getNodes()) {
        const view = store.get(node.data.name)
        if (!view) continue
        const body = pictureBox({ id: node.id, name: node.data.name, box: node.bounds, view, selected: Boolean(node.selected) })
        const box = { x: node.bounds.x, y: node.bounds.y, w: node.bounds.w, under: body.y + body.h - node.bounds.y }
        if (view.draft !== undefined || (view.asking && !view.here)) next.push({ name: node.data.name, draft: view.draft, asking: view.here ? undefined : view.asking, box })
      }
      setOverlays((previous) => JSON.stringify(previous) === JSON.stringify(next) ? previous : next)
      const list = board.getNodes().map((node) => {
        const view = store.get(node.data.name)
        const state = view ? viewState(view) : null
        const going = Boolean(view?.asset && !view.placing && (view.asset.standing === "making" || view.queued || view.asked !== undefined))
        return { name: node.data.name, label: `${view?.name ?? node.data.name}${state ? `, ${state}` : ""}`, selected: Boolean(node.selected), cancel: going }
      })
      const key = JSON.stringify(list)
      if (key !== listKey) { listKey = key; setCards(list) }
    }
    let announced = ""
    const announce = () => {
      const nodes = board.getNodes().filter((node) => node.selected)
      const key = nodes.map((node) => node.id).join(" ")
      if (key !== announced) {
        announced = key
        latest.current.onSelectionChange({ nodes })
        // The selected box, said by name to a screen reader as the arrows walk the boxes.
        const first = nodes[0]
        setSaid(first ? store.get(first.data.name)?.name ?? first.data.name : "")
      }
    }
    stops.push(
      board.subscribeNodes(() => { announce(); publishDom(); invalidate() }),
      board.camera.subscribeCamera(() => { placeWorld(); latest.current.onMoveStart?.() }),
      board.camera.subscribeCameraEnd((view) => { latest.current.onMoveEnd(null, view); invalidate() }),
      store.sceneRevision.subscribe(() => { publishDom(); invalidate() }),
      store.tools.subscribe(invalidate), store.names.subscribe(invalidate), theme.subscribe(invalidate), carried.subscribe(invalidate),
      tabShown.subscribe(() => { enable(); invalidate() }),
      onFontsLoaded(() => { scene?.redrawAll(); invalidate() }),
      queries.getQueryCache().subscribe((event) => {
        if (event.type !== "updated" || event.action.type !== "success") return
        const key = event.query.queryKey
        if ((key[0] === "project-run" && key[1] === store.projectId) || key[0] === "machines") invalidate()
      }),
    )
    publishDom()
    placeWorld()
    // What lies at a client point for a drop: the topmost box whose body is there, past any box being carried.
    store.canvasAt = (x, y) => {
      if (!visible()) return null
      const top = document.elementFromPoint(x, y)
      if (!top || !element.contains(top)) return null
      const point = board.camera.screenToWorld({ x, y })
      const under = board.query({ x: point.x, y: point.y, w: 0, h: 0 }).reverse()
      const found = under.sort((a, b) => Number(Boolean(b.selected)) - Number(Boolean(a.selected))).find((node) => {
        const view = store.get(node.data.name)
        if (!view || store.isDragging(node.data.name)) return false
        const body = pictureBox({ id: node.id, name: node.data.name, box: node.bounds, view, selected: Boolean(node.selected) })
        return point.x >= body.x && point.x <= body.x + body.w && point.y >= node.bounds.y && point.y <= body.y + body.h
      })
      return found?.data.name ?? null
    }
    stops.push(() => { store.canvasAt = () => null })
    Object.defineProperty(element, "canvasInspection", {
      configurable: true,
      value: inspection({ board, store, scene: () => scene, interaction: () => interaction, media, frames: () => frames, ticking: () => ticker.started, renderer: () => renderer, software: () => software.value }),
    })
    return () => {
      stopped = true
      clearTimeout(wakeTimer)
      ticker.destroy()
      interaction?.destroy()
      for (const stop of stops) stop()
      scene?.destroy()
      media.destroy()
      stage.destroy({ children: true })
      renderer?.destroy({ removeView: true })
      forgetMeasurements()
      delete (element as HTMLDivElement & { canvasInspection?: unknown }).canvasInspection
      live.current = { invalidate: () => {}, enable: () => {} }
    }
  }, [board, store, client, queries, projectEpoch, loggedOut])

  return <div ref={root} className="canvas-root" tabIndex={0} role="region" aria-label="Project canvas" aria-describedby={helpId}>
    {props.children}
    <div ref={world} className="canvas-world">
      {overlays.map(({ name, draft, asking, box }) => <div key={name} className="canvas-over" style={{ left: box.x, top: box.y, width: box.w }}>
        {draft !== undefined && <div className="canvas-name card"><div className="head" style={{ height: HEAD }}>
          <NameField name={name} draft={draft} onCancel={() => { if (store.get(name)?.placing) store.cancel(store.resolveName(name)); else store.patch(name, { draft: undefined }) }} />
        </div></div>}
        {asking && <div className="rest-asking canvas-control" style={{ top: box.under + 4 }}><Asking name={name} fit={asking.fit} /></div>}
      </div>)}
    </div>
    <ul className="canvas-cards sr-only" role="listbox" aria-label="Boxes on the canvas" aria-multiselectable="true">
      {cards.map((card) => <li key={card.name} role="option" data-asset={card.name} aria-selected={card.selected}>{card.label}</li>)}
    </ul>
    {/* What the drawn Cancel does, for a keyboard or a screen reader. */}
    <div className="canvas-actions sr-only" role="group" aria-label="Renders on the canvas">
      {cards.filter((card) => card.cancel).map((card) => <button key={card.name} type="button" className="canvas-control" aria-label={`Cancel ${card.name}'s render`} onClick={() => {
        const view = store.get(card.name), take = view && watched(view)
        if (take) store.cancelTake(card.name, { id: take.id, run: take.run })
        else store.cancelRender(card.name)
      }}>Cancel</button>)}
    </div>
    <span id={helpId} className="sr-only">Use arrow keys to select a box, Enter to open it, and Escape to return. Drag empty space to pan; Shift-drag selects a rectangle.</span>
    <span className="sr-only" aria-live="polite">{said}</span>
    {lostGraphics && <div className="empty-canvas" role="alert"><p>Graphics context lost; waiting for the browser to restore it.</p></div>}
    {failed && <div className="empty-canvas" role="alert"><p>This browser couldn't draw the canvas ({failed}). Turn on hardware acceleration in its settings, or open this page in another browser.</p></div>}
  </div>
}
