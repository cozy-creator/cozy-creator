import type { Asset, Control, Tool } from "../types"
import { BEAT_S } from "./text"

/** A box from whatever `fields` holds, for a tool the list doesn't know: what it holds is what can be edited, and text is always there to type. */
function fromFields(fields: Record<string, unknown>): Control[] {
  const held = Object.entries(fields).flatMap(([field, value]): Control[] => {
    if (field === "description" || field === "models" || field === "lora") return []
    if (field === "beats" && Array.isArray(value)) return [{ field, type: "beats", label: "Beats" }]
    if (field === "aspect_ratio") return [{ field, type: "shape", label: "Shape" }]
    if (typeof value === "boolean") return [{ field, type: "toggle", label: field }]
    if (typeof value === "number") return [{ field, type: "range", label: field, min: 0, max: Math.max(10, value * 2), step: Number.isInteger(value) ? 1 : 0.1 }]
    if (typeof value === "string") return [{ field, type: "text", label: field }]
    return []
  })
  if (!held.some((control) => control.type === "text")) return [{ field: "instructions", type: "text", label: "Instructions" }, ...held]
  // What a box says comes first, whatever the alphabet makes of `style` and `summary`.
  const said = (control: Control) => (control.field === "instructions" || control.field === "summary" ? 0 : 1)
  return [...held].sort((a, b) => said(a) - said(b))
}

/** What a box asks for: its tool's controls, or what its fields hold. Nothing here knows any function. */
export function controlsOf(asset: Asset, tools: Map<string, Tool>): Control[] {
  const tool = asset.tool ? tools.get(asset.tool) : undefined
  return tool ? tool.controls : fromFields(asset.fields ?? {})
}

/** What a box shows unasked: its first text, its beats and what a render can't go without, and with no text the file it takes. The rest waits behind More. */
export function shownControls(controls: Control[]): Control[] {
  const first = controls.find((control) => control.type === "text") ?? controls.find((control) => control.type === "file")
  return controls.filter((control) => control === first || control.type === "beats" || control.required)
}

/** A new beat. A shot always has one, so one comes with its box. */
export const newBeat = (control: Control) => ({ duration: Math.min(BEAT_S, control.max_duration ?? BEAT_S) })

/** What a tool's box holds the moment it's dropped. */
export const startingFields = (tool: Tool) => Object.fromEntries(tool.controls.filter((control) => control.type === "beats").map((control) => [control.field, [newBeat(control)]]))

const empty = (value: unknown) => value === undefined || value === null || value === "" || (Array.isArray(value) && !value.length)

/** How a control behind More says it holds other than its default, or null. A shape needs no words: it's the box's own. */
export function markOf(control: Control, value: unknown): string | null {
  if (control.type === "shape" || empty(value) || JSON.stringify(value) === JSON.stringify(control.default)) return null
  if (control.type === "toggle") return `${control.label} ${value ? "on" : "off"}`
  return control.type === "range" || control.type === "choice" ? `${control.label} ${String(value)}` : control.label
}

/** A field no control names, in words. */
export const fieldWords = (field: string) => field[0].toUpperCase() + field.slice(1).replaceAll("_", " ")

/** What it holds that no control draws, such as a LoRA or a hand-written seed: it reaches a render all the same. Its description is its own line. */
export const heldBeyond = (fields: Record<string, unknown> | undefined, controls: Control[]) =>
  Object.entries(fields ?? {}).filter(([field, value]) => field !== "description" && !empty(value) && !controls.some((control) => control.field === field))

/** The shapes a box snaps to when its tool doesn't say. */
const SHAPES = ["21:9", "16:9", "3:2", "4:3", "1:1", "3:4", "2:3", "9:16"]

export const shapesOf = (control: Control): string[] => {
  const options = (control.options ?? []).filter((option): option is string => typeof option === "string")
  return options.length ? options : SHAPES
}

const parts = (shape: string) => shape.split(":").map(Number)

/** Of `shapes`, what's closest to `ratio`, as eyes judge it: by how many times over. */
export function snapped(ratio: number, shapes: string[]): string {
  const off = (shape: string) => {
    const [w, h] = parts(shape)
    return Math.abs(Math.log(ratio / (w / h)))
  }
  return shapes.reduce((best, shape) => (off(shape) < off(best) ? shape : best))
}
