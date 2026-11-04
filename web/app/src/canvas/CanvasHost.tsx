import { useQuery } from "@tanstack/react-query"
import {
  Background,
  BackgroundVariant,
  type OnSelectionChangeParams,
  ReactFlow,
  ReactFlowProvider,
  SelectionMode,
  useReactFlow,
  useStore,
  type Viewport,
} from "@xyflow/react"
import { type ChangeEvent, type DragEvent, useCallback, useEffect, useRef, useState } from "react"
import { createPortal } from "react-dom"
import { Link, useNavigate } from "react-router-dom"
import { ApiError } from "../api/client"
import { canvasQuery, machinesQuery, projectQuery } from "../api/queries"
import { useConnection } from "../app/Connection"
import { loadMachine, loadSelection, loadViewport, saveMachine, saveSelection, saveViewport } from "../app/view"
import { say } from "../ui/notes"
import { signal, useSignal } from "../ui/signal"
import { CanvasStore, type ElementNode, type GraphEdge } from "./CanvasStore"
import { Card } from "./Card"
import { CanvasStoreProvider, useCanvasStore } from "./context"
import { EdgeTip, edgeTypes } from "./Edges"
import { EntityCard } from "./EntityCard"
import { Placer } from "./place"
import { TextSheet } from "./TextSheet"

const nodeTypes = { element: Card, entity: EntityCard }
// Nothing fitted goes under the dock.
const FIT = { maxZoom: 1, padding: { top: "24px", right: "24px", bottom: "88px", left: "24px" } } as const

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

  useEffect(() => {
    if (state) store.applyState(state)
  }, [store, state])
  useEffect(() => {
    if (canvas) store.applyCanvas(canvas.cards)
  }, [store, canvas])
  useEffect(() => {
    if (!stateError) return
    say("Couldn't read this project", stateError)
    if (stateError instanceof ApiError && stateError.status === 404) navigate("/", { replace: true })
  }, [stateError, navigate])
  useEffect(() => {
    if (canvasError) say("Couldn't read where things sit", canvasError)
  }, [canvasError])
  useEffect(
    () =>
      stream.subscribe((event) => {
        if (event.type === "canvas" && event.id === id) store.applyRows(event.cards)
        if (event.type === "resync" || event.type === "hello") store.clearMasks()
      }),
    [stream, store, id],
  )
  useEffect(() => bridge.onRun((run) => store.applyRun(run)), [bridge, store])
  return null
}

/** The machine a new take goes to: the last used, else this computer if it runs models, else one that's up. Renting is under Machines. */
function CurrentMachine({ store, machinesLink }: { store: CanvasStore; machinesLink: string }) {
  const { client, scope, status } = useConnection()
  const { data } = useQuery(machinesQuery(client))
  const [chosen, choose] = useState(() => loadMachine(scope))
  const up = (data?.machines ?? []).filter((machine) => machine.state === "ready" && !machine.offline && !machine.unreachable && (machine.kind !== "local" || status.local_usable))
  const current = up.find((machine) => machine.name === chosen)?.name ?? up.find((machine) => machine.kind === "local")?.name ?? up[0]?.name ?? null
  useEffect(() => {
    store.machine = () => current
  }, [store, current])
  if (!current) {
    return (
      <Link className="tb" to={machinesLink}>
        No machine up
      </Link>
    )
  }
  return (
    <select
      className="tb"
      aria-label="The machine new takes go to"
      value={current}
      onChange={(event) => {
        choose(event.target.value)
        saveMachine(scope, event.target.value)
      }}
    >
      {up.map((machine) => (
        <option key={machine.name} value={machine.name}>
          {machine.name}
        </option>
      ))}
    </select>
  )
}

function Controls({ store, fit, choose }: { store: CanvasStore; fit: () => void; choose: () => void }) {
  const zoom = useStore((state) => Math.round(state.transform[2] * 100))
  const id = store.projectId
  return (
    <>
      <span className="sum">{zoom}%</span>
      <button type="button" className="tb" title="Fit everything (F)" onClick={fit}>
        Fit
      </button>
      <CurrentMachine store={store} machinesLink={id ? `/projects/${id}/machines` : "/machines"} />
      <button type="button" className="tb primary" onClick={choose}>
        Add file
      </button>
    </>
  )
}

/** The bottom of the screen. It offers only what's built: placing. */
function Dock({ choose, selected }: { choose: () => void; selected: ReturnType<typeof signal<number>> }) {
  const count = useSignal(selected)
  return (
    <div className="dock">
      <button type="button" className="tb" onClick={choose}>
        Add file
      </button>
      <span className="dock-hint">Drop a picture, a video or a sound anywhere on the canvas</span>
      <span className="sp" />
      {count > 0 && <span className="sum">{count} selected</span>}
    </div>
  )
}

function Empty({ choose }: { choose: () => void }) {
  const empty = useStore((state) => state.nodes.length === 0)
  if (!empty) return null
  return (
    <div className="empty-canvas">
      <p>Drop a picture, a video or a sound here</p>
      <button type="button" className="tb" onClick={choose}>
        Choose a file
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
  const flow = useReactFlow<ElementNode, GraphEdge>()
  const { scope, client, bridge } = connection
  const [saved] = useState(() => loadViewport(scope, projectId))
  const [store] = useState(() => new CanvasStore(client, projectId, loadSelection(scope, projectId)))
  const adopt = useRef(onAdopt)
  adopt.current = onAdopt
  const [placer] = useState(() => new Placer(store, client, { adopt: (id) => adopt.current(id), refresh: (id) => bridge.refresh(id) }))
  const [openedOn] = useState(projectId)
  const [selected] = useState(() => signal(0))
  const [veil, setVeil] = useState(false)
  const stage = useRef<HTMLDivElement>(null)
  const picker = useRef<HTMLInputElement>(null)
  const settling = useRef<ReturnType<typeof setTimeout>>(undefined)

  useEffect(() => {
    // A project opened here for the first time is fitted, once. Either way the viewport is kept, so a reload finds it.
    // A project with no rows has nothing to fit at its first read: its first placing is what to look at.
    let fitted = Boolean(saved)
    const fitOnce = async () => {
      if (fitted || !store.projectId) return
      fitted = true
      if (openedOn) await flow.fitView(FIT)
      saveViewport(scope, store.projectId, flow.getViewport())
    }
    store.onFirstRead = async () => {
      if (flow.getNodes().length || !store.projectId) return fitOnce()
    }
    store.onPlaced = fitOnce
    store.refresh = () => {
      if (store.projectId) bridge.refresh(store.projectId)
    }
    store.attach(flow)
    return () => store.detach()
  }, [store, flow, saved, scope, openedOn, bridge])

  const fit = useCallback(() => void flow.fitView({ ...FIT, duration: 150 }), [flow])
  const choose = useCallback(() => picker.current?.click(), [])

  // The middle of what's visible: the canvas less any drawer over it.
  const visibleCentre = useCallback(() => {
    const box = stage.current!.getBoundingClientRect()
    const drawer = document.querySelector(".drawer")?.getBoundingClientRect().width ?? 0
    return flow.screenToFlowPosition({ x: box.left + (box.width - drawer) / 2, y: box.top + box.height / 2 })
  }, [flow])

  const picked = useCallback(
    (event: ChangeEvent<HTMLInputElement>) => {
      const files = [...(event.target.files ?? [])]
      event.target.value = ""
      if (files.length) placer.add(files, visibleCentre())
    },
    [placer, visibleCentre],
  )

  const drop = useCallback(
    (event: DragEvent) => {
      if (!hasFiles(event)) return
      event.preventDefault()
      setVeil(false)
      const files = [...event.dataTransfer.files]
      if (files.length) placer.add(files, flow.screenToFlowPosition({ x: event.clientX, y: event.clientY }))
    },
    [placer, flow],
  )

  const moved = useCallback(
    (_: unknown, viewport: Viewport) => {
      if (store.projectId) saveViewport(scope, store.projectId, viewport)
    },
    [store, scope],
  )

  const selectionChanged = useCallback(
    ({ nodes }: OnSelectionChangeParams<ElementNode, GraphEdge>) => {
      selected.set(nodes.length)
      store.select(nodes.map((node) => node.data.name))
      if (stage.current) stage.current.dataset.focus = nodes.length ? "on" : ""
      clearTimeout(settling.current)
      settling.current = setTimeout(() => {
        if (store.projectId) saveSelection(scope, store.projectId, nodes.map((node) => node.data.name))
      }, 200)
    },
    [selected, store, scope],
  )

  const dragStart = useCallback(
    (nodes: ElementNode[]) => store.dragStarted(nodes.map((node) => node.data.name)),
    [store],
  )
  const dragStop = useCallback((nodes: ElementNode[]) => store.dragStopped(nodes), [store])

  useEffect(() => {
    const key = (event: KeyboardEvent) => {
      if (typing(event.target)) return
      const command = event.metaKey || event.ctrlKey
      if (event.key === "Escape") {
        store.open.set(null)
        for (const node of flow.getNodes()) {
          const placing = store.get(node.data.name)?.placing
          if (node.selected && placing && placing.stage !== "adding") store.cancel(store.resolveName(node.data.name))
        }
        flow.setNodes((nodes) => nodes.map((node) => (node.selected ? { ...node, selected: false } : node)))
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

  return (
    <CanvasStoreProvider value={store}>
      <div
        ref={stage}
        className="stage"
        data-lod={band}
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
        <ReactFlow<ElementNode, GraphEdge>
          defaultNodes={[]}
          defaultEdges={[]}
          nodeTypes={nodeTypes}
          edgeTypes={edgeTypes}
          edgesFocusable={false}
          edgesReconnectable={false}
          defaultViewport={saved ?? undefined}
          panOnScroll
          panOnScrollSpeed={1}
          selectionOnDrag
          panOnDrag={[1]}
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
          attributionPosition="bottom-left"
          onMoveEnd={moved}
          onSelectionChange={selectionChanged}
          onNodeDragStart={(_, __, nodes) => dragStart(nodes)}
          onNodeDragStop={(_, __, nodes) => dragStop(nodes)}
          onSelectionDragStart={(_, nodes) => dragStart(nodes)}
          onSelectionDragStop={(_, nodes) => dragStop(nodes)}
        >
          <Background variant={BackgroundVariant.Dots} gap={24} size={2} color="var(--dot)" />
          <Empty choose={choose} />
        </ReactFlow>
        {veil && <div className="dropveil">Drop to add to this project</div>}
        <EdgeTip />
      </div>
      <TextSheet />
      <input ref={picker} className="picker" type="file" multiple accept={ACCEPT} onChange={picked} aria-label="Add a file" />
      <Dock choose={choose} selected={selected} />
      {slot && createPortal(<Controls store={store} fit={fit} choose={choose} />, slot)}
      {projectId && <ProjectSync id={projectId} />}
    </CanvasStoreProvider>
  )
}

/** Always mounted: everything else is a drawer over it, so opening one loses no viewport, no draft and no import in flight. */
export function CanvasHost(props: { projectId: string | null; slot: HTMLElement | null; onAdopt: (id: string) => void }) {
  return (
    <ReactFlowProvider defaultNodes={[]} defaultEdges={[]}>
      <Canvas {...props} />
    </ReactFlowProvider>
  )
}
