import type { FoundModel, Tool } from "../types"
import { say } from "../ui/notes"
import { signal } from "../ui/signal"
import { isNsfw } from "../ui/Tags"
import type { CanvasStore, ElementView } from "./CanvasStore"
import { madeHere } from "./shown"

/** What a box can be given by a drag: whether this one takes it, and what taking it does. A checkpoint is one; anything else goes by this same path. */
export interface Carried {
  /** On open canvas: true, it's for whatever let it go (a box of its own); words, why it can't go there. Neither, it isn't dropped for nothing. */
  canvas?: true | string
  /** Why this box can't take it, in plain words, or null when it can. */
  whyNot(name: string, view: ElementView | undefined, tools: Map<string, Tool>): string | null
  put(store: CanvasStore, name: string): void
}

/** What's in the air: a drag's data can't be read before its drop. */
export const carried = signal<Carried | null>(null)

/** A LoRA or a fine-tune on a box, as its file says it. */
export interface AddOn {
  ref: string
  lora: boolean
  /** The model it's under in `models`. None: it's in `lora`, and goes on the box's model. */
  slot?: string
  strength: number
}

/** What a checkpoint can go on in what a tool runs. One for now. */
export const modelsOf = (tool: Tool | undefined) => (tool?.model ? [tool.model] : [])

type Entry = Record<string, unknown>
type Fields = Record<string, unknown>

const isEntry = (value: unknown): value is Entry => typeof value === "object" && value !== null && !Array.isArray(value)
const slotsOf = (models: unknown): Record<string, Entry> => (isEntry(models) ? (Object.fromEntries(Object.entries(models).filter(([, held]) => isEntry(held))) as Record<string, Entry>) : {})
const refOf = (entry: unknown) => (isEntry(entry) && typeof entry.ref === "string" ? entry.ref : null)

const lorasIn = (list: unknown, slot?: string): AddOn[] =>
  (Array.isArray(list) ? list : []).flatMap((entry) => {
    const ref = refOf(entry)
    return ref ? [{ ref, lora: true, slot, strength: Number(entry.strength ?? 1) }] : []
  })

/** What a box's `lora` and `models` hold, its fine-tune first. */
export function addOnsOf(lora: unknown, models: unknown): AddOn[] {
  const slots = Object.entries(slotsOf(models))
  return [
    ...slots.flatMap(([slot, held]): AddOn[] => {
      const ref = refOf(held)
      return ref ? [{ ref, lora: false, slot, strength: 1 }] : []
    }),
    ...slots.flatMap(([slot, held]) => lorasIn(held.lora, slot)),
    ...lorasIn(lora),
  ]
}

/** The fields with `addOn` changed by `to`, or with no answer taken off. What else they say stays as it's written. */
function changed(addOn: AddOn, lora: unknown, models: unknown, to: (entry: Entry) => Entry | null): Fields {
  const inList = (list: unknown) => {
    const kept = (Array.isArray(list) ? list : []).flatMap((entry) => (refOf(entry) === addOn.ref ? (to(entry) ?? []) : [entry]))
    return kept.length ? kept : null
  }
  if (!addOn.slot) return { lora: inList(lora) }
  const { [addOn.slot]: held = {}, ...others } = slotsOf(models)
  const { ref: _, lora: loras, ...rest } = held
  const next = { ...rest, ...(addOn.lora ? { ...(_ === undefined ? {} : { ref: _ }), lora: inList(loras) } : loras === undefined ? {} : { lora: loras }) }
  const left = Object.fromEntries(Object.entries(next).filter(([, value]) => value !== null))
  const slots = Object.keys(left).length ? { ...others, [addOn.slot]: left } : others
  return { models: Object.keys(slots).length ? slots : null }
}

export const takenOff = (addOn: AddOn, lora: unknown, models: unknown) => changed(addOn, lora, models, () => null)

export const atStrength = (addOn: AddOn, strength: number, lora: unknown, models: unknown) => changed(addOn, lora, models, (entry) => ({ ...entry, strength }))

/** A checkpoint's own word: `hf://a/b` and `a/b@1.0` are `b`. */
export const wordOf = (ref: string) => (ref.split("://").pop() ?? ref).split("@")[0].split("/").filter(Boolean).pop()?.replace(/\.safetensors$/, "") ?? ref

/** Why `found` can't go on this box, in plain words, or null when it can. Only its header and the box's tool say, so this names no model. */
export function whyNot(found: FoundModel, name: string, view: ElementView | undefined, tools: Map<string, Tool>): string | null {
  const word = wordOf(found.model)
  const made = view?.asset?.tool
  if (!made || madeHere(made)) return `No model makes ${name}, so ${word} has nothing to go on`
  const tool = tools.get(made)
  if (!tool) return `Can't tell which model ${name} runs, so ${word} stays off it`
  const models = modelsOf(tool)
  if (!models.length) return `${name} runs no one model, so ${word} has nothing to go on`
  if (!found.kind || !found.family) return `Can't tell what ${word} is for, so it stays off ${name}`
  const families = models.flatMap((model) => model.family ?? [])
  if (!families.length) return `Can't tell which model ${name} runs, so ${word} stays off it`
  return families.includes(found.family) ? null : `${word} is for ${found.family}, and ${name} runs ${families.join(", ")}`
}

/** The box under a point, its add-ons being part of it. */
export function boxAt(store: CanvasStore, x: number, y: number): string | null {
  for (const element of document.elementsFromPoint(x, y)) {
    const card = element.closest(".react-flow__node")?.querySelector<HTMLElement>(".card[data-asset]")
    if (card) return store.resolveName(card.dataset.asset ?? "")
  }
  return null
}

/** Carried over a point: what would take it says so. */
export function carriedOver(store: CanvasStore, x: number, y: number) {
  const held = carried.get()
  if (!held) return
  const name = boxAt(store, x, y)
  store.setHot(name && !held.whyNot(name, store.get(name), store.tools.get()) ? name : null)
}

/** Whatever's carried, let go at a point: on a box that takes it, it goes on; on one that doesn't, that says why and nothing changes. */
export function letGo(store: CanvasStore, x: number, y: number) {
  const held = carried.get()
  if (!held) return false
  store.setHot(null)
  const name = boxAt(store, x, y)
  if (typeof held.canvas === "string" && !name) say(held.canvas)
  if (!name) return held.canvas !== true
  const why = held.whyNot(name, store.get(name), store.tools.get())
  if (why) say(why)
  else held.put(store, name)
  return true
}

/** What the lists of checkpoints show: no component (a text encoder, a VAE: weights its header verifies as no family's model), and what's
 *  tagged NSFW only while the viewer's Show NSFW switch is on. */
export const listed = (found: FoundModel, nsfwShown: boolean) => found.kind !== "component" && (nsfwShown || !isNsfw(found.tags))

/** What a model family is called: what the tool of it on your list says, else its own words, `sdxl` being SDXL and `krea-2` Krea 2. */
export function familyName(family: string, tools: Map<string, Tool>): string {
  const labels = [...tools.values()].flatMap((tool) => (tool.model?.family === family ? [tool.label] : []))
  if (labels.length) return labels.reduce((short, label) => (label.length < short.length ? label : short))
  return family
    .split(/[-_]+/)
    .map((word) => (/[aeiou]/i.test(word) ? word[0].toUpperCase() + word.slice(1) : word.toUpperCase()))
    .join(" ")
}

/** The tool on your list that runs a checkpoint of its own: the one it's the default of, else the first of its family. A LoRA runs on a model, never alone. */
export function runnerOf(found: FoundModel, tools: Map<string, Tool>): Tool | undefined {
  if (found.kind !== "weights" || !found.family) return undefined
  const runs = [...tools.values()].filter((tool) => tool.model?.family === found.family)
  return runs.find((tool) => tool.model?.default?.split("@")[0] === found.model) ?? runs[0]
}

/** A checkpoint carried: a LoRA joins those on a box, and a fine-tune takes the place of any there. A checkpoint runs on its own, so on open canvas it's
 *  a new box of the tool that runs it; a LoRA there goes nowhere. */
export const carrying = (found: FoundModel, tools: Map<string, Tool>): Carried => ({
  canvas: found.kind !== "weights" ? undefined : runnerOf(found, tools) ? true : `No tool runs ${found.family ? familyName(found.family, tools) : wordOf(found.model)} yet`,
  whyNot: (name, view, tools) => whyNot(found, name, view, tools),
  put(store, name) {
    const lora = store.valueOf(name, "lora")
    const models = store.valueOf(name, "models")
    if (addOnsOf(lora, models).some((addOn) => addOn.ref === found.model)) return void say(`${wordOf(found.model)} is already on ${name}`)
    const slot = modelsOf(store.tools.get().get(store.get(name)?.asset?.tool ?? "")).find((model) => model.family === found.family)?.slot
    if (found.kind === "lora") store.putOn(name, { lora: [...(Array.isArray(lora) ? lora : []), { ref: found.model, strength: 1 }] })
    else if (slot) store.putOn(name, { models: { ...slotsOf(models), [slot]: { ...slotsOf(models)[slot], ref: found.model } } })
  },
})
