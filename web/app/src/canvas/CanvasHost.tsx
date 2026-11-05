import { useQuery, useQueryClient } from "@tanstack/react-query"
import {
  Background,
  BackgroundVariant,
  type Edge,
  type OnSelectionChangeParams,
  ReactFlow,
  ReactFlowProvider,
  SelectionMode,
  useReactFlow,
  useStore,
  type Viewport,
} from "@xyflow/react"
import { type ChangeEvent, type DragEvent, type MouseEvent as ReactMouseEvent, useCallback, useEffect, useRef, useState } from "react"
import { createPortal } from "react-dom"
import { useNavigate } from "react-router-dom"
import { ApiError } from "../api/client"
import { canvasQuery, machinesQuery, projectQuery, toolsQuery } from "../api/queries"
import { useConnection } from "../app/Connection"
import { loadMore, loadSelection, loadToolsShut, loadViewport, saveMore, saveSelection, saveToolsShut, saveViewport } from "../app/view"
import type { Tool } from "../types"
import { dismiss, say } from "../ui/notes"
import { useSignal } from "../ui/signal"
import { AssetMenu } from "./AssetMenu"
import { CARD_WIDTH, CanvasStore, type ElementNode } from "./CanvasStore"
import { Card } from "./Card"
import { CanvasStoreProvider, useCanvasStore } from "./context"
import { DRAGGED_FILE, DRAGGED_TOOL } from "./drag"
import { EntityCard } from "./EntityCard"
import { Placer } from "./place"
import { madeHere } from "./shown"
import { ToolPanel } from "./ToolPanel"

const nodeTypes = { element: Card, entity: EntityCard }
const fitOver = (toolsShut: boolean) => ({ maxZoom: 1, padding: { top: "56px", right: "24px", bottom: "24px", left: toolsShut ? "56px" : "220px" } as const })

type Pointing = ReactMouseEvent | MouseEvent | TouchEvent
const pointOf = (event: Pointing) => ("clientX" in event ? event : event.changedTouches[0])

const ACCEPT = "image/*,video/*,audio/*"
const MULTI = ["Shift", "Meta", "Control"]
const hasFiles = (event: DragEvent) => event.dataTransfer.types.includes("Files")
const typing = (target: EventTarget | null) =>
  target instanceof HTMLElement && (target.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName))

/** Project state and rows into the store; nothing here renders. */
function ProjectSync({ id }: { id: string }) {
  const { client, stream, bridge } = useConnection()
  const store = useCanvasStore()
  const navigate = useNavigate()
  const { data: state, error: stateError } = useQuery(projectQuery(client, id))
  const { data: canvas, error: canvasError } = useQuery(canvasQuery(client, id))
  const asked = useRef(new Set<string>())
  const queries = useQueryClient()

  const tools = useSignal(store.tools)
  useEffect(() => {
    if (state) store.applyState(state)
  }, [store, state])
  useEffect(() => {
    // A tool an asset names that the list didn't hold, asked for by its name, once. Before the list has come, that would be all of them.
    if (!tools.size) return
    for (const tool of new Set(state?.assets.flatMap((asset) => asset.tool ?? []))) {
      if (madeHere(tool) || asked.current.has(tool) || tools.has(tool)) continue
      asked.current.add(tool)
      // Local code is listed only once something names it, so the list is read again. Anything else can be searched for.
      if (/^[./~\\]|^\w:[\\/]/.test(tool)) void queries.invalidateQueries({ queryKey: ["tools"] })
      else client.tools(tool.split("/").slice(0, -1).join(" "), null, id).then(({ tools }) => store.addTools(tools), () => {})
    }
  }, [store, client, id, queries, state, tools])
  useEffect(() => {
    if (canvas) store.applyCanvas(canvas.cards)
  }, [store, canvas])
  // A file that doesn't read, said once, and gone when it does: someone's still saving it.
  useEffect(() => {
    if (!stateError) return
    const note = say("Couldn't read this project", stateError)
    if (stateError instanceof ApiError && stateError.status === 404) navigate("/", { replace: true })
    return () => dismiss(note)
  }, [stateError, navigate])
  useEffect(() => {
    if (canvasError) say("Couldn't read where things sit", canvasError)
  }, [canvasError])
  useEffect(
    () =>
      stream.subscribe((event) => {
        if (event.type === "canvas" && event.id === id) store.applyRows(event.cards)
        if (event.type === "resync" || event.type === "hello") store.fellBehind()
      }),
    [stream, store, id],
  )
  useEffect(() => bridge.onRun((run) => store.applyRun(run)), [bridge, store])
  return null
}

/** The machines a render may go to: there's no choosing, the walk places it. With none, Render asks for one under Machines. Null until they're read. */
function useMachines(store: CanvasStore): string[] | null {
  const { client, status } = useConnection()
  const { data, isPending } = useQuery(machinesQuery(client))
  const up = (data?.machines ?? []).filter((machine) => machine.state === "ready" && !machine.offline && !machine.unreachable && (machine.kind !== "local" || status.local_usable))
  const names = [...up.filter((machine) => machine.kind !== "local"), ...up.filter((machine) => machine.kind === "local")].map((machine) => machine.name)
  const key = names.join()
  // biome-ignore lint/correctness/useExhaustiveDependencies: the names, as text
  useEffect(() => {
    store.machines = () => names
    if (names.length) store.machineUp()
  }, [store, key])
  return isPending ? null : names
}

/** Render all, and how many that is. Approved assets stay unless it's said. */
function RenderAll({ id }: { id: string }) {
  const { client } = useConnection()
  const store = useCanvasStore()
  const { data } = useQuery(projectQuery(client, id))
  const [outdated, setOutdated] = useState(false)
  const assets = data?.assets ?? []
  const fresh = assets.filter((asset) => asset.tool && !madeHere(asset.tool) && !asset.pinned).length
  const late = assets.filter((asset) => asset.pinned && asset.tool && asset.standing === "out_of_date").length
  const also = outdated && late > 0
  if (!fresh && !late) return null
  return (
    <span className="render-all">
      <button type="button" className="tb primary" onClick={() => void store.runAll(also)}>
        Render all {fresh + (also ? late : 0)}
      </button>
      {late > 0 && (
        <label>
          <input type="checkbox" checked={outdated} onChange={(event) => setOutdated(event.target.checked)} />
          Also re-render {late} out of date
        </label>
      )}
    </span>
  )
}

/** One line, and it does what it says when clicked too. */
function Empty({ choose }: { choose: () => void }) {
  const empty = useStore((state) => state.nodes.length === 0)
  if (!empty) return null
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
  onAdopt,
}: {
  projectId: string | null
  slot: HTMLElement | null
  onAdopt: (id: string) => void
}) {
  const connection = useConnection()
  const flow = useReactFlow<ElementNode, Edge>()
  const navigate = useNavigate()
  const { scope, client, bridge } = connection
  const [saved] = useState(() => loadViewport(scope, projectId))
  const [store] = useState(() => new CanvasStore(client, projectId, loadSelection(scope, projectId), loadMore(scope, projectId)))
  const adopt = useRef(onAdopt)
  adopt.current = onAdopt
  const [placer] = useState(() => new Placer(store, client, { adopt: (id) => adopt.current(id) }))
  const [openedOn] = useState(projectId)
  const [veil, setVeil] = useState(false)
  const [shut, setShut] = useState(loadToolsShut)
  const stage = useRef<HTMLDivElement>(null)
  const picker = useRef<HTMLInputElement>(null)
  const settling = useRef<ReturnType<typeof setTimeout>>(undefined)
  // Which machine's packages a list leads with. The list waits to know, or it would be asked for twice, each a wait on the Hub.
  const machines = useMachines(store)
  const machine = machines?.at(-1) ?? null
  const { data: listed } = useQuery({ ...toolsQuery(client, "", machine, projectId), enabled: machines !== null })

  useEffect(() => {
    // A project opened here for the first time is fitted, once. Either way the viewport is kept, so a reload finds it.
    // A project with no rows has nothing to fit at its first read: its first placing is what to look at.
    let fitted = Boolean(saved)
    const fitOnce = async () => {
      if (fitted || !store.projectId) return
      fitted = true
      if (openedOn) await flow.fitView(fitOver(loadToolsShut()))
      saveViewport(scope, store.projectId, flow.getViewport())
    }
    store.onFirstRead = async () => {
      if (flow.getNodes().length || !store.projectId) return fitOnce()
    }
    store.onPlaced = fitOnce
    store.onMore = (names) => {
      if (store.projectId) saveMore(scope, store.projectId, names)
    }
    store.askMachine = () => navigate(store.projectId ? `/projects/${store.projectId}/machines` : "/machines")
    store.attach(flow)
    return () => store.detach()
  }, [store, flow, saved, scope, openedOn, bridge, navigate])

  const fit = useCallback(() => void flow.fitView({ ...fitOver(loadToolsShut()), duration: 150 }), [flow])
  const choose = useCallback(() => picker.current?.click(), [])

  // The middle of what's visible: the canvas between whatever floats over it.
  const visibleCentre = useCallback(() => {
    const box = stage.current!.getBoundingClientRect()
    const left = document.querySelector(".tools")?.getBoundingClientRect().right ?? box.left
    const right = document.querySelector(".drawer")?.getBoundingClientRect().left ?? box.right
    return flow.screenToFlowPosition({ x: (left + right) / 2, y: box.top + box.height / 2 })
  }, [flow])

  // A click puts a box in the middle of what's visible, or the nearest place clear of what's there, so none lands hidden under another.
  const clearSpot = useCallback(
    (width: number, height: number) => {
      const { x: middle, y } = visibleCentre()
      const box = stage.current!.getBoundingClientRect()
      const leftmost = flow.screenToFlowPosition({ x: (document.querySelector(".tools")?.getBoundingClientRect().right ?? box.left) + 24, y: box.top }).x
      const rightmost = flow.screenToFlowPosition({ x: (document.querySelector(".drawer")?.getBoundingClientRect().left ?? box.right) - 24, y: box.top }).x
      const spot = { x: Math.round(middle - width / 2), y: Math.round(y - height / 2) }
      for (let tries = 0; tries < 60; tries++) {
        const under = flow.getIntersectingNodes({ ...spot, width, height })
        if (!under.length) break
        spot.x = Math.round(Math.max(...under.map((node) => node.position.x + (node.width ?? CARD_WIDTH))) + 32)
        if (spot.x + width > rightmost) {
          spot.x = Math.round(leftmost)
          spot.y += Math.round(height + 48)
        }
      }
      return spot
    },
    [flow, visibleCentre],
  )
  const placeTool = useCallback(
    (tool: Tool) => {
      const width = tool.controls.some((control) => control.type === "timeline") ? 720 : tool.kind === "video" ? 360 : 300
      const { x, y } = clearSpot(width, width * 0.75)
      void placer.addTool(tool, { x: x + CARD_WIDTH / 2, y: y + 16 })
    },
    [placer, clearSpot],
  )

  const picked = useCallback(
    (event: ChangeEvent<HTMLInputElement>) => {
      const files = [...(event.target.files ?? [])]
      event.target.value = ""
      if (!files.length) return
      const columns = Math.ceil(Math.sqrt(files.length))
      const { x, y } = clearSpot(columns * CARD_WIDTH, Math.ceil(files.length / columns) * CARD_WIDTH)
      placer.add(files, { x: x + CARD_WIDTH / 2, y: y + 16 })
    },
    [placer, clearSpot],
  )

  const drop = useCallback(
    (event: DragEvent) => {
      const point = flow.screenToFlowPosition({ x: event.clientX, y: event.clientY })
      const tool = event.dataTransfer.getData(DRAGGED_TOOL)
      const taken = event.dataTransfer.getData(DRAGGED_FILE)
      if (tool || taken) {
        event.preventDefault()
        const named = store.tools.get().get(tool)
        if (named) void placer.addTool(named, point)
        if (taken) void placer.addFile(JSON.parse(taken), point)
        return
      }
      if (!hasFiles(event)) return
      event.preventDefault()
      setVeil(false)
      const files = [...event.dataTransfer.files]
      if (files.length) placer.add(files, point)
    },
    [placer, flow, store],
  )

  const moved = useCallback(
    (_: unknown, viewport: Viewport) => {
      if (store.projectId) saveViewport(scope, store.projectId, viewport)
    },
    [store, scope],
  )

  const selectionChanged = useCallback(
    ({ nodes }: OnSelectionChangeParams<ElementNode, Edge>) => {
      store.select(nodes.map((node) => node.data.name))
      clearTimeout(settling.current)
      settling.current = setTimeout(() => {
        if (store.projectId) saveSelection(scope, store.projectId, nodes.map((node) => node.data.name))
      }, 200)
    },
    [store, scope],
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
      store.dropTargetAt(clientX, clientY, draggedEls(nodes))?.over(nodes.map((node) => node.data.name), clientX, clientY)
    },
    [store],
  )
  const dragStop = useCallback(
    (event: Pointing, nodes: ElementNode[]) => {
      carrying(false)
      const { clientX, clientY } = pointOf(event)
      const target = store.dropTargetAt(clientX, clientY, draggedEls(nodes))
      if (!target) {
        store.setHot(null)
        return store.dragStopped(nodes)
      }
      store.dragDropped(nodes)
      target.drop(nodes.map((node) => node.data.name), clientX, clientY)
      // The box stays open under the pointer until it moves.
      window.addEventListener("pointermove", () => store.setHot(null), { once: true })
    },
    [store],
  )

  useEffect(() => {
    const key = (event: KeyboardEvent) => {
      if (typing(event.target)) return
      const command = event.metaKey || event.ctrlKey
      if (event.key === "Escape") {
        store.menu.set(null)
        store.editing.set(null)
        for (const node of flow.getNodes()) {
          const placing = store.get(node.data.name)?.placing
          if (node.selected && placing && placing.stage !== "adding") store.cancel(store.resolveName(node.data.name))
        }
        flow.setNodes((nodes) => nodes.map((node) => (node.selected ? { ...node, selected: false } : node)))
      } else if (event.key === "Delete" || (event.key === "Backspace" && !command)) {
        const names = flow.getNodes().filter((node) => node.selected && node.type === "element").map((node) => node.data.name)
        if (names.length) {
          event.preventDefault()
          void store.remove(names)
        }
      } else if (command && event.key.toLowerCase() === "a") {
        event.preventDefault()
        flow.setNodes((nodes) => nodes.map((node) => ({ ...node, selected: true })))
      } else if (command && event.key === "0") {
        event.preventDefault()
        void flow.zoomTo(1, { duration: 150 })
      } else if (command && (event.key === "=" || event.key === "+")) {
        event.preventDefault()
        void flow.zoomIn({ duration: 150 })
      } else if (command && event.key === "-") {
        event.preventDefault()
        void flow.zoomOut({ duration: 150 })
      } else if (!command && (event.key === "f" || event.key === "!")) {
        fit()
      }
    }
    window.addEventListener("keydown", key)
    return () => window.removeEventListener("keydown", key)
  }, [flow, store, fit])

  // Far out, a card shows its picture, its name and little else.
  const band = useStore((state) => (state.transform[2] < 0.2 ? 2 : state.transform[2] < 0.45 ? 1 : 0))
  const accepted = useCallback((event: DragEvent) => event.dataTransfer.types.includes(DRAGGED_TOOL) || event.dataTransfer.types.includes(DRAGGED_FILE), [])

  // The list a box's controls come from, whatever the panel is searching.
  useEffect(() => {
    if (listed) store.addTools(listed.tools)
  }, [store, listed])

  return (
    <CanvasStoreProvider value={store}>
      <ToolPanel
        machine={machine}
        waiting={machines === null}
        shut={shut}
        setShut={(next) => {
          setShut(next)
          saveToolsShut(next)
        }}
        place={placeTool}
        choose={choose}
      />
      <div
        ref={stage}
        className="stage"
        data-tools={shut ? "shut" : "open"}
        data-lod={band}
        onDragEnter={(event) => {
          if (hasFiles(event)) setVeil(true)
        }}
        onDragOver={(event) => {
          if (!hasFiles(event) && !accepted(event)) return
          event.preventDefault()
          event.dataTransfer.dropEffect = "copy"
        }}
        onDragLeave={(event) => {
          if (!event.currentTarget.contains(event.relatedTarget as globalThis.Node | null)) setVeil(false)
        }}
        onDrop={drop}
      >
        <ReactFlow<ElementNode, Edge>
          defaultNodes={[]}
          nodeTypes={nodeTypes}
          defaultViewport={saved ?? undefined}
          panOnScroll
          panOnScrollSpeed={1}
          selectionMode={SelectionMode.Partial}
          zoomOnDoubleClick={false}
          minZoom={0.05}
          maxZoom={4}
          multiSelectionKeyCode={MULTI}
          deleteKeyCode={null}
          disableKeyboardA11y
          nodesConnectable={false}
          onlyRenderVisibleElements
          colorMode="dark"
          attributionPosition="bottom-right"
          onMoveEnd={moved}
          onSelectionChange={selectionChanged}
          onPaneClick={() => store.editing.set(null)}
          onNodeContextMenu={(event, node) => {
            if (node.type !== "element") return
            event.preventDefault()
            store.menu.set({ name: node.data.name, x: event.clientX, y: event.clientY })
          }}
          onNodeDragStart={(_, __, nodes) => dragStart(nodes)}
          onNodeDrag={(event, _, nodes) => dragging(event, nodes)}
          onNodeDragStop={(event, _, nodes) => dragStop(event, nodes)}
          onSelectionDragStart={(_, nodes) => dragStart(nodes)}
          onSelectionDrag={(event, nodes) => dragging(event, nodes)}
          onSelectionDragStop={(event, nodes) => dragStop(event, nodes)}
        >
          <Background variant={BackgroundVariant.Dots} gap={24} size={2} color="var(--dot)" />
          <Empty choose={choose} />
        </ReactFlow>
        {veil && <div className="dropveil" />}
      </div>
      <AssetMenu />
      <input ref={picker} className="picker" type="file" multiple accept={ACCEPT} onChange={picked} aria-label="Add a file" />
      {slot && projectId && createPortal(<RenderAll id={projectId} />, slot)}
      {projectId && <ProjectSync id={projectId} />}
    </CanvasStoreProvider>
  )
}

/** Always mounted: everything else is a drawer over it, so opening one loses no viewport, no draft and no import in flight. */
export function CanvasHost(props: { projectId: string | null; slot: HTMLElement | null; onAdopt: (id: string) => void }) {
  return (
    <ReactFlowProvider defaultNodes={[]}>
      <Canvas {...props} />
    </ReactFlowProvider>
  )
}
