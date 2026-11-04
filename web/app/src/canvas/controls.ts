import type { Asset, Control, Tool } from "../types"

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
  return held.some((control) => control.type === "text") ? held : [{ field: "instructions", type: "text", label: "Instructions" }, ...held]
}

/** What a box asks for: its tool's controls, or what its fields hold. Nothing here knows any function. */
export function controlsOf(asset: Asset, tools: Map<string, Tool>): Control[] {
  const tool = asset.tool ? tools.get(asset.tool) : undefined
  return tool ? tool.controls : fromFields(asset.fields ?? {})
}

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
