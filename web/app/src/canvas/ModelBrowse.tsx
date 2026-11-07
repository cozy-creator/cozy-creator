import { keepPreviousData, useQuery } from "@tanstack/react-query"
import { useState } from "react"
import { useConnection } from "../app/Connection"
import type { FoundModel, Tool } from "../types"
import { useSignal } from "../ui/signal"
import { NSFW, ShowNsfw, Tags, useShowNsfw } from "../ui/Tags"
import type { Load } from "./carry"
import { carrying, familyName, listed, runnerOf, whyNot, wordOf } from "./checkpoints"
import { useCanvasStore } from "./context"
import { useDragScroll } from "./dragScroll"

const PAGE = 50
const MOST = 100

const spaced = (word: string) => word.replace(/[-_]+/g, " ").trim()

/** A checkpoint's name in its family's group: its own word less the family's, `minimax-h3-vfx-edit` and `h3-vfx-edit` under MiniMax H3 both
 *  `vfx edit`, `qwen-image-2.1-turbo` under Qwen Image 2 `turbo`, and `ikea-lora-sdxl` under SDXL `ikea lora`. The family's own, `minimax-h3`, stays. */
export function shortName(ref: string, family?: string): string {
  const word = wordOf(ref)
  if (!family) return spaced(word)
  const words = family.toLowerCase().split(/[-_]+/)
  // As written, a version on it or not, and its shorter forms that are more than a number: `minimax-h3` and `h3`.
  const said = (form: string) => `${form.replace(/[.*+?^${}()|[\]\\]/g, "\\$&").replace(/-/g, "[-_]")}(?:\\.\\d+)*`
  const forms = words.map((_, i) => words.slice(i).join("-")).filter((form) => /[a-z]/.test(form))
  let short = forms.reduce((held, form) => held.replace(new RegExp(`^${said(form)}[-_]+`, "i"), ""), word)
  short = short.replace(new RegExp(`[-_]+${said(words.join("-"))}$`, "i"), "")
  return spaced(short) || spaced(word)
}

const KIND = { lora: "LoRA", weights: "Fine-tune", component: "Component" } as const

/** What a row says it is, and why it's dimmed if it is: a LoRA runs only on a box of its family, and a checkpoint goes anywhere a tool runs it. */
function rowOf(found: FoundModel, tools: Map<string, Tool>, fits: boolean, family: string | null) {
  const what = found.kind ? KIND[found.kind] : "Can't tell what it is"
  const lora = found.kind !== "weights"
  const runs = Boolean(runnerOf(found, tools))
  const why = !found.kind || !found.family ? `Can't tell what model ${wordOf(found.model)} is for` : lora ? (fits ? null : `No box here runs ${family}`) : runs ? null : `No tool runs ${family} yet`
  return { what, away: lora ? !fits : !runs, title: [`${found.model}: ${what}${family ? ` for ${family}` : ""}`, why].filter(Boolean).join(". ") }
}

function Row({ found, label, row }: { found: FoundModel; label: string; row: ReturnType<typeof rowOf> }) {
  return (
    <li className={`browsed checkpoint ${row.away ? "away" : ""}`} data-checkpoint={found.model} title={row.title}>
      <b>{label}</b>
      <span>{row.what}</span>
      <Tags tags={found.tags} />
    </li>
  )
}

/** What's carried from a row: a checkpoint on open canvas is a box of the tool that runs it, the checkpoint on it unless it's that tool's own. */
export function loadOf(found: FoundModel, tools: Map<string, Tool>): Load {
  const runner = runnerOf(found, tools)
  const own = runner?.model?.default?.split("@")[0] === found.model
  return { box: carrying(found, tools), tool: runner, fields: runner?.model && !own ? { models: { [runner.model.slot]: { ref: found.model } } } : undefined }
}

/** The Hub's fine-tunes and LoRAs by the model they're for, what fits a box here first, to drag onto one; a checkpoint onto open canvas too. Listing
 *  fetches no weights and installs nothing. */
export function ModelBrowse({ q }: { q: string }) {
  const { client } = useConnection()
  const store = useCanvasStore()
  const tools = useSignal(store.tools)
  useSignal(store.names)
  const [limit, setLimit] = useState(PAGE)
  const [showNsfw, setShowNsfw] = useShowNsfw()
  const { data, error, isPending } = useQuery({
    queryKey: ["models", q, limit, showNsfw],
    queryFn: ({ signal }) => client.models(q, limit, signal, { noTag: showNsfw ? undefined : NSFW }),
    staleTime: 60_000,
    placeholderData: keepPreviousData,
  })
  const models = (data?.models ?? []).filter((found) => listed(found, showNsfw))
  const list = useDragScroll({
    lift(pressed) {
      const from = pressed.closest<HTMLElement>("[data-checkpoint]")
      const found = from && models.find((one) => one.model === from.dataset.checkpoint)
      return found ? { load: loadOf(found, tools), from } : null
    },
  })
  const boxes = store.takenBesides("").flatMap((name) => {
    const view = store.get(name)
    return view?.asset?.tool ? [{ name, view }] : []
  })
  const fits = (found: FoundModel) => boxes.some(({ name, view }) => !whyNot(found, name, view, tools))
  // By family, what fits a box here first, then what a tool on your list runs, each as the Hub lists them. Weights no family knows aren't for a box:
  // they're found by name, and clutter nothing else. What has no family, or couldn't be read, is Other, last.
  const families = [...new Set(models.flatMap((found) => (found.family && found.kind ? [found.family] : [])))]
  const rank = (family: string) => (models.some((found) => found.family === family && fits(found)) ? 0 : [...tools.values()].some((tool) => tool.model?.family === family) ? 1 : 2)
  const groups = [...families]
    .sort((a, b) => rank(a) - rank(b))
    .map((family) => ({ family, words: familyName(family, tools), found: models.filter((found) => found.family === family && found.kind) }))
  const other = models.filter((found) => !found.kind || (!found.family && (found.kind === "lora" || q)))
  if (other.length) groups.push({ family: "", words: "Other", found: other })
  const order = (found: FoundModel[]) => [found.filter((one) => one.kind === "lora"), found.filter((one) => one.kind === "weights"), found.filter((one) => !one.kind)].flatMap((kind) => [...kind.filter(fits), ...kind.filter((one) => !fits(one))])
  return (
    <div ref={list} className="browse checkpoints">
      {error ? <p className="warn">{error instanceof Error ? error.message : String(error)}</p> : null}
      {isPending && !error && <p className="muted">Looking</p>}
      <ShowNsfw shown={showNsfw} onChange={setShowNsfw} />
      {models.length > 0 && <p className="muted">Drag one onto a box</p>}
      {groups.map(({ family, words, found }) => {
        const labels = found.map((one) => shortName(one.model, family || undefined))
        // The author only where two would read the same.
        const said = (i: number) => (labels.filter((label) => label === labels[i]).length > 1 ? `${labels[i]} · ${found[i].model.split("/")[0]}` : labels[i])
        const shown = new Map(found.map((one, i) => [one, said(i)]))
        return (
          <section key={family || "other"} data-family={family || undefined}>
            <h4>{words}</h4>
            <ul>
              {order(found).map((one) => (
                <Row key={one.model} found={one} label={shown.get(one)!} row={rowOf(one, tools, fits(one), family ? words : null)} />
              ))}
            </ul>
          </section>
        )
      })}
      {data && !models.length && <p className="muted">{q ? `Nothing for "${q}"` : "Nothing yet"}</p>}
      {Boolean(data?.older) && limit < MOST && (
        <button type="button" className="act" onClick={() => setLimit(Math.min(MOST, limit + PAGE))}>
          More
        </button>
      )}
    </div>
  )
}
