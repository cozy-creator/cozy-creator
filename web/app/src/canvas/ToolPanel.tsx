import { useQuery } from "@tanstack/react-query"
import { useEffect, useState } from "react"
import { toolsQuery } from "../api/queries"
import { useConnection } from "../app/Connection"
import type { Tool, Tools } from "../types"
import { Icon, type IconName } from "../ui/Icon"
import { say } from "../ui/notes"
import type { Load } from "./carry"
import { useCanvasStore } from "./context"
import { TEXT_TOOL } from "./controls"
import { useDragScroll } from "./dragScroll"
import { ModelBrowse } from "./ModelBrowse"
import { madeHere } from "./shown"
import { carryingTool } from "./toolchange"
import { ToolBrowse } from "./ToolBrowse"

const GROUPS: { id: string; words: string; icon: IconName }[] = [
  { id: "image", words: "Pictures", icon: "picture" },
  { id: "video", words: "Videos", icon: "video" },
  { id: "audio", words: "Sounds", icon: "sound" },
  { id: "touch", words: "Touch up", icon: "touch" },
  { id: "local", words: "Editable", icon: "edit" },
]

/** What you're after. One that asks for a file and for no words changes what it's given; anything else makes something of its kind. */
const groupOf = (tool: Tool) => tool.local ? "local" : (tool.controls.some((control) => control.type === "file" && control.required) && !tool.controls.some((control) => control.type === "text" || control.type === "group") ? "touch" : tool.kind)

/** A function and its quick form are one row, the quick one its box's Turbo toggle: `generate` and `generate_turbo` of a package. */
function rowsOf(tools: Tool[]): { tool: Tool; quick?: Tool }[] {
  const quick = new Map(tools.map((tool) => [`${tool.package}/${tool.function}`, tool]))
  const taken = new Set<Tool>()
  const rows = tools.map((tool) => {
    // Two of a label are one box, its toggle starting on: `variantOf` flips it.
    if (tools.some((other) => other.variant && other !== tool && other.package === tool.package && other.label === tool.label)) {
      taken.add(tool)
      return { tool }
    }
    const fast = quick.get(`${tool.package}/${tool.function}_turbo`)
    if (fast) taken.add(fast)
    return { tool, quick: fast }
  })
  return rows.filter((row) => !taken.has(row.tool))
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

function ToolRow({ tool, quick, place, yours }: { tool: Tool; quick?: Tool; place: (tool: Tool) => void; yours: boolean }) {
  const remove = useRemove()
  return (
    <li className="tool" title={tool.description || tool.id} data-kind={tool.kind} data-tool={tool.id} onClick={() => place(tool)}>
      <b>{tool.label}</b>
      {yours && !tool.local && !madeHere(tool.id) && (
        <button
          type="button"
          className="tool-off"
          aria-label={`Take ${tool.label} off your list`}
          title="Take off your list"
          onClick={(event) => {
            event.stopPropagation()
            remove([tool.id, ...(quick ? [quick.id] : [])])
          }}
        >
          <Icon name="close" />
        </button>
      )}
    </li>
  )
}

/** What can be dropped on the canvas, by what it makes, and it folds away. The canvas stays where it is under it. */
export function ToolPanel({ project, machine, waiting, shut, setShut, place, choose }: { project: string | null; machine: string | null; waiting: boolean; shut: boolean; setShut: (shut: boolean) => void; place: (tool: Tool) => void; choose: () => void }) {
  const { client } = useConnection()
  const store = useCanvasStore()
  const [typed, setTyped] = useState("")
  const [q, setQ] = useState("")
  const [shelf, setShelf] = useState<"tools" | "browse" | "checkpoints">("tools")
  const browsing = shelf === "browse"
  const exploring = shelf === "checkpoints"
  useEffect(() => {
    const timer = setTimeout(() => setQ(typed.trim()), 250)
    return () => clearTimeout(timer)
  }, [typed])
  const { data, error, isPending, isPlaceholderData } = useQuery({ ...toolsQuery(client, shelf === "tools" ? q : "", machine, project), enabled: !waiting })
  useEffect(() => {
    store.addTools(data && !isPlaceholderData ? [TEXT_TOOL, ...data.tools] : [TEXT_TOOL])
  }, [store, data, isPlaceholderData])
  const tools = data?.tools ?? []
  const list = useDragScroll({
    lift(pressed) {
      const from = pressed.closest<HTMLElement>("li.tool")
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
        {"text".includes(q.toLowerCase()) && (
          <ul>
            <ToolRow tool={TEXT_TOOL} place={place} yours={false} />
          </ul>
        )}
        {error ? <p className="warn">{error instanceof Error ? error.message : String(error)}</p> : null}
        {isPending && !error && (
          <ul aria-hidden="true">
            {Array.from({ length: 8 }, (_, i) => (
              <li key={i} className="tool-ghost" />
            ))}
          </ul>
        )}
        {GROUPS.map((group) => {
          const rows = rowsOf(tools.filter((tool) => groupOf(tool) === group.id))
          if (!rows.length) return null
          return (
            <section key={group.id}>
              <h4>
                <Icon name={group.icon} />
                {group.words}
              </h4>
              <ul>
                {rows.map((row) => (
                  <ToolRow key={row.tool.id} tool={row.tool} quick={row.quick} place={place} yours={!q} />
                ))}
              </ul>
            </section>
          )
        })}
        {data && !tools.length && !isPlaceholderData && <p className="muted">Nothing for "{q}"</p>}
        {data?.note && <p className="muted">{data.note}</p>}
      </div>
    </aside>
  )
}
