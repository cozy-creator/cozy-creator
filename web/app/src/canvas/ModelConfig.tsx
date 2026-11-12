import { useSignal } from "../ui/signal"
import { useCanvasStore, useElement } from "./context"
import { modelChoiceLabel, type PickerSlot, slotsOf } from "./ModelPicker"

type Entry = Record<string, unknown>
type Row = { ref: string; slot: string; lora: boolean; strength: unknown }
const entryOf = (value: unknown): Entry => value && typeof value === "object" && !Array.isArray(value) ? value as Entry : {}
const entries = (value: unknown): Entry[] => Array.isArray(value) ? value.map(entryOf) : []
const isDefault = (ref: string, declared?: string | null) => ref === declared || (!ref.includes("@") && ref === declared?.split("@")[0])

/** What one key of `models` sets: its model unless it's the default, then its LoRAs. */
function rowsOf(value: unknown, slot: string, declared: PickerSlot | undefined): Row[] {
  const held = entryOf(value)
  const primary = typeof held.ref === "string" && !isDefault(held.ref, declared?.default) ? [{ ref: held.ref, slot, lora: declared?.kind === "lora", strength: held.strength }] : []
  return [...primary, ...entries(held.lora).flatMap((adapter) => typeof adapter.ref === "string" ? [{ ref: adapter.ref, slot, lora: true, strength: adapter.strength }] : [])]
}

/** The effective overrides remain visible while the picker is closed: each choice once, however many keys it sets. */
export function ModelConfig({ name }: { name: string }) {
  const store = useCanvasStore()
  const view = useElement(name)
  const tools = useSignal(store.tools)
  const slots = slotsOf(tools.get(view?.asset?.tool ?? ""))
  const models = entryOf(store.valueOf(name, "models"))
  const chosen = new Set(slots.flatMap((slot) => slot.keys))
  const rows = [
    ...slots.flatMap((slot) => rowsOf(models[slot.slot], slot.slot, slot)),
    ...Object.entries(models).filter(([key]) => !chosen.has(key)).flatMap(([key, value]) => rowsOf(value, key, undefined)),
  ]
  for (const held of entries(store.valueOf(name, "lora"))) if (typeof held.ref === "string") rows.push({ ref: held.ref, slot: slots[0]?.slot ?? "model", lora: true, strength: held.strength })
  if (!rows.length) return null
  return <div className="active-models" aria-label="Active models and LoRAs">{[false, true].map((lora) => {
    const group = rows.filter((row) => row.lora === lora)
    return group.length ? <section key={String(lora)}><h4>{lora ? "LoRAs:" : "Models:"}</h4><ul>{group.map((row, index) => <li key={`${row.slot}:${row.ref}:${index}`}><span className="active-model-name" title={row.ref}>{modelChoiceLabel(row.ref)}</span>{lora && <Strength value={row.strength} />}</li>)}</ul></section> : null
  })}</div>
}

function Strength({ value }: { value: unknown }) {
  // The renderer's default for an omitted override is 1.0.
  const strength = value == null ? 1 : typeof value === "number" && Number.isFinite(value) ? value : undefined
  return <span className="active-model-strength">{strength === undefined ? ", Invalid weight" : `, ${Number.isInteger(strength) ? strength.toFixed(1) : strength} / 1.0`}</span>
}
