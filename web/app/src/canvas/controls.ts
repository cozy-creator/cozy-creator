import type { Asset, Control, Tool } from "../types"
import { ratioOf } from "./shown"
import { type Row, rowsIn } from "./text"

/** Words with a name, which whatever mentions them reads: in the tool list, and no function of any package. */
export const TEXT_TOOL: Tool = { id: "text", package: "", function: "", release: "", name: "text", label: "Text", kind: "text", controls: [] }

/** A row's members, from what rows hold: what says the most, and each number, the first adding up and a new row as long as the first. What else a row holds stays, unseen. */
function membersOf(rows: Row[]): Control[] {
  const held = new Map(rows.flatMap((row) => Object.entries(row)))
  const said = (field: string) => Math.max(...rows.map((row) => String(row[field] ?? "").length))
  const [words = "description"] = [...held.keys()].filter((field) => typeof held.get(field) === "string").sort((a, b) => said(b) - said(a))
  const numbers = [...held.keys()].filter((field) => typeof held.get(field) === "number")
  return [
    { field: words, type: "text", label: fieldWords(words) },
    ...numbers.map((field, at): Control => ({ field, type: "range", label: fieldWords(field), min: 0, max: Math.max(30, ...rows.map((row) => Number(row[field]) || 0)), step: 1, total: at === 0, default: rows[0][field] })),
  ]
}

/** A box from whatever `fields` holds, for a tool the list doesn't know: what it holds is what can be edited, and text is always there to type. */
function fromFields(fields: Record<string, unknown>): Control[] {
  const held = Object.entries(fields).flatMap(([field, value]): Control[] => {
    if (field === "description" || field === "models" || field === "lora") return []
    if (rowsIn(value).length) return [{ field, type: "group", label: fieldWords(field), min_items: 1, members: membersOf(rowsIn(value)) }]
    if (field === "aspect_ratio") return [{ field, type: "shape", label: "Shape" }]
    if (typeof value === "boolean") return [{ field, type: "toggle", label: field }]
    if (typeof value === "number") return [{ field, type: "range", label: field, min: 0, max: Math.max(10, value * 2), step: Number.isInteger(value) ? 1 : 0.1 }]
    if (typeof value === "string") return [{ field, type: "text", label: field }]
    return []
  })
  if (!held.some((control) => control.type === "text" || control.type === "group")) return [{ field: "instructions", type: "text", label: "Instructions" }, ...held]
  // What says the most comes first, whatever the alphabet makes of the fields.
  const said = (control: Control) => (control.type === "text" ? String(fields[control.field]).length : 0)
  return [...held].sort((a, b) => said(b) - said(a))
}

/** A toggle's field that flips what makes the box, not a field of it. */
export const VARIANT = "variant"

/** The other function a box can be, of its package and label: the word on its toggle, whether it's on, and what it flips to. */
export function variantOf(tool: Tool | undefined, tools: Map<string, Tool>) {
  if (!tool) return null
  const all = [...tools.values()]
  const named = all.find((held) => held.package === tool.package && held.label === tool.label && held.id !== tool.id)
  // A function and its `_turbo` form are one box too, its toggle Turbo.
  const paired = named ? undefined : all.find((held) => held.package === tool.package && (held.function === `${tool.function}_turbo` || `${held.function}_turbo` === tool.function))
  const other = named ?? paired
  const word = tool.variant ?? other?.variant ?? (paired && "Turbo")
  return other && word ? { word, on: named ? Boolean(tool.variant) : tool.function.endsWith("_turbo"), other: other.id } : null
}

/** What a box asks for: its tool's controls, or what its fields hold. Nothing here knows any function. */
export function controlsOf(asset: Asset, tools: Map<string, Tool>): Control[] {
  const tool = asset.tool ? tools.get(asset.tool) : undefined
  if (!tool) return fromFields(asset.fields ?? {})
  const variant = variantOf(tool, tools)
  return variant ? [...tool.controls, { field: VARIANT, type: "toggle", label: variant.word, face: true }] : tool.controls
}

/** What a box shows unasked: its tool says, its first text, what a render can't go without and what its package shows; for a box from its fields, what it says, and with no text the file it takes. The rest waits. */
export function shownControls(controls: Control[]): Control[] {
  if (controls.some((control) => control.face)) return controls.filter((control) => control.face)
  const first = controls.find((control) => control.type === "text") ?? controls.find((control) => control.type === "file")
  return controls.filter((control) => control === first || control.type === "group" || control.required)
}

/** A new row: what its members default to. */
export const newRow = (control: Control): Row => Object.fromEntries((control.members ?? []).flatMap((member) => (member.default === undefined || member.default === "" ? [] : [[member.field, member.default]])))

/** What a group holds once it has as many rows as it must. */
export const fullRows = (control: Control, held: Row[]): Row[] => [...held, ...Array.from({ length: Math.max(0, (control.min_items ?? 0) - held.length) }, () => newRow(control))]

/** What a tool's box holds the moment it's dropped. */
export const startingFields = (tool: Tool) => Object.fromEntries(tool.controls.flatMap((control) => (control.type === "group" && control.min_items ? [[control.field, fullRows(control, [])]] : [])))

const empty = (value: unknown) => value === undefined || value === null || value === "" || (Array.isArray(value) && !value.length)

/** A field no control names, in words. */
export const fieldWords = (field: string) => field[0].toUpperCase() + field.slice(1).replaceAll("_", " ")

/** What it holds that no control draws, such as a hand-written seed: it reaches a render all the same. Its description is its own line, and a LoRA or a fine-tune is an add-on. */
export const heldBeyond = (fields: Record<string, unknown> | undefined, controls: Control[]) =>
  Object.entries(fields ?? {}).filter(([field, value]) => field !== "description" && field !== "lora" && field !== "models" && !empty(value) && !controls.some((control) => control.field === field))

/** The shapes a box snaps to when its tool doesn't say. */
const SHAPES = ["32:9", "2.39:1", "21:9", "2:1", "16:9", "5:3", "3:2", "4:3", "5:4", "1:1", "4:5", "3:4", "2:3", "3:5", "9:16", "1:2", "9:21"]

const parts = (shape: string) => shape.split(":").map(Number)

/** What a tool offers that's a shape: `source` isn't. */
const optionsOf = (control: Control) => (control.options ?? []).filter((option): option is string => ratioOf(option) !== undefined)

export const shapesOf = (control: Control): string[] => {
  const options = optionsOf(control)
  if (options.length) return options
  const fits = (shape: string) => {
    const [w, h] = parts(shape)
    return w / h >= (control.min ?? 0) && w / h <= (control.max ?? Infinity)
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

/** `ratio` as its tool can make it: the nearest of its options, else held to its bounds. */
export function heldTo(control: Control, ratio: number): number {
  const options = optionsOf(control)
  if (options.length) return ratioOf(snapped(ratio, options))!
  return Math.min(control.max ?? Infinity, Math.max(control.min ?? 0, ratio))
}
