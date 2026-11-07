import { keepPreviousData, useQuery } from "@tanstack/react-query"
import { useState } from "react"
import { useConnection } from "../app/Connection"
import type { Tool } from "../types"
import { say } from "../ui/notes"
import { useCanvasStore } from "./context"
import { useDragScroll } from "./dragScroll"

const PAGE = 50

/** The Hub's packages that make media, each with what it does and Add, which puts its functions on your list and installs nothing. */
export function ToolBrowse({ q, listed }: { q: string; listed: Tool[] }) {
  const { client, queries } = useConnection()
  const store = useCanvasStore()
  const [limit, setLimit] = useState(PAGE)
  const [added, setAdded] = useState<string[]>([])
  const list = useDragScroll()
  const { data, error, isPending } = useQuery({
    queryKey: ["browse", q, limit],
    queryFn: ({ signal }) => client.browseTools(q, limit, signal),
    staleTime: 60_000,
    placeholderData: keepPreviousData,
  })
  const add = (name: string) => {
    setAdded((now) => [...now, name])
    store.outbox.add({
      send: (once) => client.changeToolList({ add: name }, once),
      done: () => void queries.invalidateQueries({ queryKey: ["tools"] }),
      refused: (why) => {
        say(`Couldn't add ${name}`, why)
        setAdded((now) => now.filter((held) => held !== name))
      },
    })
  }
  const packages = data?.packages ?? []
  return (
    <div ref={list} className="browse">
      {error ? <p className="warn">{error instanceof Error ? error.message : String(error)}</p> : null}
      {isPending && !error && <p className="muted">Looking</p>}
      <ul>
        {packages.map(({ package: name, summary, tools }) => {
          const has = added.includes(name) || tools.every((id) => listed.some((tool) => tool.id === id))
          return (
            <li key={name} className="browsed">
              <b>{name}</b>
              {summary && <span title={summary}>{summary}</span>}
              <button type="button" className="act" disabled={has} onClick={() => add(name)}>
                {has ? "Added" : "Add"}
              </button>
            </li>
          )
        })}
      </ul>
      {data && !packages.length && <p className="muted">{q ? `Nothing for "${q}"` : "Nothing yet"}</p>}
      {data && data.older > 0 && (
        <button type="button" className="act" onClick={() => setLimit(limit + PAGE)}>
          More
        </button>
      )}
    </div>
  )
}
