import type { Asset, Tool } from "../api/calcifer"
import { addOnsOf, type Carried, modelsOf, takenOff, wordOf } from "./checkpoints"
import { OWN } from "./controls"
import { madeHere, toolName } from "./shown"

// What a tool going on, off or swapped does to a box, worked out here so the box shows it at once. Calcifer does the same to the file (`with_tool`). A tool coming
// off takes its fields and what they held with it (Paul, 11-06), and Undo puts them back; its pictures, takes, approval and description stay. This holds no state.

type Fields = Record<string, unknown>

/** One step and the way back from it. */
export interface ToolStep {
  /** What the box shows now. */
  row: Partial<Asset>
  /** What's sent: `tool`, `""` taking it off, and the fields that go with it. */
  tool: string
  fields: Fields
  /** The step that undoes this. */
  back: { row: Partial<Asset>; tool: string; fields: Fields }
  /** What it can't take, in words. */
  lost: string[]
}

const WORDS = ["instructions", "summary"]
const said = (value: unknown) => typeof value === "string" && value.trim() !== ""
const wordsIn = (fields: Fields) => WORDS.find((field) => said(fields[field]))
const nounOf = (kind: string) => (kind === "video" ? "take" : "version")

/** A box with no pictures and no tool is a text box. */
export const holdsPictures = (asset: Asset) => Boolean(asset.prefixes?.length || asset.sha256)

function rowBack(asset: Asset): Partial<Asset> {
  const { tool, kind, noun, standing, fields, prefixes, versions, version, approved, status } = asset
  return { tool, kind, noun, standing, fields, prefixes, versions, version, approved, status }
}

/** What `lora` and `models` keep on `next`, and what they lose: a LoRA is for a family, and a slot of `models` for a tool. */
function addOnsOn(fields: Fields, from: Tool | undefined, next: Tool): { kept: Fields; lost: string[] } {
  const runs = modelsOf(next)
  const same = modelsOf(from).some((held) => held.family && runs.some((model) => model.family === held.family))
  let [lora, models] = [fields.lora, fields.models]
  const lost: string[] = []
  for (const addOn of addOnsOf(lora, models)) {
    if (same && (!addOn.slot || runs.some((model) => model.slot === addOn.slot))) continue
    lost.push(`${addOn.lora ? "the LoRA" : "the fine-tune"} ${wordOf(addOn.ref)}`)
    const off = takenOff(addOn, lora, models)
    if ("lora" in off) lora = off.lora
    if ("models" in off) models = off.models
  }
  return { kept: { lora: lora ?? null, models: models ?? null }, lost }
}

/** Where a tool going on a picture or a video reads it, as Calcifer writes it (`with_tool`): its first required slot of that kind, else the start of its first prompt. */
function startsFrom(tool: Tool, kind: string, fields: Fields, own: string): Fields {
  const slot = tool.fields.find((field) => field.required && field.type === "slot" && field.accept === kind)
  if (slot) return fields[slot.field] === undefined ? { [slot.field]: own } : {}
  const prompt = tool.fields.find((field) => field.type === "prompt")
  if (!prompt) return {}
  const words = typeof fields[prompt.field] === "string" ? (fields[prompt.field] as string) : ""
  return words.includes(own) ? {} : { [prompt.field]: words ? `${own} ${words}` : own }
}

/** `tool` going on `asset`, whatever it holds now. `fields` is what it says, what's typed included, and `looked` the version being looked at. */
export function goingOn(asset: Asset, fields: Fields, from: Tool | undefined, tool: Tool, looked?: number): ToolStep {
  const back = { row: rowBack(asset), tool: asset.tool ?? "", fields: {} as Fields }
  const row: Partial<Asset> = { tool: tool.id, kind: holdsPictures(asset) ? asset.kind : tool.makes, noun: holdsPictures(asset) ? asset.noun : nounOf(tool.makes) }
  if (!asset.tool && !holdsPictures(asset)) {
    // Its words are the prompt.
    const { text, ...rest } = fields
    return { row: { ...row, standing: "missing", fields: { ...rest, instructions: text ?? "" } }, tool: tool.id, fields: {}, back, lost: [] }
  }
  // On pictures: its prompt starts from the version shown, and what's approved stays as it was (Paul, 11-06). A lone file is its version 1.
  if (!asset.tool) {
    const first = asset.prefixes?.length ? undefined : 1
    const number = first ?? looked ?? asset.version ?? asset.prefixes?.at(-1)?.number
    const main = first && asset.sha256 && asset.path && asset.field ? { output: asset.field, path: asset.path, sha256: asset.sha256, field: asset.field, width: asset.width, height: asset.height, duration_ms: asset.duration_ms, alpha: asset.alpha } : undefined
    const numbered: Partial<Asset> = main ? { prefixes: [{ number: 1, prefix: main.sha256.slice(0, 12), main }], versions: 1, version: 1, ...(asset.pinned ? { approved: 1 } : {}) } : {}
    const words = startsFrom(tool, asset.kind, fields, `<self.${number}>`)
    back.fields = Object.fromEntries(Object.keys(words).map((field) => [field, fields[field] ?? null]))
    return { row: { ...row, ...numbered, fields: { ...fields, ...words } }, tool: tool.id, fields: {}, back, lost: [] }
  }
  // A swap: what the new tool asks of keeps, and so do its words, in the field it reads them from.
  const asks = new Set(tool.fields.map((field) => field.field))
  const { kept: addOns, lost } = addOnsOn(fields, from, tool)
  const words = wordsIn(fields)
  const reads = tool.fields.find((field) => field.type === "prompt" || field.type === "words")?.field ?? "instructions"
  const send: Fields = {}
  for (const field of Object.keys(fields)) if (!asks.has(field) && !OWN.includes(field) && field !== "instructions") send[field] = null
  if (words && words !== reads) {
    send[words] = null
    send[reads] = fields[words]
  }
  if (JSON.stringify(addOns.lora ?? undefined) !== JSON.stringify(fields.lora)) send.lora = addOns.lora
  if (JSON.stringify(addOns.models ?? undefined) !== JSON.stringify(fields.models)) send.models = addOns.models
  const shown = Object.fromEntries(Object.entries({ ...fields, ...send }).filter(([, value]) => value !== null))
  back.fields = Object.fromEntries(Object.keys(send).map((field) => [field, fields[field] ?? null]))
  return { row: { ...row, fields: shown }, tool: tool.id, fields: send, back, lost }
}

/** Its tool coming off, and its fields with it: the box's own words stay, and a box of no pictures keeps its first words as its text. */
export function comingOff(asset: Asset, fields: Fields): ToolStep {
  const own = Object.fromEntries(Object.entries(fields).filter(([field]) => OWN.includes(field)))
  const gone = Object.keys(fields).filter((field) => !OWN.includes(field))
  const back = { row: rowBack(asset), tool: asset.tool ?? "", fields: Object.fromEntries(gone.map((field) => [field, fields[field]])) }
  if (holdsPictures(asset)) return { row: { tool: undefined, standing: "current", fields: own }, tool: "", fields: {}, back, lost: [] }
  const words = wordsIn(fields)
  return { row: { tool: undefined, kind: "text", noun: "version", standing: "current", fields: { ...own, text: words ? fields[words] : "" } }, tool: "", fields: {}, back: { ...back, fields: { ...back.fields, text: null } }, lost: [] }
}

/** A tool carried: dropped on a box it goes on, or takes the place of the one there, and the box opens (`putTool`). On open canvas it's a new box. */
export const carryingTool = (tool: Tool): Carried => ({
  canvas: true,
  whyNot(name, view) {
    if (!view?.asset || view.placing) return `${toolName(tool)} can't go on ${name}`
    if (madeHere(view.asset.tool)) return `${name} is made here, so ${toolName(tool)} can't go on it`
    return view.asset.tool === tool.id ? `${toolName(tool)} is on ${name} already` : null
  },
  put: (store, name) => store.putTool(name, tool),
})
