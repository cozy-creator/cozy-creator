import { useQuery } from "@tanstack/react-query"
import { useEffect, useState } from "react"
import { toolsQuery } from "../api/queries"
import { useConnection } from "../app/Connection"
import type { Tool, Tools } from "../api/calcifer"
import { Icon } from "../ui/Icon"
import { say } from "../ui/notes"
import { useSignal } from "../ui/signal"
import type { Load } from "./carry"
import { BoxShelf } from "./BoxShelf"
import { useCanvasStore, useOfElement } from "./context"
import { TEXT_TOOL, VIDEO_EDIT_TOOL } from "./controls"
import { BASIC_NODES, paletteEntries, type BasicNodeKind, type PaletteEntry } from "./palette"
import { useDragScroll } from "./dragScroll"
import { ModelBrowse } from "./ModelBrowse"
import { madeHere } from "./shown"
import { carryingTool } from "./toolchange"
import { ToolBrowse } from "./ToolBrowse"

/** Search the displayed model and its declared functions without listing helpers separately. */
const says = (entry: PaletteEntry, q: string) => {
  const text = `${entry.label} ${entry.model ?? ""} ${entry.variants.map((tool) => `${tool.package} ${tool.label}`).join(" ")}`.toLowerCase()
  return q.toLowerCase().split(/\s+/).every((word) => text.includes(word))
}

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

function ToolRow({ entry, place, yours }: { entry: PaletteEntry; place: (tool: Tool) => void; yours: boolean }) {
  const remove = useRemove()
  const tool = entry.tool
  return (
    <li className="tool" title={tool.description || entry.model || tool.id} data-kind={tool.makes} data-tool={tool.id} data-model={entry.model ?? undefined} onClick={() => place(tool)}>
      <b>{entry.label}</b>
      {yours && !tool.local && !madeHere(tool.id) && (
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

/** What can be dropped on the canvas, by package (Paul, 11-06: "the key point is the package, not the model"), and it folds away. The canvas stays
 *  where it is under it. While a box whose tool runs a model is open, it's what goes on that box instead (Paul, 11-06); leaving the box, the tools
 *  are back as they were, searched and scrolled as they were. */
export function ToolPanel({ project, machine, waiting, shut, setShut, place, placeNode, choose, box }: { project: string | null; machine: string | null; waiting: boolean; shut: boolean; setShut: (shut: boolean) => void; place: (tool: Tool) => void; placeNode: (kind: BasicNodeKind) => void; choose: () => void; box?: string }) {
  const { client } = useConnection()
  const store = useCanvasStore()
  const runs = useOfElement(box ?? "", (view) => view?.asset?.tool ?? undefined)
  const known = useSignal(store.tools)
  const boxed = box && known.get(runs ?? "")?.model ? box : null
  const [typed, setTyped] = useState("")
  const [q, setQ] = useState("")
  const [shelf, setShelf] = useState<"tools" | "browse" | "checkpoints">("tools")
  const browsing = shelf === "browse"
  const exploring = shelf === "checkpoints"
  useEffect(() => {
    const timer = setTimeout(() => setQ(typed.trim()), 250)
    return () => clearTimeout(timer)
  }, [typed])
  const { data, error, isPending, isPlaceholderData } = useQuery({ ...toolsQuery(client, "", machine, project), enabled: !waiting })
  useEffect(() => {
    store.addTools(data && !isPlaceholderData ? [TEXT_TOOL, VIDEO_EDIT_TOOL, ...data.tools] : [TEXT_TOOL, VIDEO_EDIT_TOOL])
  }, [store, data, isPlaceholderData])
  const tools = data?.tools ?? []
  const entries = paletteEntries(tools).filter((entry) => says(entry, q))
  const nodes = BASIC_NODES.filter((node) => q.toLowerCase().split(/\s+/).every((word) => node.label.toLowerCase().includes(word)))
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
        <button type="button" className="tb add" onClick={choose}>
          <Icon name="plus" />
          Add file
        </button>
        <button type="button" className="iconb" aria-label="Hide the tools" title="Hide the tools" onClick={() => setShut(true)}>
          <Icon name="back" />
        </button>
      </div>
      {boxed && <BoxShelf name={boxed} />}
      <div className="tool-shelf" hidden={Boolean(boxed)}>
        <input className="field wide" aria-label={exploring ? "Search the checkpoints" : "Search the tools"} placeholder="Search" spellCheck={false} value={typed} onChange={(event) => setTyped(event.target.value)} />
        <div className="row center">
          <button type="button" className={`act browse-toggle ${browsing ? "on" : ""}`} aria-pressed={browsing} onClick={() => setShelf(browsing ? "tools" : "browse")}>
            {browsing ? "Your tools" : "Browse"}
          </button>
          <button type="button" className={`act browse-toggle ${exploring ? "on" : ""}`} aria-pressed={exploring} onClick={() => setShelf(exploring ? "tools" : "checkpoints")}>
            {exploring ? "Your tools" : "Checkpoints"}
          </button>
        </div>
        {browsing && <ToolBrowse q={q} listed={tools} />}
        {exploring && <ModelBrowse q={q} />}
        <div ref={list} className={`tool-groups ${isPlaceholderData ? "stale" : ""}`} hidden={shelf !== "tools"}>
          <section data-palette="nodes">
            <h4>Nodes</h4>
            <ul>{nodes.map((node) => <li key={node.kind} className="tool" data-node={node.kind} data-kind={node.kind === "video_edit" ? "video" : node.kind} data-tool={node.kind === "text" ? "text" : node.kind === "video_edit" ? "cozy/edit" : undefined} onClick={() => placeNode(node.kind)}><b>{node.label}</b></li>)}</ul>
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
          <ul>{entries.map((entry) => <ToolRow key={entry.key} entry={entry} place={place} yours={!q} />)}</ul>
          {data && !entries.length && !isPlaceholderData && <p className="muted">{q ? `No tools match "${q}"` : "No tools added"}</p>}
          {data?.note && <p className="muted">{data.note}</p>}
          </section>
        </div>
      </div>
    </aside>
  )
}
