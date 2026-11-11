import { useQuery } from "@tanstack/react-query"
import { useEffect } from "react"
import { toolsQuery } from "../api/queries"
import { useConnection } from "../app/Connection"
import type { Tool, Tools } from "../api/calcifer"
import { Icon } from "../ui/Icon"
import { say } from "../ui/notes"
import type { Load } from "./carry"
import { useCanvasStore } from "./context"
import { TEXT_TOOL, VIDEO_EDIT_TOOL } from "./controls"
import { BASIC_NODES, paletteEntries, type BasicNodeKind, type PaletteEntry } from "./palette"
import { useDragScroll } from "./dragScroll"
import { madeHere } from "./shown"
import { carryingTool } from "./toolchange"

/** A tool carried out of the list: a box of it where it's let go, or on a box, that box's tool. Words and what's made here are boxes of their own: nothing takes them. */
const toolLoad = (tool: Tool): Load => ({ tool, box: tool === TEXT_TOOL || madeHere(tool.id) ? undefined : carryingTool(tool) })

/** Off your list at once. Nothing is deleted, and a box of it goes on as it was. */
function useRemove() {
  const { client, queries } = useConnection()
  const store = useCanvasStore()
  return (ids: string[]) => {
    queries.setQueriesData<Tools>({ queryKey: ["tools"] }, (list) => list && { ...list, tools: list.tools.filter((tool) => !ids.includes(tool.id)) })
    store.outbox.add({
      send: (once) => client.changeToolList({ remove: ids }, once),
      refused: (error) => {
        say("Couldn't take that off your list", error)
        void queries.invalidateQueries({ queryKey: ["tools"] })
      },
    })
  }
}

function ToolRow({ entry, place }: { entry: PaletteEntry; place: (tool: Tool) => void }) {
  const remove = useRemove()
  const tool = entry.tool
  return (
    <li className="tool" title={tool.description || entry.model || tool.id} data-kind={tool.makes} data-tool={tool.id} data-entry={entry.key} data-model={entry.model ?? undefined} data-local={tool.local || undefined} data-package={tool.package} onClick={() => place(tool)}>
      <b>{entry.label}</b>
      {!tool.local && !madeHere(tool.id) && (
        <button
          type="button"
          className="tool-off"
          aria-label={`Take ${entry.label} off your list`}
          title="Take off your list"
          onClick={(event) => {
            event.stopPropagation()
            remove(entry.variants.map((variant) => variant.id))
          }}
        >
          <Icon name="close" />
        </button>
      )}
    </li>
  )
}

/** The sidebar contains only basic node types and model tools, whether a node is open or closed. */
export function ToolPanel({ project, machine, waiting, shut, setShut, place, placeNode }: { project: string | null; machine: string | null; waiting: boolean; shut: boolean; setShut: (shut: boolean) => void; place: (tool: Tool) => void; placeNode: (kind: BasicNodeKind) => void }) {
  const { client } = useConnection()
  const store = useCanvasStore()
  const { data, error, isPending, isPlaceholderData } = useQuery({ ...toolsQuery(client, "", machine, project), enabled: !waiting })
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
      const tool = from && [TEXT_TOOL, ...tools].find((one) => one.id === from.dataset.tool)
      return tool ? { load: toolLoad(tool), from } : null
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
            {isPending && !error && (
              <ul aria-hidden="true">
                {Array.from({ length: 8 }, (_, i) => (
                  <li key={i} className="tool-ghost" />
                ))}
              </ul>
            )}
            <ul>{entries.map((entry) => <ToolRow key={entry.key} entry={entry} place={place} />)}</ul>
            {data && !entries.length && !isPlaceholderData && <p className="muted">No tools added</p>}
            {data?.note && <p className="muted">{data.note}</p>}
          </section>
        </div>
      </div>
    </aside>
  )
}
