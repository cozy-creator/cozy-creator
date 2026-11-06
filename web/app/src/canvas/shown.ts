import type { MainFile, Tool } from "../types"
import type { ElementView } from "./CanvasStore"

/** The file a card shows, and where it came from. */
export interface Shown extends MainFile {
  /** The version it is, when it's one. */
  number?: number
  /** The run that made it, for a file only its machine has. */
  run?: number
}

/**
 * What a card shows: the version being looked at, else its state, else, out of date or with nothing else, its newest. A constant's
 * state may be no version at all.
 */
export function shownOf(view: ElementView): Shown | null {
  const asset = view.asset
  if (!asset) return view.landed ? { output: view.landed.field, path: "", sha256: view.landed.sha256, field: view.landed.field } : null
  const versions = asset.prefixes ?? []
  const version = (number?: number) => versions.find((held) => held.number === number)
  const held = version(view.viewed) ?? version(asset.version)
  if (held?.main) return { ...held.main, number: held.number, run: held.run }
  if (asset.sha256 && asset.path && asset.field) {
    const { path, sha256, field, width, height, duration_ms, alpha, absent, machine } = asset
    return { output: field, path, sha256, field, width, height, duration_ms, alpha, absent, machine }
  }
  const newest = versions.at(-1)
  return newest?.main ? { ...newest.main, number: newest.number, run: newest.run } : null
}

/** A card's size, unmeasured, so a card culled away still counts in a fit. */
export const HEAD = 32
const ENTITY_DESC = 60
const ENTITY_SLOTS = 72
export const SUB_ROW = 136

/** What a tool makes, once it's run, as anything on the canvas is. */
export const makes = (view: ElementView | undefined) => Boolean(view?.asset?.tool) && (!madeHere(view?.asset?.tool) || Boolean(view?.asset?.timeline))

/** What Calcifer makes itself, free, with no machine. */
export const madeHere = (tool: string | undefined) => tool === "cozy" || Boolean(tool?.startsWith("cozy/"))

/** A box that makes something is what it's asked for; approved, it's that file, until its words are asked for. */
export const faceOf = (view: ElementView): "edit" | "media" => (makes(view) && (view.hereWords || !view.asset?.pinned) ? "edit" : "media")

export const widthFor = (view: ElementView | undefined): number => (view?.asset?.timeline ? 720 : view?.entity ? 240 : view?.asset?.kind === "video" ? 360 : makes(view) ? 300 : 240)

/** `16:9` as a number, if it's one. */
export function ratioOf(value: unknown): number | undefined {
  const parts = typeof value === "string" ? value.split(":").map(Number) : []
  return parts.length === 2 && parts[0] > 0 && parts[1] > 0 ? parts[0] / parts[1] : undefined
}

/** What its fields show: what's typed, else what was sent, else what it holds. */
export const fieldsOf = (view: ElementView | undefined): Record<string, unknown> => ({ ...view?.asset?.fields, ...view?.sent, ...view?.drafts })

/** What a field shows: what's typed, else what was sent, else what it holds. */
export function valueIn(view: ElementView | undefined, field: string): unknown {
  if (view?.drafts && field in view.drafts) return view.drafts[field]
  if (view?.sent && field in view.sent) return view.sent[field]
  return view?.asset?.fields?.[field]
}

/** A box an image tool makes: open, its takes have a + beside them, which writes the next one in its frame. */
export const paints = (view: ElementView, tools: Map<string, Tool>) => makes(view) && !view.asset?.timeline && tools.get(view.asset?.tool ?? "")?.kind === "image"

/** Showing no picture, or its words, a box is the shape it will make, which its corner changes; showing one, it's that picture's. */
export const reshapes = (view: ElementView) => makes(view) && (Boolean(view.hereWords) || !shownOf(view))

/** The shape it will make: the one set, else what its tool takes with none. */
const madeShape = (view: ElementView) => ratioOf(valueIn(view, "aspect_ratio")) ?? view.auto

/** The shape of the picture a mention of it sends: version `number`'s, else its state's, else, with none yet, the one it will make. */
export function sentShape(view: ElementView, number?: number): number | undefined {
  const asset = view.asset
  const main = asset?.prefixes?.find((held) => held.number === (number ?? asset.version))?.main
  if (main?.width && main.height) return main.width / main.height
  if (number !== undefined) return undefined
  if (asset?.width && asset.height) return asset.width / asset.height
  return makes(view) ? madeShape(view) : view.aspect
}

export function aspectOf(view: ElementView): number {
  const made = reshapes(view) ? (ratioOf(view.shape) ?? madeShape(view)) : undefined
  if (made) return made
  const shown = shownOf(view)
  const measured = shown?.width && shown.height ? shown.width / shown.height : undefined
  return view.aspect ?? measured ?? ((shown?.field ?? view.asset?.kind ?? view.placing?.media) === "audio" ? 3 : 16 / 9)
}

const hasSlots = (links?: Record<string, string>) => Object.keys(links ?? {}).length > 0

export function heightFor(view: ElementView, width: number): number {
  if (view.entity) {
    const subs = view.entity.subs ?? []
    return HEAD + ENTITY_DESC + (hasSlots(view.entity.links) ? ENTITY_SLOTS : 0) + subs.length * SUB_ROW
  }
  // An Edit on its controls: a picture over a filmstrip.
  if (view.asset?.timeline && faceOf(view) === "edit") return HEAD + 332
  return HEAD + Math.round(width / aspectOf(view))
}
