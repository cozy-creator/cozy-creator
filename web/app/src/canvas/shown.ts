import type { MainFile } from "../types"
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

/** A box that makes something opens on its controls; approved, it's that file, and either way can flip. */
export const faceOf = (view: ElementView): "edit" | "media" => (makes(view) ? (view.face ?? (view.asset?.pinned ? "media" : "edit")) : "media")

export const widthFor = (view: ElementView | undefined): number => (view?.asset?.timeline ? 720 : view?.entity ? 240 : view?.asset?.kind === "video" ? 360 : makes(view) ? 300 : 240)

/** `16:9` as a number, if it's one. */
export function ratioOf(value: unknown): number | undefined {
  const parts = typeof value === "string" ? value.split(":").map(Number) : []
  return parts.length === 2 && parts[0] > 0 && parts[1] > 0 ? parts[0] / parts[1] : undefined
}

export function aspectOf(view: ElementView): number {
  const shown = shownOf(view)
  // On its controls a box is the shape it will make.
  const shaped = faceOf(view) === "edit" ? ratioOf(view.shape ?? view.sent?.aspect_ratio ?? view.asset?.fields?.aspect_ratio) : undefined
  if (shaped) return shaped
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
