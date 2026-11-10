import { useQuery, useQueryClient } from "@tanstack/react-query"
import {
  type Edge,
  type OnSelectionChangeParams,
  ReactFlow,
  ReactFlowProvider,
  SelectionMode,
  useReactFlow,
  useStore,
  useStoreApi,
  type Viewport,
} from "@xyflow/react"
import { type ChangeEvent, type DragEvent, type MouseEvent as ReactMouseEvent, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react"
import { createPortal } from "react-dom"
import { useMatch, useNavigate } from "react-router-dom"
import { ApiError } from "../api/client"
import { lost } from "../api/outbox"
import { canvasQuery, machinesQuery, projectQuery, toolsQuery } from "../api/queries"
import { useTo } from "../app/project"
import { useConnection } from "../app/Connection"
import { loadMore, loadSelection, loadToolsShut, loadViewport, saveMore, saveSelection, saveToolsShut, saveViewport } from "../app/view"
import type { Tool } from "../api/calcifer"
import { Copyable } from "../ui/Copyable"
import { dismiss, say } from "../ui/notes"
import { useSignal } from "../ui/signal"
import { AssetMenu } from "./AssetMenu"
import { Asking } from "./Asking"
import { CARD_WIDTH, CanvasStore, type ElementNode } from "./CanvasStore"
import { Card } from "./Card"
import { CanvasStoreProvider, useCanvasStore, useElement } from "./context"
import { carriedAt, LET_GO, type Load, type LetGo } from "./carry"
import { boxAt, carriedOver, letGo } from "./checkpoints"
import { copyBoxes, ownCopy } from "./copy"
import { EntityCard } from "./EntityCard"
import { HistoryControls } from "./HistoryControls"
import { Placer } from "./place"
import { Dots, Sheet } from "./Sheet"
import { HANG, madeHere, shownOf } from "./shown"
import { noHash } from "./text"
import { ToolPanel } from "./ToolPanel"
import { Viewer } from "./Viewer"
import { keys, stepZoom, ZOOMS, ZoomControl, type ZoomItem } from "./ZoomControl"
import { MOST_HERE } from "./readableSurface"

const nodeTypes = { element: Card, entity: EntityCard }
/** The room left on the canvas by what floats over it: the tools at the left, the top bar, and the zoom control in the bottom corner. */
const clearOf = () => ({
  left: (document.querySelector(".tools")?.getBoundingClientRect().right ?? 0) + 24,
  top: (document.querySelector(".top")?.getBoundingClientRect().bottom ?? 0) + 12,
  right: 24,
  bottom: Math.max(24, innerHeight - (document.querySelector(".zoom")?.getBoundingClientRect().top ?? innerHeight) + 12),
})
/** A fit, clear of what floats over the canvas. */
const fitOver = () => {
  const { left, top, right, bottom } = clearOf()
  return { maxZoom: 1, padding: { top: `${top}px`, right: `${right}px`, bottom: `${bottom}px`, left: `${left}px` } as const }
}

const [MIN_ZOOM, MAX_ZOOM] = [ZOOMS[0], ZOOMS[ZOOMS.length - 1]]
/** A window this narrow has no room for the tools beside a box being worked on: they fold away, and come back after. */
const NARROW = 760
/** How long the canvas takes to draw in on a box, or go back. */
const HERE_MS = 260

type Pointing = ReactMouseEvent | MouseEvent | TouchEvent
const pointOf = (event: Pointing) => ("clientX" in event ? event : event.changedTouches[0])

const ACCEPT = "image/*,video/*,audio/*"
const MULTI = ["Shift", "Meta", "Control"]
const hasFiles = (event: DragEvent) => event.dataTransfer.types.includes("Files")
const typing = (target: EventTarget | null) =>
  target instanceof HTMLElement && (target.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName))
/** A field with words in it, which keeps its own undo. */
const words = (target: EventTarget | null) =>
  target instanceof HTMLElement && (target.isContentEditable || target.tagName === "TEXTAREA" || (target instanceof HTMLInputElement && ["text", "search", "url", "email", "number", "password", "tel"].includes(target.type)))

/** Project state and rows into the store; nothing here renders. */
function ProjectSync({ id }: { id: string }) {
  const { client, stream, bridge } = useConnection()
  const store = useCanvasStore()
  const navigate = useNavigate()
  const { data: state, error: stateError } = useQuery(projectQuery(client, stream, id))
  const { data: canvas, error: canvasError } = useQuery(canvasQuery(client, id))
  const asked = useRef(new Set<string>())
  const queries = useQueryClient()

  const tools = useSignal(store.tools)
  useEffect(() => {
    if (state) store.applyState(state)
  }, [store, state])
  useEffect(() => {
    // A tool an asset names that the list didn't hold, asked for by its name, once. Before the list has come, that would be all of them, and a
    // first read under way can't be asked again. Text is there from the start, so the list has come when there's more.
    if (tools.size < 2) return
    for (const { tool, prefixes } of state?.assets ?? []) {
      // And again with each version it lands: a render can read local code the list couldn't, on a Runtime newer than the one that lists it.
      const ask = `${tool}@${prefixes?.length ?? 0}`
      if (!tool || madeHere(tool) || asked.current.has(ask) || tools.has(tool)) continue
      asked.current.add(ask)
      // Local code is listed only once something names it, so the list is read again. Anything else can be searched for.
      if (/^[./~\\]|^\w:[\\/]/.test(tool)) void queries.invalidateQueries({ queryKey: ["tools"] })
      else client.tools(tool.split("/").slice(0, -1).join(" "), null, id).then(({ tools }) => store.addTools(tools), () => {})
    }
  }, [store, client, id, queries, state, tools])
  useEffect(() => {
    if (canvas) store.applyCanvas(canvas.cards, canvas.began, canvas.seq)
  }, [store, canvas])
  // Gone: there's nothing to show. Anything else that stops it being read is said on the canvas (`Unread`), for as long as it holds.
  useEffect(() => {
    if (!(stateError instanceof ApiError && stateError.status === 404)) return
    const note = say("Couldn't read this project", stateError)
    navigate("/", { replace: true })
    return () => dismiss(note)
  }, [stateError, navigate])
  useEffect(() => {
    if (canvasError && !lost(canvasError)) say("Couldn't read where things sit", canvasError)
  }, [canvasError])
  useEffect(() => {
    store.history.read()
    return stream.subscribe((event) => {
      if (event.type === "canvas" && event.id === id) store.applyRows(event.cards, event.seq)
      if (event.type === "history" && event.id === id) store.history.read()
      if (event.type === "resync" || event.type === "hello") {
        store.fellBehind()
        store.history.read()
      }
    })
  }, [stream, store, id])
  useEffect(() => bridge.onRun((run) => store.applyRun(run)), [bridge, store])
  useEffect(() => {
    store.readAgain = (names) => bridge.changed(id, { assets: names, head: true })
  }, [bridge, store, id])
  return null
}

/**
 * The machines a render may go to, as `cozy project render` sends them: there's no choosing, the walk places each job on whichever ends it first (Paul,
 * 11-05). Every rental of yours, up or on its way, since a rented GPU is for using; another computer's that's up; this one last, which loses a tie.
 * Another computer's rental of your account takes nothing from here. With none, Render asks for one under Machines. Null until they're read.
 */
function useMachines(store: CanvasStore): string[] | null {
  const { client, status } = useConnection()
  const { data, isPending } = useQuery(machinesQuery(client))
  const held = (data?.machines ?? []).filter((machine) => machine.ordered !== false && !machine.offline)
  // One still booting takes what's sent to it, which waits for it there.
  const rented = held.filter((machine) => machine.kind === "rental" && (machine.state === "ready" || machine.state === "booting"))
  const up = held.filter((machine) => machine.kind !== "rental" && machine.state === "ready" && !machine.unreachable && (machine.kind !== "local" || status.local_usable))
  const names = [...rented, ...up.filter((machine) => machine.kind !== "local"), ...up.filter((machine) => machine.kind === "local")].map((machine) => machine.name)
  const key = names.join()
  // biome-ignore lint/correctness/useExhaustiveDependencies: the names, as text
  useEffect(() => {
    store.machines = () => names
    if (names.length) store.machineUp()
  }, [store, key])
  return isPending ? null : names
}

/** Render all, and how many that is. Approved assets stay. */
function RenderAll({ id }: { id: string }) {
  const { client, stream } = useConnection()
  const store = useCanvasStore()
  const { data } = useQuery(projectQuery(client, stream, id))
  const fresh = (data?.assets ?? []).filter((asset) => asset.tool && !madeHere(asset.tool) && !asset.pinned).length
  const asking = useSignal(store.allAsking)
  if (!fresh) return null
  return (
    <span className="render-all">
      <button type="button" className="tb primary" onClick={() => store.runAll()}>
        Render all {fresh}
      </button>
      {asking && <Asking fit={asking.fit} all />}
    </span>
  )
}

/** A project this Calcifer can't read: why, and what to do, where its boxes would be. */
function Unread({ error }: { error: unknown }) {
  const body = error instanceof ApiError ? error.body : undefined
  return (
    <div className="unread" role="alert">
      <div>
        <h2>This project can't be opened yet</h2>
        <p>{error instanceof Error ? noHash(error.message) : String(error)}</p>
        {body?.remedy && <p>{noHash(body.remedy)}</p>}
        {body?.next?.map((line) => (
          <Copyable key={line} text={line} />
        ))}
      </div>
    </div>
  )
}

/** The box the address names is the one worked on, however the page got there. Gone from under it, by anyone, there's nothing to look at. */
function Addressed({ name, enter, leave }: { name: string; enter: (name: string) => void; leave: () => void }) {
  const store = useCanvasStore()
  const there = Boolean(useElement(name))
  const names = useSignal(store.names)
  // biome-ignore lint/correctness/useExhaustiveDependencies: names tells it the project has been read
  useEffect(() => {
    if (there) enter(store.resolveName(name))
    else if (store.hasRead) leave()
  }, [there, name, names, store, enter, leave])
  return null
}

/** The zoom control, drawn again only as the zoom changes: a pan changes nothing here. */
function Zoom({ zoomBy, items }: { zoomBy: (by: 1 | -1) => void; items: () => ZoomItem[] }) {
  const zoom = useStore((state) => state.transform[2])
  return <ZoomControl zoom={zoom} zoomIn={() => zoomBy(1)} zoomOut={() => zoomBy(-1)} items={items} />
}

/** One line, and it does what it says when clicked too. */
function Empty({ choose }: { choose: () => void }) {
  const store = useCanvasStore()
  useSignal(store.names)
  const empty = useStore((state) => state.nodes.length === 0)
  if (!empty || (store.projectId && !store.hasRead) || !store.empty) return null
  return (
    <div className="empty-canvas">
      <button type="button" onClick={choose}>
        Drop a file here
      </button>
    </div>
  )
}

function Canvas({
  projectId,
  slot,
  history,
  onAdopt,
}: {
  projectId: string | null
  slot: HTMLElement | null
  history: HTMLElement | null
  onAdopt: (id: string, code: string) => void
}) {
  const connection = useConnection()
  const flow = useReactFlow<ElementNode, Edge>()
  const navigate = useNavigate()
  const to = useTo()
  const { client, stream, bridge, outbox, canvases } = connection
  const [saved] = useState(() => loadViewport(projectId))
  // One per project for as long as the tab is open: a tab gone to and back finds what it showed, and what's still waiting for Calcifer shows as done.
  // One a code reload left is another class's, and is made again.
  const [store] = useState(() => {
    const kept = projectId ? canvases.get(projectId) : undefined
    return kept instanceof CanvasStore ? kept : new CanvasStore(client, outbox, projectId, loadSelection(projectId), loadMore(projectId))
  })
  // A canvas's first box makes its project: kept from then on.
  useEffect(() => {
    if (store.projectId) canvases.set(store.projectId, store)
  })
  const adopt = useRef(onAdopt)
  adopt.current = onAdopt
  const [placer] = useState(() => new Placer(store, client, { adopt: (id, code) => adopt.current(id, code) }))
  const [openedOn] = useState(projectId)
  const [veil, setVeil] = useState(false)
  const [shut, setShut] = useState(loadToolsShut)
  const compactAsset = useMatch("/p/:code/assets/:name")
  const legacyAsset = useMatch("/projects/:id/assets/:name")
  const opened = (compactAsset ?? legacyAsset)?.params.name
  const stage = useRef<HTMLDivElement>(null)
  const returning = useRef<{ view: Viewport; until: number } | null>(null)
  const folded = useRef(false)
  const shutNow = useRef(shut)
  shutNow.current = shut
  const fitHereNow = useCallback(
    (only = false) => {
      const held = store.inPlace
      const id = held && store.idOf(held.name)
      if (!held || !id || !stage.current || held.moved || held.pulled || store.isDragging(held.name)) return
      const { left, top, right, bottom } = clearOf()
      const stageBox = stage.current.getBoundingClientRect()
      const room = { x: left, y: top, w: stageBox.width - left - right, h: stageBox.height - top - bottom }
      // What's still in reach stays as it is: nothing moves under what's being typed.
      const at = held.fitted
      if (only && at) {
        const projected = store.drawSurface(undefined, at)
        if (!projected) return
        const { bounds } = projected
        const x = bounds.x * at.zoom + at.x
        const y = bounds.y * at.zoom + at.y
        if (x >= room.x - 1 && y >= room.y - 1 && x + bounds.w * at.zoom <= room.x + room.w + 1 && y + bounds.h * at.zoom <= room.y + room.h + 1) return
      }
      const fit = store.drawSurface(room)
      if (!fit) {
        held.fitted = undefined
        return
      }
      held.fitted = fit.view
      void flow.setViewport(fit.view, { duration: HERE_MS })
    },
    [store, flow],
  )
  const fitting = useRef<{ frame: number; only: boolean; owner: NonNullable<CanvasStore["inPlace"]> } | null>(null)
  const fitHere = useCallback((only = false) => {
    const owner = store.inPlace
    if (!owner) return
    if (fitting.current && fitting.current.owner !== owner) {
      cancelAnimationFrame(fitting.current.frame)
      fitting.current = null
    }
    if (fitting.current) {
      fitting.current.only &&= only
      return
    }
    const request = { frame: 0, only, owner }
    fitting.current = request
    request.frame = requestAnimationFrame(() => {
      fitting.current = null
      if (store.inPlace !== owner) return
      fitHereNow(request.only)
    })
  }, [fitHereNow, store])
  useEffect(() => () => {
    if (fitting.current) cancelAnimationFrame(fitting.current.frame)
    fitting.current = null
  }, [])
  store.fitHere = fitHere
  /** Working on a box where it stands: the canvas draws in on it, and all it can do comes around it. Another box takes over, and leaving goes back to just where it was. */
  const writeHere = useCallback(
    (name: string) => {
      if (store.inPlace?.name === name) return
      if (fitting.current) cancelAnimationFrame(fitting.current.frame)
      fitting.current = null
      if (store.inPlace) {
        store.closeSurface()
        store.patch(store.inPlace.name, { here: undefined, hereWords: undefined })
      }
      // Still on its way back from the last one, that's where to come back to.
      const going = returning.current
      const owner = { name, back: store.inPlace?.back ?? (going && performance.now() < going.until ? going.view : flow.getViewport()) }
      store.inPlace = owner
      store.view(name)
      const view = store.get(name)
      // Its approved take, else its newest; with none yet, its words, ready to type in.
      const words = !view?.asset?.prefixes?.length && !view?.asset?.sha256
      if (words) store.focusing = name
      store.patch(name, { here: true, hereWords: words || undefined })
      store.openSurface(name)
      if (store.writeOnOpen === name) store.writeNext(name, true)
      store.writeOnOpen = null
      if (window.innerWidth < NARROW && !shutNow.current) {
        folded.current = true
        setShut(true)
      }
      requestAnimationFrame(() => {
        if (store.inPlace === owner) fitHere()
      })
    },
    [store, flow, fitHere],
  )
  store.openHere = (name: string) => {
    writeHere(name)
    navigate(to(`assets/${encodeURIComponent(name)}`))
  }
  const stopWriting = useCallback(() => {
    const held = store.inPlace
    if (!held) return
    if (fitting.current) cancelAnimationFrame(fitting.current.frame)
    fitting.current = null
    store.inPlace = null
    store.closeSurface()
    void store.save(held.name)
    store.patch(held.name, { here: undefined, hereWords: undefined, pickingUp: undefined })
    store.view(held.name)
    returning.current = { view: held.back, until: performance.now() + HERE_MS }
    if (folded.current) {
      folded.current = false
      setShut(false)
    }
    void flow.setViewport(held.back, { duration: HERE_MS })
  }, [store, flow])
  const leaveHere = useCallback(() => {
    if (!store.inPlace) return
    stopWriting()
    navigate(to())
  }, [store, navigate, stopWriting, to])
  store.leaveHere = leaveHere
  store.hereRenamed = (name) => navigate(to(`assets/${encodeURIComponent(name)}`), { replace: true })
  // The address says which box is being worked on, so a reload or Back finds it, and anything else leaves it.
  useEffect(() => {
    if (!opened) stopWriting()
  }, [opened, stopWriting])
  const picker = useRef<HTMLInputElement>(null)
  const settling = useRef<ReturnType<typeof setTimeout>>(undefined)
  const pointer = useRef<{ x: number; y: number } | null>(null)
  // Which machine's packages a list leads with. The list waits to know, or it would be asked for twice, each a wait on the Hub.
  const machines = useMachines(store)
  const machine = machines?.at(-1) ?? null
  // What can't be read says so on the canvas, and everything around it still works. One already read stays while Calcifer's away.
  const { data: read, error } = useQuery({ ...projectQuery(client, stream, projectId ?? ""), enabled: Boolean(projectId) })
  const unread = error && !(read && lost(error)) ? error : null
  const readable = unread ? null : projectId
  const { data: listed } = useQuery({ ...toolsQuery(client, "", machine, readable), enabled: machines !== null })

  useEffect(() => {
    // A project opened here for the first time is fitted, once. Either way the viewport is kept, so a reload finds it.
    // A project with no rows has nothing to fit at its first read: its first placing is what to look at.
    let fitted = Boolean(saved)
    const fitOnce = async () => {
      if (fitted || !store.projectId) return
      fitted = true
      if (openedOn) await flow.fitView(fitOver())
      saveViewport(store.projectId, flow.getViewport())
    }
    store.onFirstRead = async () => {
      if (flow.getNodes().length || !store.projectId) return fitOnce()
    }
    store.onPlaced = fitOnce
    store.onMore = (names) => {
      if (store.projectId) saveMore(store.projectId, names)
    }
    store.askMachine = () => navigate(to("machines"))
    store.rent = (pick) => {
      const chosen = pick ? `?${new URLSearchParams({ rent: pick.sku, gpus: String(pick.gpus) })}` : ""
      navigate(`${to("machines")}${chosen}`)
    }
    store.attach(flow)
    return () => store.detach()
  }, [store, flow, saved, openedOn, bridge, navigate, to])

  /** Where the last zoom of the buttons or keys is going, so a second press steps on from there, not from part of the way. */
  const aim = useRef<{ view: Viewport; until: number } | null>(null)
  const aimed = useCallback(() => (aim.current && performance.now() < aim.current.until ? aim.current.view : flow.getViewport()), [flow])
  /** Zoom to `zoom` about the middle of what's visible, as a fit moves. Worked on where it stands, a box then stays where this puts it. */
  const zoomTo = useCallback(
    (zoom: number) => {
      const pane = stage.current?.querySelector(".react-flow")?.getBoundingClientRect()
      if (!pane) return
      const from = aimed()
      const to = Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, zoom))
      const left = document.querySelector(".tools")?.getBoundingClientRect().right ?? pane.left
      const right = document.querySelector(".drawer")?.getBoundingClientRect().left ?? pane.right
      const [x, y] = [(left + right) / 2 - pane.left, pane.height / 2]
      const view = { x: x - ((x - from.x) * to) / from.zoom, y: y - ((y - from.y) * to) / from.zoom, zoom: to }
      aim.current = { view, until: performance.now() + HERE_MS }
      if (store.inPlace) store.inPlace.moved = true
      void flow.setViewport(view, { duration: HERE_MS })
    },
    [flow, store, aimed],
  )
  const zoomBy = useCallback((by: 1 | -1) => zoomTo(stepZoom(aimed().zoom, by)), [aimed, zoomTo])
  /** Everything; a box worked on, it and what's under it as they were drawn in. */
  const fit = useCallback(() => {
    aim.current = null
    if (!store.inPlace) return void flow.fitView({ ...fitOver(), duration: HERE_MS })
    store.inPlace.moved = false
    fitHere()
  }, [flow, store, fitHere])
  /** What's selected, else the box worked on. */
  const fitSelected = useCallback(() => {
    const nodes = flow.getNodes().filter((node) => node.selected)
    if (!nodes.length) return fit()
    if (nodes.length === 1 && nodes[0].data.name === store.inPlace?.name) return fit()
    aim.current = null
    if (store.inPlace) store.inPlace.moved = true
    void flow.fitView({ ...fitOver(), maxZoom: MOST_HERE, nodes, duration: HERE_MS })
  }, [flow, store, fit])
  const zoomItems = useCallback(
    (): ZoomItem[] => [
      { label: "Zoom in", keys: keys("Mod", "+"), run: () => zoomBy(1) },
      { label: "Zoom out", keys: keys("Mod", "−"), run: () => zoomBy(-1) },
      { label: "Zoom to fit", keys: keys("Shift", "1"), run: fit },
      { label: "Zoom to selection", keys: keys("Shift", "2"), run: fitSelected, disabled: !store.inPlace && !flow.getNodes().some((node) => node.selected) },
      "rule",
      { label: "Zoom to 50%", run: () => zoomTo(0.5) },
      { label: "Zoom to 100%", keys: keys("Shift", "0"), run: () => zoomTo(1) },
      { label: "Zoom to 200%", run: () => zoomTo(2) },
    ],
    [flow, store, zoomBy, zoomTo, fit, fitSelected],
  )
  const choose = useCallback(() => picker.current?.click(), [])

  // The middle of what's visible: the canvas between whatever floats over it.
  const visibleCentre = useCallback(() => {
    const box = stage.current!.getBoundingClientRect()
    const left = document.querySelector(".tools")?.getBoundingClientRect().right ?? box.left
    const right = document.querySelector(".drawer")?.getBoundingClientRect().left ?? box.right
    return flow.screenToFlowPosition({ x: (left + right) / 2, y: box.top + box.height / 2 })
  }, [flow])

  // A click puts a box in the middle of what's visible, and `place` keeps it clear of what's there.
  const placeTool = useCallback(
    (tool: Tool) => {
      const width = tool.fields.some((field) => field.type === "timeline") ? 720 : tool.makes === "video" ? 360 : 300
      const { x, y } = visibleCentre()
      placer.addTool(tool, { x: x - width / 2 + CARD_WIDTH / 2, y: y - (width * 0.75) / 2 + 16 })
    },
    [placer, visibleCentre],
  )

  const picked = useCallback(
    (event: ChangeEvent<HTMLInputElement>) => {
      const files = [...(event.target.files ?? [])]
      event.target.value = ""
      if (!files.length) return
      const columns = Math.ceil(Math.sqrt(files.length))
      const { x, y } = visibleCentre()
      placer.add(files, { x: x - (columns * CARD_WIDTH) / 2 + CARD_WIDTH / 2, y: y - (Math.ceil(files.length / columns) * CARD_WIDTH) / 2 + 16 })
    },
    [placer, visibleCentre],
  )

  // What's carried in the page, let go here: on a box that takes it, else by what it is.
  const land = useCallback(
    (load: Load, x: number, y: number) => {
      if (letGo(store, x, y)) return
      const point = flow.screenToFlowPosition({ x, y })
      const under = boxAt(store, x, y)
      if (load.name) {
        const target = store.dropTargetAt(x, y, [])
        if (target) target.drop([load.name], x, y)
        else if (under) store.takeInto(under, [load.name])
        store.setHot(null)
        return
      }
      if (load.tool) placer.addTool(load.tool, point, load.fields)
      if (load.take && !(under && store.takePicture(under, load.take))) placer.addFile(load.take, point)
    },
    [placer, flow, store],
  )
  useEffect(() => {
    const el = stage.current
    if (!el) return
    const heard = (event: Event) => {
      const { load, x, y } = (event as LetGo).detail
      land(load, x, y)
    }
    el.addEventListener(LET_GO, heard)
    return () => el.removeEventListener(LET_GO, heard)
  }, [land])
  // While something's carried, what would take it says so.
  useEffect(
    () =>
      carriedAt.subscribe(() => {
        const at = carriedAt.get()
        if (at) carriedOver(store, at.x, at.y)
        else store.setHot(null)
      }),
    [store],
  )

  // Files from the computer: the browser's own drag.
  const drop = useCallback(
    (event: DragEvent) => {
      if (!hasFiles(event)) return
      event.preventDefault()
      setVeil(false)
      const point = flow.screenToFlowPosition({ x: event.clientX, y: event.clientY })
      const under = boxAt(store, event.clientX, event.clientY)
      const files = [...event.dataTransfer.files]
      if (!files.length) return
      if (under && store.joins(under)) placer.join(files, under)
      else if (under && store.get(under)?.asset?.tool) placer.add(files, point, (other) => store.takeInto(under, [other]))
      else placer.add(files, point)
    },
    [placer, flow, store],
  )

  const moved = useCallback(
    (_: unknown, viewport: Viewport) => {
      if (store.projectId && !store.inPlace) saveViewport(store.projectId, viewport)
    },
    [store],
  )

  const selectionChanged = useCallback(
    ({ nodes }: OnSelectionChangeParams<ElementNode, Edge>) => {
      store.select(nodes.map((node) => node.data.name))
      clearTimeout(settling.current)
      settling.current = setTimeout(() => {
        if (store.projectId) saveSelection(store.projectId, nodes.map((node) => node.data.name))
      }, 200)
    },
    [store],
  )

  const draggedEls = (nodes: ElementNode[]) => nodes.flatMap((node) => document.querySelector(`.react-flow__node[data-id="${CSS.escape(node.id)}"]`) ?? [])

  // While something's carried, what could take it says so, with no render of anything it isn't.
  const carrying = (held: boolean) => stage.current?.toggleAttribute("data-carrying", held)
  const dragStart = useCallback(
    (nodes: ElementNode[]) => {
      carrying(true)
      store.dragStarted(nodes)
    },
    [store],
  )
  const dragging = useCallback(
    (event: Pointing, nodes: ElementNode[]) => {
      const { clientX, clientY } = pointOf(event)
      const names = nodes.map((node) => node.data.name)
      const { target, why } = store.offered(clientX, clientY, names, draggedEls(nodes))
      if (!why) target?.over(names, clientX, clientY)
      stage.current?.toggleAttribute("data-offering", Boolean(target) && !why)
    },
    [store],
  )
  const dragStop = useCallback(
    (event: Pointing, nodes: ElementNode[]) => {
      carrying(false)
      stage.current?.removeAttribute("data-offering")
      const { clientX, clientY } = pointOf(event)
      const { target, why } = store.offered(clientX, clientY, nodes.map((node) => node.data.name), draggedEls(nodes))
      store.refusing.set(null)
      if (why) say(why)
      if (!target || why) {
        store.setHot(null)
        return store.dragStopped(nodes)
      }
      store.dragDropped(nodes)
      target.drop(nodes.map((node) => node.data.name), clientX, clientY)
      store.setHot(null)
    },
    [store],
  )

  /** On a box's picture, the arrows walk its row one take at a time, and Enter approves the one showing, or unapproves it. */
  const stepTakes = useCallback(
    (name: string, event: KeyboardEvent) => {
      const view = store.get(name)
      const asset = view?.asset
      if (!view || !asset || view.hereWords || event.target instanceof HTMLSelectElement || !["ArrowLeft", "ArrowRight", "Enter"].includes(event.key)) return false
      const row = [...(asset.prefixes ?? [])].reverse()
      const at = row.findIndex((version) => version.number === shownOf(view)?.number)
      if (at < 0) return false
      event.preventDefault()
      if (event.key === "Enter") {
        store.toggleApproved(name, row[at].number)
        return true
      }
      const next = row[at + (event.key === "ArrowLeft" ? -1 : 1)]
      if (next) {
        store.pickTake(name, next.number)
        requestAnimationFrame(() => document.querySelector(".card.here .take.on")?.scrollIntoView({ block: "nearest", inline: "nearest" }))
      }
      return true
    },
    [store],
  )

  useEffect(() => {
    const key = (event: KeyboardEvent) => {
      // Bubbling up to here: a name field or the names offered in a text has taken its own Esc, and a menu goes first.
      if (store.viewing.get()) return
      const here = store.inPlace?.name
      if (event.key === "Escape" && here && !store.menu.get() && store.get(here)?.draft === undefined) return leaveHere()
      // Undo and redo are the project's, but in a field with words in it, which keeps its own.
      const letter = event.key.toLowerCase()
      if ((event.metaKey || event.ctrlKey) && !event.altKey && (letter === "z" || letter === "y") && !words(event.target)) {
        event.preventDefault()
        if (letter === "y" || event.shiftKey) store.redo()
        else store.undo()
        return
      }
      if (typing(event.target)) return
      if (here && stepTakes(here, event)) return
      const command = event.metaKey || event.ctrlKey
      if (event.key === "Escape") {
        store.menu.set(null)
        for (const node of flow.getNodes()) {
          const placing = store.get(node.data.name)?.placing
          if (node.selected && placing && placing.stage !== "adding") store.cancel(store.resolveName(node.data.name))
        }
        flow.setNodes((nodes) => nodes.map((node) => (node.selected ? { ...node, selected: false } : node)))
      } else if (event.key === "Delete" || (event.key === "Backspace" && !command)) {
        const take = here ? store.lookingAt(here) : undefined
        if (here && take !== undefined) {
          event.preventDefault()
          return store.discardTake(here, take)
        }
        const names = flow.getNodes().filter((node) => node.selected && node.type === "element").map((node) => node.data.name)
        if (names.length) {
          event.preventDefault()
          store.remove(names)
        }
      } else if (command && event.key.toLowerCase() === "a") {
        event.preventDefault()
        flow.setNodes((nodes) => nodes.map((node) => ({ ...node, selected: true })))
      } else if ((command && event.key === "0") || (!command && event.shiftKey && event.code === "Digit0")) {
        event.preventDefault()
        zoomTo(1)
      } else if (event.key === "=" || event.key === "+") {
        event.preventDefault()
        zoomBy(1)
      } else if (event.key === "-" || event.key === "_") {
        event.preventDefault()
        zoomBy(-1)
      } else if (!command && (event.key === "f" || (event.shiftKey && event.code === "Digit1"))) {
        fit()
      } else if (!command && event.shiftKey && event.code === "Digit2") {
        fitSelected()
      }
    }
    window.addEventListener("keydown", key)
    return () => window.removeEventListener("keydown", key)
  }, [flow, store, fit, fitSelected, zoomTo, zoomBy, leaveHere, stepTakes])

  // A text copies and pastes its own, and so does any selection of words on the page.
  useEffect(() => {
    const copy = (event: ClipboardEvent) => {
      if (typing(event.target) || store.viewing.get() || !event.clipboardData || !document.getSelection()?.isCollapsed) return
      const here = store.inPlace?.name
      const names = here ? [here] : flow.getNodes().filter((node) => node.selected && node.type === "element").map((node) => node.data.name)
      if (copyBoxes(store, client, names, event.clipboardData)) event.preventDefault()
    }
    const paste = (event: ClipboardEvent) => {
      const data = event.clipboardData
      if (typing(event.target) || store.viewing.get() || !data) return
      const own = ownCopy(store, data)
      const files = [...data.files].filter((file) => file.type.startsWith("image/"))
      if (!own && !files.length) return
      event.preventDefault()
      const here = store.inPlace?.name
      // On a picture with no tool, a picture is its next version.
      const joins = here && store.joins(here) && !store.get(here)?.hereWords ? here : null
      const point = pointer.current ? flow.screenToFlowPosition(pointer.current) : visibleCentre()
      const one = own?.length === 1 ? own[0] : null
      if (joins && one && "file" in one && one.file.from !== joins) store.takePicture(joins, one.file)
      else if (own) placer.paste(own, point)
      else if (joins) placer.join(files, joins)
      else placer.pasteFiles(files, point)
    }
    window.addEventListener("copy", copy)
    window.addEventListener("paste", paste)
    return () => {
      window.removeEventListener("copy", copy)
      window.removeEventListener("paste", paste)
    }
  }, [store, client, flow, placer, visibleCentre])

  // Every box's node is handed these: made once, so drawing the canvas again draws no box again.
  const moveStarted = useCallback(
    (event: unknown) => {
      if (event && store.inPlace) store.inPlace.moved = true
    },
    [store],
  )
  const nodeClicked = useCallback(
    (event: ReactMouseEvent, node: ElementNode) => {
      if (node.type !== "element" || !store.projectId || store.get(node.data.name)?.placing || event.shiftKey || event.metaKey || event.ctrlKey) return
      const { name } = node.data
      const view = store.get(name)
      // A double tap on its picture approves the take showing, or unapproves it.
      const shown = view?.asset?.prefixes?.length ? shownOf(view)?.number : undefined
      if (event.detail > 1 && shown !== undefined && (event.target as Element).closest(".media")) return store.toggleApproved(name, shown)
      if (store.inPlace?.name === name && view?.here) return
      // A click opens at once: only Shift selects.
      flow.setNodes((nodes) => nodes.map((held) => (held.selected ? { ...held, selected: false } : held)))
      writeHere(name)
      navigate(to(`assets/${encodeURIComponent(name)}`))
    },
    [store, flow, writeHere, navigate, to],
  )
  const paneClicked = useCallback(() => leaveHere(), [leaveHere])
  const nodeMenu = useCallback(
    (event: ReactMouseEvent, node: ElementNode) => {
      if (node.type !== "element") return
      event.preventDefault()
      store.menu.set({ name: node.data.name, x: event.clientX, y: event.clientY })
    },
    [store],
  )
  const nodeDragStart = useCallback((_: unknown, __: unknown, nodes: ElementNode[]) => dragStart(nodes), [dragStart])
  const nodeDrag = useCallback((event: Pointing, _: unknown, nodes: ElementNode[]) => dragging(event, nodes), [dragging])
  const nodeDragStop = useCallback((event: Pointing, _: unknown, nodes: ElementNode[]) => dragStop(event, nodes), [dragStop])
  const selectionDragStart = useCallback((_: unknown, nodes: ElementNode[]) => dragStart(nodes), [dragStart])
  const selectionDrag = useCallback((event: Pointing, nodes: ElementNode[]) => dragging(event, nodes), [dragging])
  const selectionDragStop = useCallback((event: Pointing, nodes: ElementNode[]) => dragStop(event, nodes), [dragStop])

  // Far out, a card shows its picture, its name and little else.
  const band = useStore((state) => (state.transform[2] < 0.2 ? 2 : state.transform[2] < 0.45 ? 1 : 0))
  useLayoutEffect(() => store.far.set(band === 2), [store, band])
  const sharpness = useStore((state) => 2 ** Math.min(2, Math.max(-2, Math.ceil(Math.log2(state.transform[2] || 1)))))
  useLayoutEffect(() => store.sharpness.set(sharpness), [store, sharpness])
  // How large a box's words are drawn goes on the stage as the zoom crosses a step: no render, so a zoom draws no box again.
  const flowStore = useStoreApi<ElementNode, Edge>()
  useLayoutEffect(() => {
    let held = 0
    let drawn = ""
    let frame = 0
    let latest: Viewport
    const set = ([x, y, zoom]: [number, number, number]) => {
      const text = textScale(zoom)
      if (text !== held) {
        held = text
        stage.current?.style.setProperty("--text", String(text))
        // Resting cards keep their existing inverse limit.
        stage.current?.style.setProperty("--hang", String(Math.min(text, HANG)))
      }
      const now = `${x}:${y}:${zoom}`
      if (now !== drawn) {
        drawn = now
        latest = { x, y, zoom }
        if (!frame) frame = requestAnimationFrame(() => {
          frame = 0
          store.refreshSurface(latest)
        })
      }
    }
    set(flowStore.getState().transform)
    const stop = flowStore.subscribe((state) => set(state.transform))
    return () => {
      stop()
      cancelAnimationFrame(frame)
    }
  }, [flowStore, store])
  // A zoom draws the boxes and the dots each frame at their size, and two frames after it they go back on their layers, drawn at the new one.
  useLayoutEffect(() => {
    let [held, settle] = [flowStore.getState().transform[2], 0]
    const settled = () => stage.current?.toggleAttribute("data-zooming", false)
    const stop = flowStore.subscribe(({ transform: [, , zoom] }) => {
      if (zoom === held) return
      held = zoom
      stage.current?.toggleAttribute("data-zooming", true)
      cancelAnimationFrame(settle)
      settle = requestAnimationFrame(() => (settle = requestAnimationFrame(settled)))
    })
    return () => {
      stop()
      cancelAnimationFrame(settle)
    }
  }, [flowStore])

  // The list a box's controls come from, whatever the panel is searching.
  useEffect(() => {
    if (listed) store.addTools(listed.tools)
  }, [store, listed])

  return (
    <CanvasStoreProvider value={store}>
      <ToolPanel
        project={readable}
        machine={machine}
        waiting={machines === null}
        shut={shut}
        setShut={(next) => {
          setShut(next)
          saveToolsShut(next)
        }}
        place={placeTool}
        choose={choose}
        box={opened}
      />
      <div
        ref={stage}
        className="stage"
        data-layered={LAYERED ? "" : undefined}
        data-tools={shut ? "shut" : "open"}
        data-lod={band}
        onPointerMove={(event) => {
          pointer.current = { x: event.clientX, y: event.clientY }
        }}
        onPointerLeave={() => {
          pointer.current = null
        }}
        onDragEnter={(event) => {
          if (hasFiles(event)) setVeil(true)
        }}
        onDragOver={(event) => {
          if (!hasFiles(event)) return
          event.preventDefault()
          event.dataTransfer.dropEffect = "copy"
        }}
        onDragLeave={(event) => {
          if (!event.currentTarget.contains(event.relatedTarget as globalThis.Node | null)) setVeil(false)
        }}
        onDrop={drop}
      >
        <Sheet />
        <ReactFlow<ElementNode, Edge>
          defaultNodes={[]}
          nodeTypes={nodeTypes}
          defaultViewport={saved ?? undefined}
          panOnScroll
          panOnScrollSpeed={1}
          selectionMode={SelectionMode.Partial}
          selectNodesOnDrag={false}
          zoomOnDoubleClick={false}
          minZoom={MIN_ZOOM}
          maxZoom={MAX_ZOOM}
          multiSelectionKeyCode={MULTI}
          deleteKeyCode={null}
          disableKeyboardA11y
          nodesConnectable={false}
          onlyRenderVisibleElements
          colorMode="dark"
          attributionPosition="bottom-left"
          onMoveStart={moveStarted}
          onMoveEnd={moved}
          onSelectionChange={selectionChanged}
          onNodeClick={nodeClicked}
          onPaneClick={paneClicked}
          onNodeContextMenu={nodeMenu}
          onNodeDragStart={nodeDragStart}
          onNodeDrag={nodeDrag}
          onNodeDragStop={nodeDragStop}
          onSelectionDragStart={selectionDragStart}
          onSelectionDrag={selectionDrag}
          onSelectionDragStop={selectionDragStop}
        >
          <Dots />
          {unread ? <Unread error={unread} /> : <Empty choose={choose} />}
        </ReactFlow>
        <Zoom zoomBy={zoomBy} items={zoomItems} />
        {veil && <div className="dropveil" />}
      </div>
      {opened && <Addressed name={opened} enter={writeHere} leave={leaveHere} />}
      <AssetMenu />
      <Viewer />
      <input ref={picker} className="picker" type="file" multiple accept={ACCEPT} onChange={picked} aria-label="Add a file" />
      {slot && projectId && createPortal(<RenderAll id={projectId} />, slot)}
      {history && projectId && createPortal(<HistoryControls />, history)}
      {projectId && <ProjectSync id={projectId} />}
    </CanvasStoreProvider>
  )
}

/**
 * WebKit draws again all that a transform moves, each frame it moves, so there the boxes are on a layer of their own: 300 of them panned at
 * 214–590 ms a frame on macOS, and 16 ms on one. Chrome and Firefox move them as drawn already, and Chrome rasters a layer at its whole size:
 * 5,000 fitted were many windows of it, and every frame took 1.5 s.
 */
const LAYERED = /AppleWebKit/.test(navigator.userAgent) && !/Chrome|Chromium/.test(navigator.userAgent)

/** How much a box's words are scaled against the canvas, so they read at nearly one size at any zoom: in steps, so a zoom sets it only as it crosses one. */
const textScale = (zoom: number) => Math.round((1 / Math.min(2.5, Math.max(0.25, zoom))) * 20) / 20

/** Always mounted: everything else is a drawer over it, so opening one loses no viewport, no draft and no import in flight. */
export function CanvasHost(props: { projectId: string | null; slot: HTMLElement | null; history: HTMLElement | null; onAdopt: (id: string, code: string) => void }) {
  return (
    <ReactFlowProvider defaultNodes={[]}>
      <Canvas {...props} />
    </ReactFlowProvider>
  )
}
