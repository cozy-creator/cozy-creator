import { useQuery } from "@tanstack/react-query"
import { useEffect } from "react"
import { toolsQuery } from "../api/queries"
import { useConnection } from "../app/Connection"
import type { Tool } from "../api/calcifer"
import { Icon } from "../ui/Icon"
import type { Load } from "./carry"
import { useCanvasStore } from "./context"
import { TEXT_TOOL, VIDEO_EDIT_TOOL } from "./controls"
import { BASIC_NODES, paletteEntries, type BasicNodeKind, type Operation, type PaletteEntry } from "./palette"
import { useDragScroll } from "./dragScroll"
import { madeHere } from "./shown"
import { carryingTool } from "./toolchange"

/** A tool carried out of the list: a box of it where it's let go, or on a box, that box's tool. Words and what's made here are boxes of their own: nothing takes them. */
const toolLoad = (tool: Tool, operation: Operation): Load => ({ tool, operation, box: tool === TEXT_TOOL || madeHere(tool.id) ? undefined : {
  ...carryingTool(tool),
  put(store, name) {
    store.putTool(name, tool)
    store.patch(name, { operation })
  },
} })

function ToolRow({ entry, place }: { entry: PaletteEntry; place: (tool: Tool, operation: Operation) => void }) {
  const tool = entry.tool
  return (
    <li className="tool" title={tool ? `${entry.label} — choose a model in the node` : `No ${entry.label} model available`} aria-disabled={!tool} data-kind={entry.makes} data-operation={entry.key} data-tool={tool?.id} onClick={() => tool && place(tool, entry.key)}>
      <b>{entry.label}</b>
    </li>
  )
}

/** The sidebar contains only node types and operations, whether a node is open or closed. */
export function ToolPanel({ project, machine, waiting, shut, setShut, place, placeNode }: { project: string | null; machine: string | null; waiting: boolean; shut: boolean; setShut: (shut: boolean) => void; place: (tool: Tool, operation: Operation) => void; placeNode: (kind: BasicNodeKind) => void }) {
  const { client } = useConnection()
  const store = useCanvasStore()
  const { data, error, isPlaceholderData } = useQuery({ ...toolsQuery(client, "", machine, project, true), enabled: !waiting })
  useEffect(() => {
    store.addTools(data && !isPlaceholderData ? [TEXT_TOOL, VIDEO_EDIT_TOOL, ...data.tools] : [TEXT_TOOL, VIDEO_EDIT_TOOL])
  }, [store, data, isPlaceholderData])
  const tools = data?.tools ?? []
  const entries = paletteEntries(tools)
  const list = useDragScroll({
    lift(pressed) {
      const from = pressed.closest<HTMLElement>("li.tool")
      const node = BASIC_NODES.find((node) => node.kind === from?.dataset.node)
      if (node && from) return { load: { node: node.kind }, from }
      const entry = from && entries.find((entry) => entry.key === from.dataset.operation)
      return entry?.tool && from ? { load: toolLoad(entry.tool, entry.key), from } : null
    },
  })
  if (shut) {
    return (
      <aside className="tools shut" aria-label="Tools">
        <button type="button" className="iconb" aria-label="Show the tools" title="Show the tools" onClick={() => setShut(false)}>
          <Icon name="forward" />
        </button>
      </aside>
    )
  }
  return (
    <aside className="tools" aria-label="Tools">
      <div className="row center">
        <button type="button" className="iconb" aria-label="Hide the tools" title="Hide the tools" onClick={() => setShut(true)}>
          <Icon name="back" />
        </button>
      </div>
      <div className="tool-shelf">
        <div ref={list} className={`tool-groups ${isPlaceholderData ? "stale" : ""}`}>
          <section data-palette="nodes">
            <h4>Nodes</h4>
            <ul>{BASIC_NODES.map((node) => <li key={node.kind} className="tool" data-node={node.kind} data-kind={node.kind === "video_edit" ? "video" : node.kind} data-tool={node.kind === "text" ? "text" : node.kind === "video_edit" ? "cozy/edit" : undefined} onClick={() => placeNode(node.kind)}><b>{node.label}</b></li>)}</ul>
          </section>
          <section data-palette="tools">
            <h4>Tools</h4>
            {error ? <p className="warn">{error instanceof Error ? error.message : String(error)}</p> : null}
            <ul>{entries.map((entry) => <ToolRow key={entry.key} entry={entry} place={place} />)}</ul>
            {data?.note && <p className="muted">{data.note}</p>}
          </section>
        </div>
      </div>
    </aside>
  )
}
