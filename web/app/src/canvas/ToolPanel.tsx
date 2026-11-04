import { useQuery } from "@tanstack/react-query"
import { useEffect, useState } from "react"
import { toolsQuery } from "../api/queries"
import { useConnection } from "../app/Connection"
import type { Tool } from "../types"
import { useCanvasStore } from "./context"
import { DRAGGED_TOOL } from "./drag"

function ToolRow({ tool, place }: { tool: Tool; place: (tool: Tool) => void }) {
  return (
    <li
      className="tool"
      draggable
      title={tool.description || tool.id}
      onDragStart={(event) => {
        event.dataTransfer.setData(DRAGGED_TOOL, tool.id)
        event.dataTransfer.effectAllowed = "copy"
      }}
      onDoubleClick={() => place(tool)}
    >
      <b>{tool.label}</b>
      <span className="tag">{tool.kind}</span>
    </li>
  )
}

/** What can be dropped on the canvas, searchable, and it folds away. The canvas stays where it is under it. */
export function ToolPanel({ machine, shut, setShut, place, choose }: { machine: string | null; shut: boolean; setShut: (shut: boolean) => void; place: (tool: Tool) => void; choose: () => void }) {
  const { client } = useConnection()
  const store = useCanvasStore()
  const [typed, setTyped] = useState("")
  const [q, setQ] = useState("")
  useEffect(() => {
    const timer = setTimeout(() => setQ(typed.trim()), 250)
    return () => clearTimeout(timer)
  }, [typed])
  const { data, error, isPending } = useQuery({ ...toolsQuery(client, q, machine), staleTime: 60_000 })
  useEffect(() => {
    if (data) store.addTools(data.tools)
  }, [store, data])
  if (shut) {
    return (
      <aside className="tools shut" aria-label="Tools">
        <button type="button" className="iconb" aria-label="Show the tools" title="Show the tools" onClick={() => setShut(false)}>
          ›
        </button>
      </aside>
    )
  }
  return (
    <aside className="tools" aria-label="Tools">
      <header>
        <b>Tools</b>
        <span className="sp" />
        <button type="button" className="iconb" aria-label="Hide the tools" title="Hide the tools" onClick={() => setShut(true)}>
          ‹
        </button>
      </header>
      <button type="button" className="tb" onClick={choose}>
        Add file
      </button>
      <input className="field wide" aria-label="Search the tools" placeholder="Search" spellCheck={false} value={typed} onChange={(event) => setTyped(event.target.value)} />
      {data?.note && <p className="muted">{data.note}</p>}
      {error ? <p className="warn">{error instanceof Error ? error.message : String(error)}</p> : null}
      {isPending && <p className="muted">Looking…</p>}
      {data && !data.tools.length && <p className="muted">{q ? `Nothing makes media for "${q}"` : "No tools here yet"}</p>}
      <ul>
        {data?.tools.map((tool) => (
          <ToolRow key={tool.id} tool={tool} place={place} />
        ))}
      </ul>
    </aside>
  )
}

