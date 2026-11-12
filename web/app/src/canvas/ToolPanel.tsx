import { useQuery } from "@tanstack/react-query"
import { useEffect, useState } from "react"
import { toolsQuery } from "../api/queries"
import { useConnection } from "../app/Connection"
import type { Tool } from "../api/calcifer"
import { Icon } from "../ui/Icon"
import type { Load } from "./carry"
import { useCanvasStore } from "./context"
import { TEXT_TOOL, VIDEO_EDIT_TOOL } from "./controls"
import { BASIC_NODES, modelLabel, paletteEntries, type BasicNodeKind, type Operation, type PaletteEntry } from "./palette"
import { useDragScroll } from "./dragScroll"
import { carryingTool } from "./toolchange"
import "./ToolPanel.css"

/** A tool is created or attached only when its implementation is dragged. */
const toolLoad = (tool: Tool, operation: Operation): Load => ({ tool, operation, box: {
  ...carryingTool(tool),
  put: (store, name) => store.putTool(name, tool, operation),
} })

function ToolGroup({ entry, open, toggle }: { entry: PaletteEntry; open: boolean; toggle(): void }) {
  return (
    <li className="tool-group">
      <button type="button" className="tool tool-category" data-operation={entry.key} data-kind={entry.makes} aria-expanded={open} aria-disabled={!entry.variants.length} disabled={!entry.variants.length} title={entry.variants.length ? `Choose a ${entry.label} tool` : `No ${entry.label} tool available`} onClick={toggle}>
        <b>{entry.label}</b><span aria-hidden="true">{open ? "⌄" : "›"}</span>
      </button>
      {open && <ul className="tool-implementations">
        {entry.variants.map((tool) => <li key={tool.id} className="tool tool-implementation" data-implementation="" data-tool={tool.id} data-tool-operation={entry.key} data-kind={tool.makes} title={tool.description || modelLabel(tool)}><b>{modelLabel(tool)}</b></li>)}
      </ul>}
    </li>
  )
}

/** Clicks browse; only a drag creates a node or attaches a tool. */
export function ToolPanel({ project, machine, waiting, shut, setShut }: { project: string | null; machine: string | null; waiting: boolean; shut: boolean; setShut: (shut: boolean) => void }) {
  const { client } = useConnection()
  const store = useCanvasStore()
  const [open, setOpen] = useState<Operation | null>(null)
  const [selected, setSelected] = useState<BasicNodeKind | null>(null)
  const { data, error, isPlaceholderData } = useQuery({ ...toolsQuery(client, "", machine, project), enabled: !waiting })
  useEffect(() => {
    store.addTools(data && !isPlaceholderData ? [TEXT_TOOL, ...data.tools.filter((tool) => tool.id !== VIDEO_EDIT_TOOL.id), VIDEO_EDIT_TOOL] : [TEXT_TOOL, VIDEO_EDIT_TOOL])
  }, [store, data, isPlaceholderData])
  const entries = paletteEntries(data?.tools ?? [])
  const list = useDragScroll({
    lift(pressed) {
      const from = pressed.closest<HTMLElement>("li[data-node], li[data-implementation]")
      if (!from) return null
      const node = BASIC_NODES.find((node) => node.kind === from.dataset.node)
      if (node) return { load: { node: node.kind }, from }
      const entry = entries.find((entry) => entry.key === from.dataset.toolOperation)
      const tool = entry?.variants.find((tool) => tool.id === from.dataset.tool)
      return tool && entry ? { load: toolLoad(tool, entry.key), from } : null
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
            <ul>{BASIC_NODES.map((node) => <li key={node.kind} className={`tool tool-node ${selected === node.kind ? "selected" : ""}`} data-node={node.kind} data-kind={node.kind} onClick={() => setSelected(node.kind)}><b>{node.label}</b></li>)}</ul>
          </section>
          <section data-palette="tools">
            <h4>Tools</h4>
            {error ? <p className="warn">{error instanceof Error ? error.message : String(error)}</p> : null}
            <ul>{entries.map((entry) => <ToolGroup key={entry.key} entry={entry} open={open === entry.key} toggle={() => setOpen(open === entry.key ? null : entry.key)} />)}</ul>
            {data?.note && <p className="muted">{data.note}</p>}
          </section>
        </div>
      </div>
    </aside>
  )
}
