import { useQuery } from "@tanstack/react-query"
import { useEffect, useMemo, useState } from "react"
import { toolsQuery } from "../api/queries"
import { useConnection } from "../app/Connection"
import type { Tool } from "../api/calcifer"
import { Icon } from "../ui/Icon"
import type { Load } from "./carry"
import { useCanvasStore } from "./context"
import { modelLabel, paletteEntries, withBuiltIns, type Operation, type PaletteEntry } from "./palette"
import { useDragScroll } from "./dragScroll"
import { toolName } from "./shown"
import { carryingTool } from "./toolchange"
import "./ToolPanel.css"

/** A tool is created or attached only when its implementation is dragged. */
const toolLoad = (tool: Tool): Load => ({ tool, box: carryingTool(tool) })

/** A tool to drag out, called `label`; its package, what it does and where it's from on hover. */
function ToolRow({ entry, tool, label, top }: { entry: PaletteEntry; tool: Tool; label: string; top?: boolean }) {
  const said = tool.package === "cozy" ? [entry.label] : [toolName(tool), tool.description]
  return (
    <li className="tool tool-implementation" data-entry={top ? entry.key : undefined} data-implementation="" data-tool={tool.id} data-tool-operation={entry.key} data-kind={tool.makes} title={[...said, tool.source && `From source: ${tool.source}`].filter(Boolean).join("\n")}>
      <b>{label}</b>
    </li>
  )
}

/** An operation of several tools opens to them; one of a single tool is that tool, called what the operation is, with nothing to open (Paul, 11-16:
 *  "why do I have a video timeline tool, section, for just one node"); one of none says there's none. */
function ToolGroup({ entry, open, toggle }: { entry: PaletteEntry; open: boolean; toggle(): void }) {
  if (entry.variants.length === 1) return <ToolRow entry={entry} tool={entry.variants[0]} label={entry.label} top />
  return (
    <li className="tool-group" data-entry={entry.key} data-kind={entry.makes}>
      <button type="button" className="tool tool-category" data-operation={entry.key} data-kind={entry.makes} aria-expanded={open} aria-disabled={!entry.variants.length} disabled={!entry.variants.length} title={entry.variants.length ? `Choose a ${entry.label} tool` : `No ${entry.label} tool available`} onClick={toggle}>
        <b>{entry.label}</b><span aria-hidden="true">{open ? "⌄" : "›"}</span>
      </button>
      {open && <ul className="tool-implementations">
        {entry.variants.map((tool) => <ToolRow key={tool.id} entry={entry} tool={tool} label={entry.labels.get(tool.id) ?? modelLabel(tool)} />)}
      </ul>}
    </li>
  )
}

/** Clicks browse; only a drag creates a box or attaches a tool. */
export function ToolPanel({ project, shut, setShut }: { project: string | null; shut: boolean; setShut: (shut: boolean) => void }) {
  const { client } = useConnection()
  const store = useCanvasStore()
  const [open, setOpen] = useState<Operation | null>(null)
  const { data, error, isPlaceholderData } = useQuery(toolsQuery(client, "", project))
  useEffect(() => {
    store.addTools(withBuiltIns(data && !isPlaceholderData ? data.tools : []))
  }, [store, data, isPlaceholderData])
  const tools = data?.tools
  const entries = useMemo(() => paletteEntries(tools ?? []), [tools])
  const list = useDragScroll({
    lift(pressed) {
      const from = pressed.closest<HTMLElement>("li[data-implementation]")
      const entry = entries.find((entry) => entry.key === from?.dataset.toolOperation)
      const tool = entry?.variants.find((tool) => tool.id === from?.dataset.tool)
      return from && tool ? { load: toolLoad(tool), from } : null
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
          <section data-palette="tools">
            {error ? <p className="warn">{error instanceof Error ? error.message : String(error)}</p> : null}
            <ul>{entries.map((entry) => <ToolGroup key={entry.key} entry={entry} open={open === entry.key} toggle={() => setOpen(open === entry.key ? null : entry.key)} />)}</ul>
          </section>
        </div>
      </div>
    </aside>
  )
}
