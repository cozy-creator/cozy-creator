import { useQuery } from "@tanstack/react-query"
import { useDeferredValue, useEffect, useRef, useState } from "react"
import { createPortal } from "react-dom"
import type { FoundModel, ModelDetail, Tool, ToolModel } from "../api/calcifer"
import { useConnection } from "../app/Connection"
import { Icon } from "../ui/Icon"
import { say } from "../ui/notes"
import { NSFW, isNsfw, useShowNsfw } from "../ui/Tags"
import { useSignal } from "../ui/signal"
import { useCanvasStore, useElement } from "./context"
import { loraNote } from "./loraNotes"
import "./ModelPicker.css"

type Entry = Record<string, unknown>
type Kind = "weights" | "lora"
/** One choice the picker offers: `slot` is its key in a box's `models`, `param` the parameter it fills. */
export type PickerSlot = ToolModel & { param: string }
type PickerModel = FoundModel & { strength?: number | null; description?: string | null }
const entryOf = (value: unknown): Entry => typeof value === "object" && value !== null && !Array.isArray(value) ? value as Entry : {}
const adaptersOf = (value: unknown): unknown[] => Array.isArray(value) ? value : []
const identityOf = (ref: string) => ref.split("@")[0]
const title = (value: string) => value.split(/[-_]+/).filter(Boolean).map((part) => part.toLowerCase() === "lora" ? "LoRA" : !/[aeiou]/i.test(part) ? part.toUpperCase() : part[0].toUpperCase() + part.slice(1)).join(" ")
const slotName = (slot: PickerSlot, qwen: boolean, slots: PickerSlot[]) => qwen && slot.slot === "rewriter" ? "Prompt-Enhancer I2I" : qwen && slot.slot === "expander" ? "Prompt-Enhancer T2I"
  : slots.filter((held) => held.param === slot.param).length > 1 ? `${title(slot.param)} (${title(slot.slot.split(".models.")[0])})` : title(slot.param)
const modelName = (ref: string) => title(identityOf(ref).split("/").filter(Boolean).at(-1)?.replace(/\.safetensors$/, "") ?? ref)
export const modelChoiceLabel = (ref: string) => {
  const parts = identityOf(ref).split("/").filter(Boolean)
  const owner = parts.length > 1 ? parts.at(-2) : undefined
  return owner ? `${owner} / ${modelName(ref)}` : modelName(ref)
}
const descriptionOf = (model: PickerModel) => (model.description ?? loraNote(model.model)?.description)?.trim().replace(/\s+/g, " ") || "No description provided."
/** What a tool's models and LoRAs are set by, the one list every model control reads: each slot it declares, and its child calls'
 *  slots of one parameter and one default as one (H3's segments each take `base_model`: one Base Model). A parameter only one of them
 *  fills is written by its name, which the binder takes to each call's slot of it (choices.rs `slots`); else by its path. */
export function slotsOf(tool: Tool | undefined): PickerSlot[] {
  const declared = tool?.models?.length ? tool.models : tool?.model ? [{ ...tool.model, kind: tool.model.kind ?? "base" }] : []
  const merged = new Map<string, PickerSlot>()
  for (const slot of declared) {
    const param = slot.slot.split(".models.").at(-1) ?? slot.slot
    if (!merged.has(`${param} ${slot.default ?? ""}`)) merged.set(`${param} ${slot.default ?? ""}`, { ...slot, param })
  }
  const slots = [...merged.values()]
  return slots.map((slot) => slots.filter((other) => other.param === slot.param).length === 1 ? { ...slot, slot: slot.param } : slot)
}
const compatible = (model: FoundModel | ModelDetail | undefined, family: string | null | undefined, kind: Kind | undefined) => Boolean(kind && family && model?.family === family && model?.kind === kind)
const adapterFits = (model: FoundModel | ModelDetail, slot: PickerSlot) => {
  if (!compatible(model, slot.family, "lora") || identityOf(model.model) === "alibaba-pai/qwen-image-2.1-fun-acc") return false
  const encoder = model.tags?.some((tag) => tag.toLowerCase().replace(/_/g, "-") === "text-encoder") ?? false
  return slot.slot === "text_encoder" ? encoder : !encoder
}
const fitsSlot = (model: FoundModel | ModelDetail | undefined, ref: string, slot: PickerSlot, kind: Kind | undefined, funAcc: boolean) => {
  if (funAcc && slot.slot === "model" && identityOf(ref) === "qwen/qwen-image-2.1-turbo") return false
  if (ref === slot.default) return true
  if (!compatible(model, slot.family, kind)) return false
  const tags = model?.tags?.map((tag) => tag.toLowerCase().replace(/_/g, "-")) ?? []
  if (slot.slot === "text_encoder") return tags.includes("text-encoder")
  return !tags.includes("text-encoder") && !tags.includes("vae")
}

/** One modal edits every model slot without changing the node's tool or creating work. */
export function ModelPicker({ name, onClose }: { name: string; onClose(): void }) {
  const { bridge, privateViewEpoch, signedOut } = useConnection()
  const store = useCanvasStore()
  const view = useElement(name)
  const tools = useSignal(store.tools)
  const dialog = useRef<HTMLDivElement>(null)
  const close = useRef(onClose)
  close.current = onClose
  const [showNsfw] = useShowNsfw()
  const [page, setPage] = useState<string | null>(null)
  const tool = tools.get(view?.asset?.tool ?? "")
  const qwen = tool?.package.split("/").at(-1) === "qwen-image-2"
  const funAcc = Boolean(qwen && slotsOf(tool).some((slot) => slot.slot === "turbo_lora" && (slot.class?.split(".").at(-1) === "FunAccLoRA" || identityOf(slot.default ?? "") === "alibaba-pai/qwen-image-2.1-fun-acc")))
  const imageEdit = view?.operation === "edit_image" || store.mentionedPictures(name).length > 0
  const helper = imageEdit ? "rewriter" : "expander"
  const slots = slotsOf(tool).filter((slot) => !qwen || (slot.slot !== "expander" && slot.slot !== "rewriter") || slot.slot === helper)
  if (qwen) {
    const order = [helper, "model", "text_encoder", "turbo_lora"]
    const at = (slot: PickerSlot) => order.includes(slot.slot) ? order.indexOf(slot.slot) : order.length
    slots.sort((a, b) => at(a) - at(b))
  }
  const active = slots.find((slot) => slot.slot === page)
  const restricted = Boolean(qwen && active?.slot === "turbo_lora")
  const legacy = adaptersOf(store.valueOf(name, "lora"))
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
          <div className="model-picker-title"><h2>{active ? slotName(active, qwen, slots) : page === "__legacy" ? "Existing LoRAs" : "Models & LoRAs"}</h2></div>
          <button type="button" className="iconb" aria-label="Close model picker" title="Close" onClick={onClose}><Icon name="close" /></button>
        </header>
        <div className="model-picker-body">
          {!page && <>
            {!slots.length && <p className="muted">No model settings for this tool.</p>}
            <ul className="model-picker-slots">{slots.map((slot) => <li key={slot.slot}><button type="button" data-model-slot={slot.slot} onClick={() => setPage(slot.slot)}><span>{slotName(slot, qwen, slots)}</span><Icon name="forward" /></button></li>)}
              {slots.length !== 1 && legacy.length > 0 && <li><button type="button" onClick={() => setPage("__legacy")}><span>Existing LoRAs</span><Icon name="forward" /></button></li>}
            </ul>
          </>}
          {active && <Slot key={active.slot} name={name} slot={active} restricted={restricted} funAcc={funAcc} showNsfw={showNsfw} legacy={slots.length === 1 ? legacy : []} />}
          {page === "__legacy" && <section className="model-picker-slot">
            <AdapterList name={name} slot={null} families={slots.flatMap((slot) => slot.family ? [slot.family] : [])} adapters={legacy} />
          </section>}
        </div>
      </div>
    </div>,
    document.body,
  )
}

function changeSlot(store: ReturnType<typeof useCanvasStore>, name: string, slot: PickerSlot, change: (held: Entry) => Entry) {
  const models = { ...entryOf(store.valueOf(name, "models")) }
  const next = Object.fromEntries(Object.entries(change(entryOf(models[slot.slot]))).filter(([, value]) => value !== undefined && value !== null && !(Array.isArray(value) && !value.length)))
  if (Object.keys(next).length) models[slot.slot] = next
  else delete models[slot.slot]
  store.putOn(name, { models: Object.keys(models).length ? models : null })
}

function Slot({ name, slot, restricted, funAcc, showNsfw, legacy }: { name: string; slot: PickerSlot; restricted: boolean; funAcc: boolean; showNsfw: boolean; legacy: unknown[] }) {
  const store = useCanvasStore()
  const { client } = useConnection()
  const view = useElement(name)
  const held = entryOf(entryOf(store.valueOf(name, "models"))[slot.slot])
  const selected = typeof held.ref === "string" ? held.ref : ""
  const malformed = held.ref != null && typeof held.ref !== "string"
  const { data: defaultModel } = useQuery({
    queryKey: ["model", identityOf(slot.default ?? "")],
    queryFn: ({ signal }) => client.model(identityOf(slot.default ?? ""), signal),
    enabled: Boolean(slot.default) && (!slot.kind || !showNsfw), staleTime: 60_000, retry: false,
  })
  const kind: Kind | undefined = slot.kind === "lora" ? "lora" : slot.kind === "base" ? "weights" : defaultModel?.kind === "weights" || defaultModel?.kind === "lora" ? defaultModel.kind : undefined
  const [typed, setTyped] = useState("")
  const q = useDeferredValue(typed.trim())
  const [limit, setLimit] = useState(50)
  const [tab, setTab] = useState<"models" | "loras">("models")
  // A model slot offers what it loads, as Calcifer reads their headers against its default's (cl-943); a LoRA slot, LoRAs of its family.
  const fits = view?.asset?.tool ? `${view.asset.tool}:${slot.slot}` : undefined
  const byHeader = Boolean(slot.default && fits) && slot.kind !== "lora"
  const noTag = showNsfw ? undefined : NSFW
  const project = store.projectId ?? undefined
  const { data: loading, error: loadError, isPending: loadPending } = useQuery({
    queryKey: ["models", "fits", fits, project, q, limit, showNsfw],
    queryFn: ({ signal }) => client.models(q, limit, signal, { fits, project, noTag }),
    enabled: byHeader && !restricted && tab === "models", staleTime: 60_000,
  })
  const { data: kin, error: kinError, isPending: kinPending } = useQuery({
    queryKey: ["models", "picker", slot.family, q, limit, showNsfw],
    queryFn: ({ signal }) => client.models(q, limit, signal, { family: slot.family ?? undefined, noTag }),
    enabled: Boolean(slot.family) && !restricted && (!byHeader || tab === "loras"), staleTime: 60_000,
  })
  const shown = (model: PickerModel) => showNsfw || !isNsfw(model.tags)
  const found: PickerModel[] = kin?.models.filter((model) => model.family === slot.family && shown(model)) ?? []
  const loads: PickerModel[] = loading?.models.filter(shown) ?? []
  const { data: fitting, isPending: fitChecking } = useQuery({
    queryKey: ["models", "fits", fits, project, identityOf(selected)],
    queryFn: ({ signal }) => client.models(identityOf(selected), 1, signal, { fits, project }),
    enabled: byHeader && Boolean(selected) && !restricted && !loads.some((model) => model.model === identityOf(selected)), staleTime: 60_000, retry: false,
  })
  const listed = byHeader ? loads.find((model) => model.model === identityOf(selected)) ?? fitting?.models.find((model) => model.model === identityOf(selected)) : found.find((model) => model.model === identityOf(selected))
  const { data: detail, isPending: checking } = useQuery({
    queryKey: ["model", identityOf(selected)],
    queryFn: ({ signal }) => client.model(identityOf(selected), signal),
    enabled: Boolean(selected && !listed && (!restricted || !showNsfw)), staleTime: 60_000, retry: false,
  })
  const turboBlocked = (ref: string) => funAcc && slot.slot === "model" && identityOf(ref) === "qwen/qwen-image-2.1-turbo"
  const selectedValid = restricted ? selected === slot.default
    : byHeader ? identityOf(selected) === identityOf(slot.default ?? "") || (Boolean(listed) && !turboBlocked(selected))
    : fitsSlot(listed ?? detail, selected, slot, kind, funAcc)
  const pending = byHeader ? fitChecking : checking
  const selectionState = selectedValid ? "" : restricted ? " (compatibility unverified)" : pending && !listed ? " (checking compatibility)" : !byHeader && !(listed ?? detail) ? " (compatibility unavailable)" : " (incompatible)"
  const alternatives = restricted ? [] : byHeader ? loads.filter((model) => !turboBlocked(model.model) && model.model !== identityOf(selected))
    : found.filter((model) => fitsSlot(model, model.model, slot, kind, funAcc) && model.model !== identityOf(selected))
  const searchable = byHeader || Boolean(slot.family)
  const offering = byHeader && tab === "models"
  const data = offering ? loading : kin
  const error = offering ? loadError : kinError
  const isPending = offering ? loadPending : kinPending
  const adapters = adaptersOf(held.lora)
  const defaultVisible = !slot.default || showNsfw || Boolean(defaultModel && !isNsfw(defaultModel.tags))
  const selectedMeta = listed ?? detail ?? (identityOf(selected) === identityOf(slot.default ?? "") ? defaultModel : undefined)
  const selectedVisible = showNsfw || Boolean(selectedMeta && !isNsfw(selectedMeta.tags))
  const availableLoras = kind ? found.filter((model) => adapterFits(model, slot) && !adapters.some((adapter) => identityOf(String(entryOf(adapter).ref ?? "")) === model.model)) : []
  const clearBase = () => changeSlot(store, name, slot, ({ ref: _, ...rest }) => {
    if (restricted) delete rest.lora
    return rest
  })
  const addLora = async (model: PickerModel) => {
    let recommended = model.strength
    if (recommended === undefined) {
      try { recommended = (await client.model(model.model)).strength }
      catch (error) { say("Couldn't read the LoRA's default strength", error); return }
    }
    const strength = recommended ?? loraNote(model.model)?.strength
    const adapter = { ref: model.model, ...(typeof strength === "number" && Number.isFinite(strength) ? { strength } : {}) }
    changeSlot(store, name, slot, (held) => adaptersOf(held.lora).some((value) => identityOf(String(entryOf(value).ref ?? "")) === identityOf(model.model)) ? held : ({ ...held, lora: [...adaptersOf(held.lora), adapter] }))
  }
  return <section className="model-picker-slot" data-model-slot={slot.slot}>
    {!restricted && <div className="model-picker-tabs" role="group" aria-label={`Choices for ${slot.slot}`}>
      <button type="button" aria-pressed={tab === "models"} onClick={() => { setTab("models"); setTyped(""); setLimit(50) }}>Models</button>
      <button type="button" aria-pressed={tab === "loras"} onClick={() => { setTab("loras"); setTyped(""); setLimit(50) }}>LoRAs</button>
    </div>}
    {!restricted && <input type="search" className="field model-picker-search" aria-label={`Search models for ${slot.slot}`} placeholder={tab === "models" ? "Find a model" : "Find a LoRA"} value={typed} disabled={!searchable} onChange={(event) => { setTyped(event.target.value); setLimit(50) }} />}
    {!restricted && !searchable && <p className="muted">Compatible choices aren't available for this slot.</p>}
    {!restricted && !byHeader && slot.family && !kind && <p className="muted">Model compatibility is not available yet.</p>}
    {!restricted && (tab === "models" ? byHeader : Boolean(slot.family)) && isPending && <p className="muted">Loading choices…</p>}
    {!restricted && error && <p className="warn">Couldn't load models.</p>}
    {tab === "models" || restricted ? <>
      <ul className="model-picker-choices" aria-label={`Base model for ${slot.slot}`}>
        <li><button type="button" aria-pressed={!selected && !malformed} disabled={!defaultVisible} onClick={clearBase}>
          <span>{defaultVisible ? slot.default ? modelChoiceLabel(slot.default) : "No model selected" : defaultModel ? "Default model hidden" : "Default model"}<small>Default</small></span><span className="model-picker-choice-mark" aria-hidden="true" />
        </button></li>
        {malformed && <li><button type="button" aria-pressed="true" disabled><span>Invalid model selection</span><span className="model-picker-choice-mark" aria-hidden="true" /></button></li>}
        {selected && <li><button type="button" aria-pressed="true" disabled={!selectedValid || !selectedVisible} onClick={() => changeSlot(store, name, slot, (held) => ({ ...held, ref: selected }))}>
          <span>{selectedVisible ? modelChoiceLabel(selected) : selectedMeta ? "Selected model hidden" : "Selected model"}<small>{selectedValid ? "Selected" : selectionState.trim().replace(/^\(|\)$/g, "")}</small></span><span className="model-picker-choice-mark" aria-hidden="true" />
        </button></li>}
        {alternatives.filter((model) => model.model !== identityOf(slot.default ?? "")).map((model) => <li key={model.model}><button type="button" aria-pressed="false" onClick={() => changeSlot(store, name, slot, (held) => ({ ...held, ref: model.model }))}>
          <span>{modelChoiceLabel(model.model)}</span><span className="model-picker-choice-mark" aria-hidden="true" />
        </button></li>)}
      </ul>
      {(selected || malformed) && <button type="button" className="tb model-picker-reset" title={slot.default ? "Use tool default" : "Remove base model"} onClick={clearBase}>{slot.default ? "Reset to default" : "Remove model"}</button>}
      {restricted && <p className="muted">Only the default Turbo LoRA is verified.</p>}
      {restricted && adapters.length > 0 && <button type="button" className="tb model-picker-reset" onClick={clearBase}>Reset Turbo LoRA</button>}
    </> : <>
      {adapters.length > 0 && <><h4>Added LoRAs</h4><AdapterList name={name} slot={slot} families={slot.family ? [slot.family] : []} adapters={adapters} /></>}
      {legacy.length > 0 && <div className="model-picker-legacy"><h4>Existing LoRAs</h4><AdapterList name={name} slot={null} families={slot.family ? [slot.family] : []} adapters={legacy} /></div>}
      <h4>Available LoRAs</h4>
      <ul className="model-picker-choices" aria-label={`Add LoRA for ${slot.slot}`}>
        {availableLoras.map((model) => <li key={model.model}><button type="button" className="model-picker-lora-option" onClick={() => void addLora(model)}><span><span>{modelChoiceLabel(model.model)}</span><small className="model-picker-lora-description" title={descriptionOf(model)}>{descriptionOf(model)}</small></span></button></li>)}
      </ul>
      {!availableLoras.length && !isPending && <p className="muted model-picker-empty">No compatible LoRAs found.</p>}
    </>}
    {!restricted && Boolean(data?.older) && (limit < 100 ? <button type="button" className="tb" onClick={() => setLimit(Math.min(100, limit + 25))}>Show more choices</button> : <p className="muted">Refine the search to see more choices.</p>)}
  </section>
}

function AdapterList({ name, slot, families, adapters }: { name: string; slot: PickerSlot | null; families: string[]; adapters: unknown[] }) {
  const store = useCanvasStore()
  const change = (index: number, strength?: number) => {
    const edit = (items: unknown[]) => strength === undefined ? items.filter((_, at) => at !== index) : items.map((held, at) => at === index ? { ...entryOf(held), strength } : held)
    if (slot === null) {
      const next = edit(adaptersOf(store.valueOf(name, "lora")))
      store.putOn(name, { lora: next.length ? next : null })
    } else changeSlot(store, name, slot, (held) => ({ ...held, lora: edit(adaptersOf(held.lora)) }))
  }
  if (!adapters.length) return <p className="muted model-picker-empty">None added</p>
  return <ul className="model-picker-adapters">{adapters.map((value, index) => <Adapter key={`${index}:${String(entryOf(value).ref ?? "")}`} value={value} families={families} slot={slot ?? undefined} onRemove={() => change(index)} onStrength={(value) => change(index, value)} />)}</ul>
}

function Adapter({ value, families, slot, onRemove, onStrength }: { value: unknown; families: string[]; slot?: PickerSlot; onRemove(): void; onStrength(value: number): void }) {
  const { client } = useConnection()
  const [showNsfw] = useShowNsfw()
  const held = entryOf(value)
  const ref = typeof held.ref === "string" ? held.ref : ""
  const { data, isPending } = useQuery({
    queryKey: ["model", identityOf(ref)], queryFn: ({ signal }) => client.model(identityOf(ref), signal),
    enabled: Boolean(ref), staleTime: 60_000, retry: false,
  })
  const valid = Boolean(data?.kind === "lora" && data.family && families.includes(data.family) && identityOf(ref) !== "alibaba-pai/qwen-image-2.1-fun-acc" && (!slot || adapterFits(data, slot)))
  const strength = held.strength == null ? 1 : typeof held.strength === "number" && Number.isFinite(held.strength) ? held.strength : null
  const [draft, setDraft] = useState<string | null>(null)
  const commit = () => {
    if (draft !== null && draft.trim() && Number.isFinite(Number(draft))) onStrength(Number(draft))
    setDraft(null)
  }
  const word = ref ? modelChoiceLabel(ref) : "Unknown adapter"
  if (!showNsfw && (!data || isNsfw(data.tags))) return null
  return <li className={`model-picker-adapter ${valid ? "" : "invalid"}`}>
    <span className="model-picker-adapter-name">{word}{!valid && <small>{ref && isPending ? "Checking compatibility…" : !data ? "Compatibility unavailable" : "Incompatible with this model slot"}</small>}{strength === null && <small>Enter a valid strength.</small>}</span>
    <label className="model-picker-strength"><span>Strength</span><input className="field" type="number" step="0.05" aria-label={`Strength of ${word}`} disabled={!valid} value={draft ?? (strength === null ? "" : String(strength))} onChange={(event) => setDraft(event.target.value)} onBlur={commit} onKeyDown={(event) => { if (event.key === "Enter") { event.preventDefault(); commit() } }} /></label>
    <button type="button" className="iconb" aria-label={`Remove ${word}`} title="Remove LoRA" onClick={onRemove}><Icon name="close" /></button>
  </li>
}
