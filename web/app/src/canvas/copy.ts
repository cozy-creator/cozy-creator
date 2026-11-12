import type { Client } from "../api/client"
import type { Asset } from "../api/calcifer"
import type { CanvasStore, CarriedFile } from "./CanvasStore"
import { fieldsOf } from "./controls"
import type { Box } from "./graph"
import { shownOf, valuesOf } from "./shown"
import { lineOf } from "./text"

/** What makes a box, which a paste where it isn't makes again, with no takes. */
export type Made = Pick<Asset, "kind" | "tool" | "fields" | "timeline"> & { name: string }

/**
 * A box copied: the take a box being worked on shows, which a paste makes a box of its own; else the box, by its name in this project (`from`), and what
 * makes it. `at` is where it stood, which a paste keeps among the others.
 */
export type CopiedBox = { at: Box } & ({ file: CarriedFile } | { made: Made; from?: string })

/** What Ctrl+C took: the system's clipboard holds only a picture or words, so this is how a paste knows them for its own. */
let copied: { project: string | null; boxes: CopiedBox[]; words: string } | null = null

/** What `name` is as a copy, where it stands. Being worked on and showing a take, it's that take, unless `whole`. */
export function copyOf(store: CanvasStore, name: string, whole = false): CopiedBox | null {
  const view = store.get(name)
  const asset = view?.asset
  const at = store.poseOf(name)
  if (!view || !asset || !at) return null
  const shown = shownOf(view)
  if (!whole && shown && !shown.absent && view.here && !view.hereWords) {
    const held = store.valueOf(name, "description")
    const description = typeof held === "string" ? held : undefined
    const called = shown.number === undefined || !asset.prefixes?.length || !asset.tool ? asset.name : `${asset.name}_${shown.number}`
    return { at, file: { sha256: shown.sha256, field: shown.field, name: called, description, from: asset.name } }
  }
  const made = { kind: asset.kind, tool: asset.tool, fields: { ...asset.fields, ...view.sent, ...view.drafts }, timeline: asset.timeline, name: asset.name }
  return { at, made, from: asset.name }
}

/** The middle of where they stood. */
export function middleOf(boxes: CopiedBox[]): { x: number; y: number } {
  const left = Math.min(...boxes.map(({ at }) => at.x))
  const top = Math.min(...boxes.map(({ at }) => at.y))
  const right = Math.max(...boxes.map(({ at }) => at.x + at.w))
  const bottom = Math.max(...boxes.map(({ at }) => at.y + at.h))
  return { x: (left + right) / 2, y: (top + bottom) / 2 }
}

/** A picture as the system's clipboard takes it, which is PNG. */
async function png(client: Client, path: string): Promise<Blob> {
  const blob = await (await client.transport.fetch(path)).blob()
  if (blob.type === "image/png") return blob
  const bitmap = await createImageBitmap(blob)
  const canvas = new OffscreenCanvas(bitmap.width, bitmap.height)
  canvas.getContext("2d")?.drawImage(bitmap, 0, 0)
  return canvas.convertToBlob({ type: "image/png" })
}

/** `names` copied. The system's clipboard gets words, which any browser takes, and a lone picture where this one lets it. Whether there was anything to copy. */
export function copyBoxes(store: CanvasStore, client: Client, names: string[], data: DataTransfer): boolean {
  const boxes = names.flatMap((name) => copyOf(store, name) ?? [])
  if (!boxes.length) return false
  const said = (name: string) => {
    const asset = store.get(name)?.asset
    return (asset && lineOf(valuesOf(store.get(name)), fieldsOf(asset, store.tools.get()))) || name
  }
  const words = names.map(said).join("\n\n")
  copied = { project: store.projectId, boxes, words }
  data.setData("text/plain", words)
  const view = names.length === 1 ? store.get(names[0]) : undefined
  const shown = view && shownOf(view)
  if (shown?.field === "image" && !shown.absent && shown.path && store.projectId && navigator.clipboard?.write) {
    // Words go with it: they're how a paste here knows this picture from a screenshot.
    const item = new ClipboardItem({ "image/png": png(client, client.file(store.projectId, shown.path)), "text/plain": new Blob([words], { type: "text/plain" }) })
    navigator.clipboard.write([item]).catch(() => {})
  }
  return true
}

/**
 * What a paste of `data` brings that this page copied. In another project its boxes and files aren't there to be had: what makes a box is made again,
 * and a picture goes by the clipboard like any other.
 */
export function ownCopy(store: CanvasStore, data: DataTransfer): CopiedBox[] | null {
  if (!copied || data.getData("text/plain") !== copied.words) return null
  if (copied.project === store.projectId) return copied.boxes
  const made = copied.boxes.flatMap((box) => ("made" in box && (box.made.tool || box.made.kind === "text") ? [{ at: box.at, made: box.made }] : []))
  return made.length === copied.boxes.length ? made : null
}
