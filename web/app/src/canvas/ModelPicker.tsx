import { useQuery } from "@tanstack/react-query"
import { useDeferredValue, useEffect, useRef, useState } from "react"
import { createPortal } from "react-dom"
import type { FoundModel, ModelDetail, Tool, ToolModel } from "../api/calcifer"
import { useConnection } from "../app/Connection"
import { Icon } from "../ui/Icon"
import { NSFW, ShowNsfw, isNsfw, useShowNsfw } from "../ui/Tags"
import { useSignal } from "../ui/signal"
import { useCanvasStore, useElement } from "./context"
import { modelLabel } from "./palette"
import "./ModelPicker.css"

type Entry = Record<string, unknown>
type Kind = "weights" | "lora"
type PickerSlot = ToolModel & { kind?: "base" | "lora" | null }
const entryOf = (value: unknown): Entry => typeof value === "object" && value !== null && !Array.isArray(value) ? value as Entry : {}
const adaptersOf = (value: unknown): unknown[] => Array.isArray(value) ? value : []
const identityOf = (ref: string) => ref.split("@")[0]
const title = (value: string) => value.split(/[-_]+/).filter(Boolean).map((part) => part.toLowerCase() === "lora" ? "LoRA" : !/[aeiou]/i.test(part) ? part.toUpperCase() : part[0].toUpperCase() + part.slice(1)).join(" ")
const slotName = (slot: PickerSlot) => slot.slot === "rewriter" ? "Rewrite" : title(slot.slot)
const modelName = (ref: string) => title(identityOf(ref).split("/").filter(Boolean).at(-1)?.replace(/\.safetensors$/, "") ?? ref)
const slotsOf = (tool: Tool | undefined): PickerSlot[] => {
  const slots = (tool as (Tool & { models?: PickerSlot[] }) | undefined)?.models
  const primary = tool?.model as PickerSlot | undefined
  return slots?.length ? slots : primary ? [{ ...primary, kind: primary.kind ?? "base" }] : []
}
const compatible = (model: FoundModel | ModelDetail | undefined, family: string | null | undefined, kind: Kind | undefined) => Boolean(kind && family && model?.family === family && model?.kind === kind)

/** One modal edits every model slot without changing the node's tool or creating work. */
export function ModelPicker({ name, onClose }: { name: string; onClose(): void }) {
  const { bridge, privateViewEpoch, signedOut } = useConnection()
  const store = useCanvasStore()
  const view = useElement(name)
  const tools = useSignal(store.tools)
  const dialog = useRef<HTMLDivElement>(null)
  const close = useRef(onClose)
  close.current = onClose
  const [showNsfw, setShowNsfw] = useShowNsfw()
  const [page, setPage] = useState<string | null>(null)
  const tool = tools.get(view?.asset?.tool ?? "")
  const qwen = tool?.package.split("/").at(-1) === "qwen-image-2"
  const imageEdit = view?.operation === "edit_image" || store.mentionedPictures(name).length > 0
  const slots = slotsOf(tool).filter((slot) => !qwen || (slot.slot !== "expander" && slot.slot !== "rewriter") || slot.slot === (imageEdit ? "rewriter" : "expander"))
  const active = slots.find((slot) => slot.slot === page)
  const legacy = adaptersOf(store.valueOf(name, "lora"))
  const activeRef = active ? entryOf(entryOf(store.valueOf(name, "models"))[active.slot]).ref : undefined
  const activeModel = typeof activeRef === "string" ? activeRef : active?.default
  useEffect(() => {
    const project = store.projectId
    const epoch = project ? bridge.projectEpoch(project) : 0
    const unwatch = privateViewEpoch.subscribe(() => {
      if (project && bridge.projectEpoch(project) !== epoch) close.current()
    })
    const unsign = signedOut.subscribe(() => { if (signedOut.get()) close.current() })
    if (signedOut.get()) close.current()
    return () => { unwatch(); unsign() }
  }, [store, bridge, privateViewEpoch, signedOut])
  useEffect(() => {
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null
    const key = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault()
        event.stopPropagation()
        close.current()
      } else if (event.key === "Tab") {
        const controls = [...(dialog.current?.querySelectorAll<HTMLElement>("button, input, select, [tabindex='0']") ?? [])].filter((element) => !element.hasAttribute("disabled") && element.getClientRects().length)
        const first = controls[0]
        const last = controls.at(-1)
        if (!first) { event.preventDefault(); dialog.current?.focus() }
        else if (event.shiftKey && (document.activeElement === first || document.activeElement === dialog.current)) { event.preventDefault(); last?.focus() }
        else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus() }
      }
    }
    window.addEventListener("keydown", key, true)
    const first = dialog.current?.querySelector<HTMLInputElement>("input[type=search]") ?? dialog.current
    first?.focus()
    return () => {
      window.removeEventListener("keydown", key, true)
      if (previous?.isConnected) previous.focus()
    }
  }, [])
  useEffect(() => {
    const first = page ? dialog.current?.querySelector<HTMLInputElement>("input[type=search]") : dialog.current?.querySelector<HTMLButtonElement>("button[data-model-slot]")
    const focus = first ?? dialog.current
    focus?.focus()
  }, [page])
  useEffect(() => { if (page && page !== "__legacy" && !active) setPage(null) }, [page, active])
  useEffect(() => { if (!view?.asset) close.current() }, [view?.asset])
  if (!view?.asset) return null
  return createPortal(
    <div className="model-picker-overlay nodrag nopan nowheel" onPointerDown={(event) => { event.stopPropagation(); if (event.target === event.currentTarget) onClose() }} onClick={(event) => event.stopPropagation()}>
      <div ref={dialog} className="model-picker" role="dialog" aria-modal="true" aria-label="Models and LoRAs" tabIndex={-1} onKeyDown={(event) => event.stopPropagation()}>
        <header className="model-picker-head">
          {page && <button type="button" className="iconb model-picker-back" aria-label="Back to model slots" title="Back" onClick={() => setPage(null)}><Icon name="back" /></button>}
          <div className="model-picker-title"><h2>{active ? slotName(active) : page === "__legacy" ? "Existing LoRAs" : tool ? modelLabel(tool) : "Models and LoRAs"}</h2>{activeModel && <span>{modelName(activeModel)}</span>}</div>
          <button type="button" className="iconb" aria-label="Close model picker" title="Close" onClick={onClose}><Icon name="close" /></button>
        </header>
        {active && <div className="model-picker-preferences"><ShowNsfw shown={showNsfw} onChange={setShowNsfw} /></div>}
        <div className="model-picker-body">
          {!page && <>
            {!slots.length && <p className="muted">No model settings for this tool.</p>}
            <ul className="model-picker-slots">{slots.map((slot) => <li key={slot.slot}><button type="button" data-model-slot={slot.slot} onClick={() => setPage(slot.slot)}><span>{slotName(slot)}</span><Icon name="forward" /></button></li>)}
              {slots.length !== 1 && legacy.length > 0 && <li><button type="button" onClick={() => setPage("__legacy")}><span>Existing LoRAs</span><Icon name="forward" /></button></li>}
            </ul>
          </>}
          {active && <Slot key={active.slot} name={name} slot={active} showNsfw={showNsfw} legacy={slots.length === 1 ? legacy : []} />}
          {page === "__legacy" && <section className="model-picker-slot">
            <AdapterList name={name} slot={null} families={slots.flatMap((slot) => slot.family ? [slot.family] : [])} adapters={legacy} />
          </section>}
        </div>
      </div>
    </div>,
    document.body,
  )
}

function changeSlot(store: ReturnType<typeof useCanvasStore>, name: string, slot: string, change: (held: Entry) => Entry) {
  const models = { ...entryOf(store.valueOf(name, "models")) }
  const next = Object.fromEntries(Object.entries(change(entryOf(models[slot]))).filter(([, value]) => value !== undefined && value !== null && !(Array.isArray(value) && !value.length)))
  if (Object.keys(next).length) models[slot] = next
  else delete models[slot]
  store.putOn(name, { models: Object.keys(models).length ? models : null })
}

function Slot({ name, slot, showNsfw, legacy }: { name: string; slot: PickerSlot; showNsfw: boolean; legacy: unknown[] }) {
  const store = useCanvasStore()
  const { client } = useConnection()
  useElement(name)
  const held = entryOf(entryOf(store.valueOf(name, "models"))[slot.slot])
  const selected = typeof held.ref === "string" ? held.ref : ""
  const malformed = held.ref != null && typeof held.ref !== "string"
  const { data: defaultModel } = useQuery({
    queryKey: ["model", identityOf(slot.default ?? "")],
    queryFn: ({ signal }) => client.model(identityOf(slot.default ?? ""), signal),
    enabled: !slot.kind && Boolean(slot.default), staleTime: 60_000, retry: false,
  })
  const kind: Kind | undefined = slot.kind === "lora" ? "lora" : slot.kind === "base" ? "weights" : defaultModel?.kind === "weights" || defaultModel?.kind === "lora" ? defaultModel.kind : undefined
  const [typed, setTyped] = useState("")
  const q = useDeferredValue(typed.trim())
  const [limit, setLimit] = useState(50)
  const { data, error, isPending } = useQuery({
    queryKey: ["models", "picker", slot.family, q, limit, showNsfw],
    queryFn: ({ signal }) => client.models(q, limit, signal, { family: slot.family ?? undefined, noTag: showNsfw ? undefined : NSFW }),
    enabled: Boolean(slot.family), staleTime: 60_000,
  })
  const found = data?.models.filter((model) => model.family === slot.family && (showNsfw || !isNsfw(model.tags))) ?? []
  const listed = found.find((model) => model.model === identityOf(selected))
  const { data: detail, isPending: checking } = useQuery({
    queryKey: ["model", identityOf(selected)],
    queryFn: ({ signal }) => client.model(identityOf(selected), signal),
    enabled: Boolean(selected && !listed), staleTime: 60_000, retry: false,
  })
  const selectedValid = compatible(listed ?? detail, slot.family, kind)
  const selectionState = selectedValid ? "" : checking && !listed ? " (checking compatibility)" : !(listed ?? detail) ? " (compatibility unavailable)" : " (incompatible)"
  const alternatives = found.filter((model) => compatible(model, slot.family, kind) && model.model !== identityOf(selected))
  const adapters = adaptersOf(held.lora)
  const availableLoras = kind ? found.filter((model) => compatible(model, slot.family, "lora") && !adapters.some((adapter) => identityOf(String(entryOf(adapter).ref ?? "")) === model.model)) : []
  const clearBase = () => changeSlot(store, name, slot.slot, ({ ref: _, ...rest }) => rest)
  return <section className="model-picker-slot" data-model-slot={slot.slot}>
    <div className="model-picker-model-row">
      <label className="model-picker-field">
        <span>{kind === "lora" ? "LoRA" : "Model"}</span>
        <select className="field" aria-label={`Base model for ${slot.slot}`} disabled={!kind} value={malformed ? "__invalid" : selected} onChange={(event) => event.target.value ? changeSlot(store, name, slot.slot, (held) => ({ ...held, ref: event.target.value })) : clearBase()}>
          <option value="">{slot.default ? `Default: ${modelName(slot.default)}` : "No model selected"}</option>
          {malformed && <option value="__invalid" disabled>Invalid model selection</option>}
          {selected && <option value={selected} disabled={!selectedValid}>{modelName(selected)}{selectionState}</option>}
          {alternatives.map((model) => <option key={model.model} value={model.model}>{modelName(model.model)}</option>)}
        </select>
      </label>
      {(selected || malformed) && <button type="button" className="tb model-picker-reset" title={slot.default ? "Use tool default" : "Remove base model"} onClick={clearBase}>{slot.default ? "Reset" : "Remove"}</button>}
    </div>
    <input type="search" className="field model-picker-search" aria-label={`Search models for ${slot.slot}`} placeholder="Find a model or LoRA" value={typed} disabled={!slot.family} onChange={(event) => { setTyped(event.target.value); setLimit(50) }} />
    {!slot.family && <p className="muted">Compatible choices aren't available for this slot.</p>}
    {slot.family && !kind && <p className="muted">Model compatibility is not available yet.</p>}
    {Boolean(slot.family) && isPending && <p className="muted">Loading choices…</p>}
    {error && <p className="warn">Couldn't load models.</p>}
    <div className="model-picker-lora-head">
      <h4>LoRAs</h4>
      <select className="field" aria-label={`Add LoRA for ${slot.slot}`} value="" disabled={!availableLoras.length} onChange={(event) => {
        const ref = event.target.value
        if (ref) changeSlot(store, name, slot.slot, (held) => adaptersOf(held.lora).some((value) => identityOf(String(entryOf(value).ref ?? "")) === identityOf(ref)) ? held : ({ ...held, lora: [...adaptersOf(held.lora), { ref, strength: 1 }] }))
      }}>
        <option value="">{!kind ? "Compatibility unavailable" : availableLoras.length ? "Add a LoRA…" : "No compatible LoRAs"}</option>
        {availableLoras.map((model) => <option key={model.model} value={model.model}>{modelName(model.model)}</option>)}
      </select>
    </div>
    <AdapterList name={name} slot={slot.slot} families={slot.family ? [slot.family] : []} adapters={adapters} />
    {legacy.length > 0 && <div className="model-picker-legacy"><h4>Existing LoRAs</h4><AdapterList name={name} slot={null} families={slot.family ? [slot.family] : []} adapters={legacy} /></div>}
    {Boolean(data?.older) && (limit < 100 ? <button type="button" className="tb" onClick={() => setLimit(Math.min(100, limit + 25))}>Show more choices</button> : <p className="muted">Refine the search to see more choices.</p>)}
  </section>
}

function AdapterList({ name, slot, families, adapters }: { name: string; slot: string | null; families: string[]; adapters: unknown[] }) {
  const store = useCanvasStore()
  const change = (index: number, strength?: number) => {
    const edit = (items: unknown[]) => strength === undefined ? items.filter((_, at) => at !== index) : items.map((held, at) => at === index ? { ...entryOf(held), strength } : held)
    if (slot === null) {
      const next = edit(adaptersOf(store.valueOf(name, "lora")))
      store.putOn(name, { lora: next.length ? next : null })
    } else changeSlot(store, name, slot, (held) => ({ ...held, lora: edit(adaptersOf(held.lora)) }))
  }
  if (!adapters.length) return <p className="muted model-picker-empty">None added</p>
  return <ul className="model-picker-adapters">{adapters.map((value, index) => <Adapter key={`${index}:${String(entryOf(value).ref ?? "")}`} value={value} families={families} onRemove={() => change(index)} onStrength={(value) => change(index, value)} />)}</ul>
}

function Adapter({ value, families, onRemove, onStrength }: { value: unknown; families: string[]; onRemove(): void; onStrength(value: number): void }) {
  const { client } = useConnection()
  const held = entryOf(value)
  const ref = typeof held.ref === "string" ? held.ref : ""
  const { data, isPending } = useQuery({
    queryKey: ["model", identityOf(ref)], queryFn: ({ signal }) => client.model(identityOf(ref), signal),
    enabled: Boolean(ref), staleTime: 60_000, retry: false,
  })
  const valid = Boolean(data?.kind === "lora" && data.family && families.includes(data.family))
  const strength = held.strength == null ? 1 : typeof held.strength === "number" && Number.isFinite(held.strength) ? held.strength : null
  const [draft, setDraft] = useState<string | null>(null)
  const commit = () => {
    if (draft !== null && draft.trim() && Number.isFinite(Number(draft))) onStrength(Number(draft))
    setDraft(null)
  }
  const word = ref ? modelName(ref) : "Unknown adapter"
  return <li className={`model-picker-adapter ${valid ? "" : "invalid"}`}>
    <span className="model-picker-adapter-name">{word}{!valid && <small>{ref && isPending ? "Checking compatibility…" : !data ? "Compatibility unavailable" : "Incompatible with this model slot"}</small>}{strength === null && <small>Enter a valid strength.</small>}</span>
    <label className="model-picker-strength"><span>Strength</span><input className="field" type="number" step="0.05" aria-label={`Strength of ${word}`} disabled={!valid} value={draft ?? (strength === null ? "" : String(strength))} onChange={(event) => setDraft(event.target.value)} onBlur={commit} onKeyDown={(event) => { if (event.key === "Enter") { event.preventDefault(); commit() } }} /></label>
    <button type="button" className="iconb" aria-label={`Remove ${word}`} title="Remove LoRA" onClick={onRemove}><Icon name="close" /></button>
  </li>
}
