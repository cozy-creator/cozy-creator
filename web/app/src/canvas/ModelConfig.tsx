import { useSignal } from "../ui/signal"
import { useCanvasStore, useElement } from "./context"
import { modelChoiceLabel } from "./ModelPicker"

type Entry = Record<string, unknown>
const entryOf = (value: unknown): Entry => value && typeof value === "object" && !Array.isArray(value) ? value as Entry : {}
const entries = (value: unknown): Entry[] => Array.isArray(value) ? value.map(entryOf) : []
const isDefault = (ref: string, declared?: string | null) => ref === declared || (!ref.includes("@") && ref === declared?.split("@")[0])

/** The effective overrides remain visible while the picker is closed. */
export function ModelConfig({ name }: { name: string }) {
  const store = useCanvasStore()
  const view = useElement(name)
  const tools = useSignal(store.tools)
  const tool = tools.get(view?.asset?.tool ?? "")
  const models = entryOf(store.valueOf(name, "models"))
  const slots = tool?.models?.length ? tool.models : tool?.model ? [tool.model] : []
  const rows = Object.entries(models).flatMap(([slot, value]) => {
    const held = entryOf(value)
    const declared = slots.find((item) => item.slot === slot)
    const primary = typeof held.ref === "string" && !isDefault(held.ref, declared?.default) ? [{ ref: held.ref, slot, lora: declared?.kind === "lora", strength: held.strength }] : []
    return [...primary, ...entries(held.lora).flatMap((adapter) => typeof adapter.ref === "string" ? [{ ref: adapter.ref, slot, lora: true, strength: adapter.strength }] : [])]
  })
  for (const held of entries(store.valueOf(name, "lora"))) if (typeof held.ref === "string") rows.push({ ref: held.ref, slot: tool?.model?.slot ?? "model", lora: true, strength: held.strength })
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
