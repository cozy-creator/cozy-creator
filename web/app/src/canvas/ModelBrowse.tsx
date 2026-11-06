import { keepPreviousData, useQuery } from "@tanstack/react-query"
import { useState } from "react"
import { useConnection } from "../app/Connection"
import type { FoundModel } from "../types"
import { useSignal } from "../ui/signal"
import { carried, carrying, whyNot, wordOf } from "./checkpoints"
import { useCanvasStore } from "./context"
import { DRAGGED_CHECKPOINT } from "./drag"

const PAGE = 50
const MOST = 100

function Row({ found, away }: { found: FoundModel; away: boolean }) {
  const store = useCanvasStore()
  const what = found.kind === "lora" ? "LoRA" : "Fine-tune"
  const lead = found.family ? `For ${found.family}` : "Can't tell what it's for"
  return (
    <li
      className={`browsed checkpoint ${away ? "away" : ""}`}
      draggable
      data-checkpoint={found.model}
      title={`${found.model}: ${what}. ${lead}`}
      onDragStart={(event) => {
        event.dataTransfer.setData(DRAGGED_CHECKPOINT, found.model)
        event.dataTransfer.effectAllowed = "copy"
        carried.set(carrying(found))
      }}
      onDragEnd={() => {
        carried.set(null)
        store.setHot(null)
      }}
    >
      <b>{wordOf(found.model)}</b>
      <span>{found.family ?? "Can't tell what it's for"}</span>
    </li>
  )
}

/** The Hub's fine-tunes and LoRAs, what fits a box here first, to drag onto one. Listing fetches no weights and installs nothing. */
export function ModelBrowse({ q }: { q: string }) {
  const { client } = useConnection()
  const store = useCanvasStore()
  const tools = useSignal(store.tools)
  useSignal(store.names)
  const [limit, setLimit] = useState(PAGE)
  const { data, error, isPending } = useQuery({
    queryKey: ["models", q, limit],
    queryFn: ({ signal }) => client.models(q, limit, signal),
    staleTime: 60_000,
    placeholderData: keepPreviousData,
  })
  const boxes = store.takenBesides("").flatMap((name) => {
    const view = store.get(name)
    return view?.asset?.tool ? [{ name, view }] : []
  })
  const fits = (found: FoundModel) => boxes.some(({ name, view }) => !whyNot(found, name, view, tools))
  const first = (found: FoundModel[]) => [...found.filter(fits), ...found.filter((one) => !fits(one))]
  const models = data?.models ?? []
  const loras = first(models.filter((found) => found.kind === "lora"))
  const weights = first(models.filter((found) => found.kind === "weights" && found.family))
  // Weights no family knows aren't for a box: they're found by name, and clutter nothing else. What couldn't be read shows, and a drop says so.
  const others = models.filter((found) => !found.kind || (q && found.kind === "weights" && !found.family))
  const section = (words: string, found: FoundModel[]) =>
    found.length > 0 && (
      <section>
        <h4>{words}</h4>
        <ul>
          {found.map((one) => (
            <Row key={one.model} found={one} away={boxes.length > 0 && !fits(one)} />
          ))}
        </ul>
      </section>
    )
  return (
    <div className="browse checkpoints">
      {error ? <p className="warn">{error instanceof Error ? error.message : String(error)}</p> : null}
      {isPending && !error && <p className="muted">Looking</p>}
      {models.length > 0 && <p className="muted">Drag one onto a box</p>}
      {section("LoRAs", loras)}
      {section("Fine-tunes", weights)}
      {section("Can't tell", others)}
      {data && !models.length && <p className="muted">{q ? `Nothing for "${q}"` : "Nothing yet"}</p>}
      {Boolean(data?.older) && limit < MOST && (
        <button type="button" className="act" onClick={() => setLimit(Math.min(MOST, limit + PAGE))}>
          More
        </button>
      )}
    </div>
  )
}
