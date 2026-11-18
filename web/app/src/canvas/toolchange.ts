import type { Asset, Tool, ToolField } from "../api/calcifer"
import { addOnsOf, type Carried, modelsOf, takenOff, wordOf } from "./checkpoints"
import { fromFields, mainText, OWN, startingFields, TEXT_TOOL } from "./controls"
import { madeHere, toolName } from "./shown"
import { toolFitsNode } from "./palette"

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

const said = (value: unknown) => typeof value === "string" && value.trim() !== ""
/** Where a box of `tool` holds its words, when it holds any: its main text (`mainText`), or with no tool the list knows, what its fields hold
 *  says (`fieldsOf`). */
function wordsIn(fields: Fields, tool: Tool | undefined): string | undefined {
  const field = mainText(tool ? tool.fields : fromFields(fields))?.field
  return field !== undefined && said(fields[field]) ? field : undefined
}
/** Its words moved from where a box of `from` holds them to where `tool` reads them: the fields that change. */
function wordsTo(fields: Fields, from: Tool | undefined, tool: Tool): Fields {
  const held = wordsIn(fields, from)
  const reads = mainText(tool.fields)?.field ?? "instructions"
  return held && held !== reads ? { [held]: null, [reads]: fields[held] } : {}
}
const nounOf = (kind: string) => (kind === "video" ? "take" : "version")

/** Whether the container already holds a file or a version. A box of words is its words, whatever versions a tool wrote on it (xs-204). */
export const holdsPictures = (asset: Asset) => asset.kind !== "text" && Boolean(asset.prefixes?.length || asset.sha256)

function rowBack(asset: Asset): Partial<Asset> {
  const { tool, kind, noun, standing, fields, prefixes, versions, version, approved, status, timeline } = asset
  return { tool, kind, noun, standing, fields, prefixes, versions, version, approved, status, timeline }
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

/** Where a box of `tool` reads a box of `kind`, as Calcifer's `reads_kind`: its first required slot of that kind, else its first prompt. `able`:
 *  only a prompt that takes that kind, and any prompt takes words. */
export function readsFrom(tool: Tool, kind: string, able = false): ToolField | undefined {
  const slot = tool.fields.find((field) => field.required && field.type === "slot" && field.accept === kind)
  return slot ?? tool.fields.find((field) => field.type === "prompt" && (!able || kind === "text" || !field.takes || field.takes.includes(kind)))
}

/** Where a tool going on a picture or a video reads it, as Calcifer writes it (`with_tool`): its first required slot of that kind, else the start of its first
 *  prompt. `able`: a new box made from it, which reads it only where it can (`readsBox`). */
export function startsFrom(tool: Tool, kind: string, fields: Fields, own: string, able = false): Fields {
  const field = readsFrom(tool, kind, able)
  if (field?.type === "slot") return fields[field.field] === undefined ? { [field.field]: own } : {}
  if (!field) return {}
  const words = typeof fields[field.field] === "string" ? (fields[field.field] as string) : ""
  return words.includes(own) ? {} : { [field.field]: words ? `${own} ${words}` : own }
}

/** Whether `tool`, let go on a box of another kind, makes a new box of its own kind beside it that reads it (Calcifer's `reads`, xs-204): an LLM on a
 *  picture, an image-to-video tool on an image. Words another tool wrote aren't words a mention reads yet. */
export const readsBox = (tool: Tool, asset: Asset): boolean =>
  tool.id !== TEXT_TOOL.id && !madeHere(tool.id) && tool.makes !== asset.kind && !(asset.kind === "text" && asset.tool) && Boolean(readsFrom(tool, asset.kind, true))

/** `tool` going on `asset`, whatever it holds now. `fields` is what it says, what's typed included, and `looked` the version being looked at. */
export function goingOn(asset: Asset, fields: Fields, from: Tool | undefined, tool: Tool, looked?: number): ToolStep {
  const initial = Object.fromEntries(Object.entries(startingFields(tool)).filter(([field]) => fields[field] == null))
  const back = { row: rowBack(asset), tool: asset.tool ?? "", fields: { ...Object.fromEntries(Object.keys(initial).map((field) => [field, fields[field] ?? null])), node_kind: fields.node_kind ?? null } as Fields }
  const row: Partial<Asset> = { tool: tool.id, kind: holdsPictures(asset) ? asset.kind : tool.makes, noun: holdsPictures(asset) ? asset.noun : nounOf(tool.makes), timeline: tool.id === "cozy/edit" ? { fps: 24, tracks: [{ clips: [] }] } : undefined }
  if (!asset.tool && !holdsPictures(asset)) {
    // Its words are the prompt.
    const words = wordsTo(fields, undefined, tool)
    back.fields = { ...back.fields, ...Object.fromEntries(Object.keys(words).map((field) => [field, fields[field] ?? null])) }
    const shown = Object.fromEntries(Object.entries({ ...fields, ...words, ...initial, node_kind: tool.makes }).filter(([, value]) => value !== null))
    return { row: { ...row, standing: "missing", fields: shown }, tool: tool.id, fields: { ...initial, ...words, node_kind: tool.makes }, back, lost: [] }
  }
  // On pictures: its prompt starts from the version shown, and what's approved stays as it was (Paul, 11-06). A lone file is its version 1.
  if (!asset.tool) {
    const first = asset.prefixes?.length ? undefined : 1
    const number = first ?? looked ?? asset.version ?? asset.prefixes?.at(-1)?.number
    const main = first && asset.sha256 && asset.path && asset.field ? { output: asset.field, path: asset.path, sha256: asset.sha256, field: asset.field, width: asset.width, height: asset.height, duration_ms: asset.duration_ms, alpha: asset.alpha } : undefined
    const numbered: Partial<Asset> = main ? { prefixes: [{ number: 1, prefix: main.sha256.slice(0, 12), main }], versions: 1, version: 1, ...(asset.pinned ? { approved: 1 } : {}) } : {}
    const words = startsFrom(tool, asset.kind, fields, `<self.${number}>`)
    back.fields = { ...back.fields, ...Object.fromEntries(Object.keys(words).map((field) => [field, fields[field] ?? null])) }
    return { row: { ...row, ...numbered, fields: { ...fields, ...initial, ...words, node_kind: tool.makes } }, tool: tool.id, fields: { ...initial, node_kind: tool.makes }, back, lost: [] }
  }
  // A swap: what the new tool asks of keeps, and so do its words, in the field it reads them from.
  const asks = new Set(tool.fields.map((field) => field.field))
  const { kept: addOns, lost } = addOnsOn(fields, from, tool)
  const send: Fields = { ...initial, node_kind: tool.makes }
  for (const field of Object.keys(fields)) if (!asks.has(field) && !OWN.includes(field) && field !== "instructions") send[field] = null
  Object.assign(send, wordsTo(fields, from, tool))
  if (JSON.stringify(addOns.lora ?? undefined) !== JSON.stringify(fields.lora)) send.lora = addOns.lora
  if (JSON.stringify(addOns.models ?? undefined) !== JSON.stringify(fields.models)) send.models = addOns.models
  const shown = Object.fromEntries(Object.entries({ ...fields, ...send }).filter(([, value]) => value !== null))
  back.fields = Object.fromEntries(Object.keys(send).map((field) => [field, fields[field] ?? null]))
  return { row: { ...row, fields: shown }, tool: tool.id, fields: send, back, lost }
}

/** Its tool coming off leaves the box's kind and its words: where `from`, that tool, held them, or on a box of words as its text. */
export function comingOff(asset: Asset, fields: Fields, from: Tool | undefined): ToolStep {
  const words = wordsIn(fields, from)
  // Named in what's sent, they stay as the rest of what the tool asked goes.
  const kept: Fields = words && asset.kind !== "text" ? { [words]: fields[words] } : {}
  const own = { ...Object.fromEntries(Object.entries(fields).filter(([field]) => OWN.includes(field))), ...kept, node_kind: asset.kind }
  const gone = Object.keys(fields).filter((field) => !OWN.includes(field))
  const back = { row: rowBack(asset), tool: asset.tool ?? "", fields: { ...Object.fromEntries(gone.map((field) => [field, fields[field]])), node_kind: fields.node_kind ?? null } }
  if (holdsPictures(asset)) return { row: { tool: undefined, standing: "current", fields: own, timeline: undefined }, tool: "", fields: { ...kept, node_kind: asset.kind }, back, lost: [] }
  if (asset.kind !== "text") return { row: { tool: undefined, kind: asset.kind, noun: asset.noun, standing: "missing", fields: own, timeline: undefined }, tool: "", fields: { ...kept, node_kind: asset.kind }, back, lost: [] }
  const text: Fields = words ? { [words]: null, text: fields[words] } : {}
  return { row: { tool: undefined, kind: "text", noun: "version", standing: "current", fields: { ...own, text: words ? fields[words] : "" } }, tool: "", fields: text, back: { ...back, fields: { ...back.fields, text: null } }, lost: [] }
}

/** What a tool can't use, in words. */
const KIND_WORDS: Record<string, string> = { image: "images", video: "videos", audio: "audio", text: "text" }

/** Why `tool` can't go on `asset`, or null when it can: a tool goes on any box of the kind it makes (tools §9). One that can't read the picture
 *  goes on all the same, and what it can't use is struck through, saying why (Paul, 11-16: "you can attach an image tool to any image"). On a box
 *  of another kind it reads, it makes a new box from it (`readsBox`); only where it can't read it at all does nothing change. */
export function wontGoOn(tool: Tool, asset: Asset, name: string): string | null {
  if (tool.id === TEXT_TOOL.id) return "Text is a box of its own: let it go on open canvas"
  if (toolFitsNode(tool, asset) || readsBox(tool, asset)) return null
  if (tool.makes === asset.kind) return `${toolName(tool)} can't go on ${name}`
  if (madeHere(tool.id)) return `${toolName(tool)} makes ${KIND_WORDS[tool.makes] ?? tool.makes}. Drop it on ${/^[aeiou]/.test(tool.makes) ? "an" : "a"} ${tool.makes} box or on open canvas`
  if (asset.kind === "text" && asset.tool && readsFrom(tool, "text", true)) return `${toolName(tool)} can't read what ${name}'s tool writes yet`
  return `${toolName(tool)} can't use ${KIND_WORDS[asset.kind] ?? asset.kind}, so it can't go on ${name}`
}

/** A tool carried: dropped on a box it goes on, or takes the place of the one there, and the box opens (`putTool`); on a box of another kind it reads,
 *  a new box beside it reads it. On open canvas it's a new box. Anywhere else nothing changes, and a note says why. */
export const carryingTool = (tool: Tool): Carried => ({
  canvas: true,
  whyNot(name, view) {
    if (!view?.asset || view.placing) return `${toolName(tool)} can't go on ${name}`
    return wontGoOn(tool, view.asset, name)
  },
  put: (store, name) => store.putTool(name, tool),
})
