import { useSignal } from "../ui/signal"
import { useCanvasStore, useElement } from "./context"
import { modelChoiceLabel } from "./ModelPicker"

type Entry = Record<string, unknown>
const entryOf = (value: unknown): Entry => value && typeof value === "object" && !Array.isArray(value) ? value as Entry : {}
const entries = (value: unknown): Entry[] => Array.isArray(value) ? value.map(entryOf) : []
const slotName = (slot: string) => slot === "rewriter" ? "Prompt-Enhancer I2I" : slot === "expander" ? "Prompt-Enhancer T2I" : slot.replaceAll("_", " ")

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
    const primary = typeof held.ref === "string" ? [{ ref: held.ref, slot, lora: slots.find((item) => item.slot === slot)?.kind === "lora", strength: held.strength }] : []
    return [...primary, ...entries(held.lora).flatMap((adapter) => typeof adapter.ref === "string" ? [{ ref: adapter.ref, slot, lora: true, strength: adapter.strength }] : [])]
  })
  for (const held of entries(store.valueOf(name, "lora"))) if (typeof held.ref === "string") rows.push({ ref: held.ref, slot: tool?.model?.slot ?? "model", lora: true, strength: held.strength })
  if (!rows.length) return null
  return <ul className="active-models" aria-label="Active models and LoRAs">{rows.map((row, index) => <li key={`${row.slot}:${row.ref}:${index}`}><span className="active-model-slot">{slotName(row.slot)} · {row.lora ? "LoRA" : "Checkpoint"}</span><span className="active-model-name" title={row.ref}>{modelChoiceLabel(row.ref)}</span>{row.lora && <Strength value={row.strength} />}</li>)}</ul>
}

function Strength({ value }: { value: unknown }) {
  // The renderer's default for an omitted override is 1.0.
  const strength = value == null ? 1 : typeof value === "number" && Number.isFinite(value) ? value : undefined
  return <span className="active-model-strength">{strength === undefined ? "Invalid weight" : `× ${strength}`}</span>
}
