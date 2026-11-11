import type { Asset, ToolField, Tool } from "../api/calcifer"
import type { Media } from "../types"
import { ratioOf } from "./shown"
import { type Row, rowsIn } from "./text"

/** Words with a name, which whatever mentions them reads: in the tool list, and no function of any package. */
export const TEXT_TOOL: Tool = { id: "text", package: "cozy", package_title: "Cozy", function: "", release: "", name: "text", label: "Text", makes: "text", fields: [] }
/** The existing local timeline engine; it is a basic node, never a model attachment. */
export const VIDEO_EDIT_TOOL: Tool = { id: "cozy/edit", package: "cozy", package_title: "Cozy", function: "edit", release: "", name: "Edit", label: "Video Editor", makes: "video", fields: [{ field: "timeline", type: "timeline", label: "Edit" }] }

/** A row's members, from what rows hold: what says the most, and each number, the first adding up. What else a row holds stays, unseen. */
function membersOf(rows: Row[]): ToolField[] {
  const held = new Map(rows.flatMap((row) => Object.entries(row)))
  const said = (field: string) => Math.max(...rows.map((row) => String(row[field] ?? "").length))
  const [words = "description"] = [...held.keys()].filter((field) => typeof held.get(field) === "string").sort((a, b) => said(b) - said(a))
  const numbers = [...held.keys()].filter((field) => typeof held.get(field) === "number")
  return [
    { field: words, type: "prompt", label: fieldWords(words) },
    ...numbers.map((field, at): ToolField => ({ field, type: "number", label: fieldWords(field), min: 0, max: Math.max(30, ...rows.map((row) => Number(row[field]) || 0)), step: 1, total: at === 0, default: rows[0][field] })),
  ]
}

/** A box from whatever its fields hold, for a tool the list doesn't know: what it holds is what can be edited, and text is always there to type. */
function fromFields(fields: Record<string, unknown>): ToolField[] {
  const held = Object.entries(fields).flatMap(([field, value]): ToolField[] => {
    if (OWN.includes(field)) return []
    if (rowsIn(value).length) return [{ field, type: "group", list: true, label: fieldWords(field), min_items: 1, members: membersOf(rowsIn(value)) }]
    if (field === "aspect_ratio") return [{ field, type: "shape", label: "Shape" }]
    if (typeof value === "boolean") return [{ field, type: "toggle", label: fieldWords(field) }]
    if (typeof value === "number") return [{ field, type: "number", label: fieldWords(field), min: 0, max: Math.max(10, value * 2), step: Number.isInteger(value) ? 1 : 0.1 }]
    if (typeof value === "string") return [{ field, type: "words", label: fieldWords(field) }]
    return []
  })
  // What says the most is its prompt, whatever the alphabet makes of the fields; other words are only words.
  const said = (field: ToolField) => (field.type === "words" ? String(fields[field.field]).length : -1)
  const [first] = [...held].sort((a, b) => said(b) - said(a))
  if (first?.type === "words") return [{ ...first, type: "prompt" }, ...held.filter((field) => field !== first)]
  if (!held.some((field) => field.type === "group")) return [{ field: "instructions", type: "prompt", label: "Instructions" }, ...held]
  return held
}

/** The platform's words on any box, whatever made it: no tool asks for them. */
export const OWN = ["description", "models", "lora", "node_kind"]

/** What a box asks for: its tool's fields, or what its fields hold. Nothing here knows any function. */
export function fieldsOf(asset: Asset, tools: Map<string, Tool>): ToolField[] {
  const tool = asset.tool ? tools.get(asset.tool) : undefined
  return tool ? tool.fields : fromFields(asset.fields ?? {})
}

export const empty = (value: unknown) => value === undefined || value === null || value === "" || (Array.isArray(value) && !value.length)

/** Unset, or set to just what it would be unset. */
export const atDefault = (field: ToolField, value: unknown) => empty(value) || JSON.stringify(value) === JSON.stringify(field.default)

/** On the face: prompts, groups, what's required, and a filled slot. Everything else is under More. */
export const onFace = (field: ToolField, value: unknown) => field.type === "prompt" || field.type === "group" || field.type === "timeline" || Boolean(field.required) || (field.type === "slot" && !empty(value))

/** A row of a list group as it starts: its members' defaults, and as many rows as each of its own lists must have. */
export function newRow(group: ToolField): Row {
  return Object.fromEntries(
    (group.members ?? []).flatMap((member): [string, unknown][] => {
      if (member.type === "group" && member.list && member.min_items) return [[member.field, fullRows(member, [])]]
      if (member.type === "group" && !member.list && member.required) return [[member.field, newRow(member)]]
      return member.default === undefined || member.default === null || member.default === "" ? [] : [[member.field, member.default]]
    }),
  )
}

/** What a list group holds once it has as many rows as it must. */
export const fullRows = (group: ToolField, held: Row[]): Row[] => [...held, ...Array.from({ length: Math.max(0, (group.min_items ?? 0) - held.length) }, () => newRow(group))]

/** What a tool's box holds the moment it's dropped: as many rows as each list must have. */
export const startingFields = (tool: Tool) => Object.fromEntries(tool.fields.flatMap((field) => (field.type === "group" && field.list && field.min_items ? [[field.field, fullRows(field, [])]] : [])))

/** A choice's option in its own words. */
export const optionWord = (field: ToolField, option: unknown) => field.words?.[String(option)] ?? String(option)

/** A field no tool names, in words. */
export const fieldWords = (field: string) => field[0].toUpperCase() + field.slice(1).replaceAll("_", " ")

/** What it holds that no field declares, such as a hand-written key: it reaches a render all the same. Its description is its own line, and a LoRA or a fine-tune is an add-on. */
export const heldBeyond = (fields: Record<string, unknown> | undefined, declared: ToolField[]) =>
  Object.entries(fields ?? {}).filter(([field, value]) => !OWN.includes(field) && !empty(value) && !declared.some((held) => held.field === field))

/** What a slot or a prompt's mention can hold, in words. */
export const WANTED: Record<Media, string> = { image: "a picture", video: "a video", audio: "a sound" }

/** The sum a list group shows, of its member marked `total`. */
export function totalOf(group: ToolField, rows: Row[]): { member: ToolField; sum: number } | null {
  const member = group.members?.find((held) => held.total)
  if (!member) return null
  return { member, sum: Math.round(rows.reduce((all, row) => all + (Number(row[member.field] ?? member.default) || 0), 0) * 100) / 100 }
}

/** The shapes a box snaps to when its tool doesn't say. */
const SHAPES = ["32:9", "2.39:1", "21:9", "2:1", "16:9", "5:3", "3:2", "4:3", "5:4", "1:1", "4:5", "3:4", "2:3", "3:5", "9:16", "1:2", "9:21"]

const parts = (shape: string) => shape.split(":").map(Number)

/** The ratios a shape allows that are shapes. */
const ratiosOf = (field: ToolField) => ((field.ratios ?? []) as string[]).filter((ratio) => ratioOf(ratio) !== undefined)

export const shapesOf = (field: ToolField): string[] => {
  const ratios = ratiosOf(field)
  if (ratios.length) return ratios
  const fits = (shape: string) => {
    const [w, h] = parts(shape)
    return w / h >= (field.min ?? 0) && w / h <= (field.max ?? Infinity)
  }
  return SHAPES.filter(fits)
}

/** Of `shapes`, what's closest to `ratio`, as eyes judge it: by how many times over. */
export function snapped(ratio: number, shapes: string[]): string {
  const off = (shape: string) => {
    const [w, h] = parts(shape)
    return Math.abs(Math.log(ratio / (w / h)))
  }
  return shapes.reduce((best, shape) => (off(shape) < off(best) ? shape : best))
}

/** `ratio` as its tool can make it: the nearest of its ratios, else held to its bounds. */
export function heldTo(field: ToolField, ratio: number): number {
  const ratios = ratiosOf(field)
  if (ratios.length) return ratioOf(snapped(ratio, ratios))!
  return Math.min(field.max ?? Infinity, Math.max(field.min ?? 0, ratio))
}
