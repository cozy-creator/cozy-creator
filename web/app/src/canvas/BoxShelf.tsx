import { keepPreviousData, useQuery } from "@tanstack/react-query"
import { useEffect, useState } from "react"
import { useConnection } from "../app/Connection"
import type { FoundModel } from "../types"
import { useSignal } from "../ui/signal"
import { AddOnRow } from "./AddOns"
import { addOnsOf, carrying, listed, takenOff } from "./checkpoints"
import { useCanvasStore, useOfElement } from "./context"
import { useDragScroll } from "./dragScroll"
import { loadOf, shortName } from "./ModelBrowse"

/** As many as one search of the Hub reads. */
const MOST = 100

const own = (ref?: string) => ref?.split("@")[0]

/**
 * The left bar while a box whose tool runs a model is open (Paul, 11-06): what can go on it, in place of the tools, each picked with a click or dragged
 * onto a box. Its model, the default first, and its LoRAs, those on it first with their strength and ×, then the rest of its family's. What its tool runs
 * inside isn't here: those have no family, and Turbo is the box's own switch.
 */
export function BoxShelf({ name }: { name: string }) {
  const { client } = useConnection()
  const store = useCanvasStore()
  // What it runs and what's on it: typing in it renders nothing here.
  useOfElement(name, (view) => JSON.stringify([view?.asset?.tool, store.valueOf(name, "lora"), store.valueOf(name, "models")]))
  const tools = useSignal(store.tools)
  const tool = tools.get(store.get(name)?.asset?.tool ?? "")
  const model = tool?.model
  const [typed, setTyped] = useState("")
  const [q, setQ] = useState("")
  useEffect(() => {
    const timer = setTimeout(() => setQ(typed.trim()), 250)
    return () => clearTimeout(timer)
  }, [typed])
  const { data, error, isPending } = useQuery({
    queryKey: ["models", q, MOST, model?.family],
    queryFn: ({ signal }) => client.models(q, MOST, signal, model?.family),
    enabled: Boolean(model?.family),
    staleTime: 60_000,
    placeholderData: keepPreviousData,
  })
  const found = (data?.models ?? []).filter((one) => listed(one) && one.family === model?.family)
  const list = useDragScroll({
    lift(pressed) {
      const from = pressed.closest<HTMLElement>("[data-pick]")
      const one = from && found.find((held) => held.model === from.dataset.pick)
      return one ? { load: loadOf(one, tools), from } : null
    },
  })
  if (!tool || !model) return null
  const lora = store.valueOf(name, "lora")
  const models = store.valueOf(name, "models")
  const held = addOnsOf(lora, models)
  const base = own(model.default)
  const fine = held.find((addOn) => !addOn.lora && addOn.slot === model.slot)
  const current = fine?.ref ?? base
  // Another function's own model in its package is its Turbo: the box's switch, not a pick.
  const switches = new Set([...tools.values()].flatMap((other) => (other !== tool && other.package === tool.package && other.model?.default ? [own(other.model.default)] : [])))
  const words = (ref: string) => shortName(ref, model.family)
  const shownBase = base && (!q || base.toLowerCase().includes(q.toLowerCase()))
  const checkpoints = found.filter((one) => one.kind === "weights" && one.model !== base && !switches.has(one.model))
  const loras = found.filter((one) => one.kind === "lora" && !held.some((addOn) => addOn.lora && addOn.ref === one.model))
  const swap = (ref: string) => {
    if (ref === current) return
    if (ref === base && fine) return store.putOn(name, takenOff(fine, lora, models))
    const one = found.find((held) => held.model === ref)
    if (one) carrying(one, tools).put(store, name)
  }
  const add = (one: FoundModel) => carrying(one, tools).put(store, name)
  return (
    <>
      <div className="box-shelf-head" title={`${name}: ${tool.label}`}>
        <b>{name}</b>
        <span>{tool.label}</span>
      </div>
      <input className="field wide" aria-label={`Search what goes on ${name}`} placeholder="Search" spellCheck={false} value={typed} onChange={(event) => setTyped(event.target.value)} />
      <div ref={list} className="browse picks" aria-label={`What goes on ${name}`}>
        {error ? <p className="warn">{error instanceof Error ? error.message : String(error)}</p> : null}
        {isPending && !error && <p className="muted">Looking</p>}
        <section data-picks="model">
          <h4>Model</h4>
          <ul>
            {shownBase && (
              <li className={`browsed pick ${current === base ? "on" : ""}`} data-pick={base} title={`${base}: ${tool.label}'s own`} onClick={() => swap(base)}>
                <b>{words(base)}</b>
                <span>Default</span>
              </li>
            )}
            {checkpoints.map((one) => (
              <li key={one.model} className={`browsed pick ${current === one.model ? "on" : ""}`} data-pick={one.model} title={`${one.model}: a fine-tune of ${tool.label}`} onClick={() => swap(one.model)}>
                <b>{words(one.model)}</b>
                <span>Fine-tune</span>
              </li>
            ))}
          </ul>
        </section>
        <section data-picks="lora">
          <h4>LoRAs</h4>
          {held.some((addOn) => addOn.lora) && (
            <div className="picks-on">
              {held.flatMap((addOn) => (addOn.lora ? [<AddOnRow key={`${addOn.slot}:${addOn.ref}`} name={name} addOn={addOn} />] : []))}
            </div>
          )}
          <ul>
            {loras.map((one) => (
              <li key={one.model} className="browsed pick" data-pick={one.model} title={`${one.model}: a LoRA for ${tool.label}. A click puts it on ${name}`} onClick={() => add(one)}>
                <b>{words(one.model)}</b>
                <span>LoRA</span>
              </li>
            ))}
          </ul>
          {data && !loras.length && <p className="muted">{q ? `No other LoRA for "${q}"` : "No other LoRA"}</p>}
        </section>
      </div>
    </>
  )
}
