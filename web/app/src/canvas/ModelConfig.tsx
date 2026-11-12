import { useQuery } from "@tanstack/react-query"
import { useState } from "react"
import { useConnection } from "../app/Connection"
import { Icon } from "../ui/Icon"
import { useSignal } from "../ui/signal"
import { useCanvasStore, useElement } from "./context"
import { modelChoiceLabel, type PickerSlot, slotsOf } from "./ModelPicker"

type Entry = Record<string, unknown>
/** A LoRA's place: `models[slot].lora[index]`, or the box's own `lora[index]` when `slot` is null. */
type Row = { ref: string; slot: string; lora: boolean; strength: unknown; at?: { slot: string | null; index: number } }
const entryOf = (value: unknown): Entry => value && typeof value === "object" && !Array.isArray(value) ? value as Entry : {}
const entries = (value: unknown): Entry[] => Array.isArray(value) ? value.map(entryOf) : []
const isDefault = (ref: string, declared?: string | null) => ref === declared || (!ref.includes("@") && ref === declared?.split("@")[0])

/** What one key of `models` sets: its model unless it's the default, then its LoRAs. */
function rowsOf(value: unknown, slot: string, declared: PickerSlot | undefined): Row[] {
  const held = entryOf(value)
  const primary = typeof held.ref === "string" && !isDefault(held.ref, declared?.default) ? [{ ref: held.ref, slot, lora: declared?.kind === "lora", strength: held.strength }] : []
  return [...primary, ...entries(held.lora).flatMap((adapter, index) => typeof adapter.ref === "string" ? [{ ref: adapter.ref, slot, lora: true, strength: adapter.strength, at: { slot, index } }] : [])]
}

/** The effective overrides remain visible while the picker is closed; each LoRA's strength is set here too (Paul, 11-12). */
export function ModelConfig({ name }: { name: string }) {
  const store = useCanvasStore()
  const view = useElement(name)
  const tools = useSignal(store.tools)
  const slots = slotsOf(tools.get(view?.asset?.tool ?? ""))
  const models = entryOf(store.valueOf(name, "models"))
  const chosen = new Set(slots.map((slot) => slot.slot))
  const rows = [
    ...slots.flatMap((slot) => rowsOf(models[slot.slot], slot.slot, slot)),
    ...Object.entries(models).filter(([key]) => !chosen.has(key)).flatMap(([key, value]) => rowsOf(value, key, undefined)),
  ]
  entries(store.valueOf(name, "lora")).forEach((held, index) => {
    if (typeof held.ref === "string") rows.push({ ref: held.ref, slot: slots[0]?.slot ?? "model", lora: true, strength: held.strength, at: { slot: null, index } })
  })
  if (!rows.length) return null
  /** One LoRA's strength, written at once: one step in the box's history. */
  const setStrength = (at: NonNullable<Row["at"]>, strength: number) => {
    const put = (list: unknown) => entries(list).map((held, index) => index === at.index ? { ...held, strength } : held)
    if (at.slot === null) return store.putOn(name, { lora: put(store.valueOf(name, "lora")) })
    const all = entryOf(store.valueOf(name, "models"))
    const held = entryOf(all[at.slot])
    store.putOn(name, { models: { ...all, [at.slot]: { ...held, lora: put(held.lora) } } })
  }
  return <div className="active-models" aria-label="Active models and LoRAs">{[false, true].map((lora) => {
    const group = rows.filter((row) => row.lora === lora)
    return group.length ? <section key={String(lora)}><h4>{lora ? "LoRAs:" : "Models:"}</h4><ul>{group.map((row, index) => <li key={`${row.slot}:${row.ref}:${index}`} className={row.at ? "active-lora" : undefined}>
      <span className="active-model-name" title={row.ref}>{modelChoiceLabel(row.ref)}</span>
      {row.at && <Strength name={modelChoiceLabel(row.ref)} reference={row.ref} value={row.strength} onChange={(strength) => setStrength(row.at!, strength)} />}
    </li>)}</ul></section> : null
  })}</div>
}

/** A LoRA's strength: a slider from 0 to 2, its number (exact, or beyond 2), and the LoRA's recommended strength back. */
function Strength({ name, reference, value, onChange }: { name: string; reference: string; value: unknown; onChange(strength: number): void }) {
  const { client } = useConnection()
  // The renderer's default for an omitted strength is 1.
  const held = value == null ? 1 : typeof value === "number" && Number.isFinite(value) ? value : undefined
  const [dragging, setDragging] = useState<number | null>(null)
  const [typed, setTyped] = useState<string | null>(null)
  const { data } = useQuery({
    queryKey: ["model", reference.split("@")[0]], queryFn: ({ signal }) => client.model(reference.split("@")[0], signal),
    staleTime: 60_000, retry: false,
  })
  const recommended = data?.strength ?? 1
  const shown = dragging ?? held
  const commit = (next: number | null) => { if (next !== null && Number.isFinite(next) && next !== held) onChange(Math.round(next * 1000) / 1000) }
  const release = () => { commit(dragging); setDragging(null) }
  const settle = () => { if (typed !== null && typed.trim() && Number.isFinite(Number(typed))) commit(Number(typed)); setTyped(null) }
  return <span className="active-lora-strength">
    <input type="range" min={0} max={2} step={0.05} aria-label={`Strength of ${name}`} value={Math.min(2, Math.max(0, shown ?? 1))}
      onChange={(event) => setDragging(Number(event.target.value))} onPointerUp={release} onKeyUp={release} onBlur={release} />
    <input type="number" className="field" step={0.05} aria-label={`Strength of ${name}, exactly`} value={typed ?? (shown === undefined ? "" : String(shown))}
      onChange={(event) => setTyped(event.target.value)} onBlur={settle} onKeyDown={(event) => { if (event.key === "Enter") { event.preventDefault(); settle() } }} />
    <button type="button" className="iconb" aria-label={`${name}'s recommended strength, ${recommended}`} title={`Recommended: ${recommended}`}
      disabled={held === recommended} onClick={() => onChange(recommended)}><Icon name="undo" /></button>
  </span>
}
