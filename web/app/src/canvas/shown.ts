import { Position } from "@xyflow/react"
import type { MainFile } from "../types"
import type { ElementView } from "./CanvasStore"
import { ABOVE, BELOW, subSource, subTarget } from "./graph"
import { beatsOf, piecesOf } from "./text"

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

/** Where a card's shape comes from: what's measured, unmeasured, so a card culled away still has a size and edges to it still land. */
export const HEAD = 30
const STATUS = 27
const BAR = 18
const ENTITY_DESC = 60
const ENTITY_SLOTS = 72
export const SUB_ROW = 136

export const widthFor = (view: ElementView | undefined): number => (view?.entity ? 240 : view?.asset?.kind === "video" ? 360 : 240)

export function aspectOf(view: ElementView): number {
  const shown = shownOf(view)
  const measured = shown?.width && shown.height ? shown.width / shown.height : undefined
  return view.aspect ?? measured ?? ((shown?.field ?? view.asset?.kind ?? view.placing?.media) === "audio" ? 3 : 16 / 9)
}

const hasBar = (view: ElementView) => beatsOf(view.asset?.fields).length > 0 || piecesOf(view.asset?.fields).length > 0

const hasSlots = (links?: Record<string, string>) => Object.keys(links ?? {}).length > 0

export function heightFor(view: ElementView, width: number): number {
  if (view.entity) {
    const subs = view.entity.subs ?? []
    return HEAD + ENTITY_DESC + (hasSlots(view.entity.links) ? ENTITY_SLOTS : 0) + subs.length * SUB_ROW
  }
  return HEAD + Math.round(width / aspectOf(view)) + (hasBar(view) ? BAR : 0) + STATUS
}

export interface Anchor {
  id?: string
  type: "source" | "target"
  position: Position
  x: number
  y: number
  width: number
  height: number
}

/** Each place an edge lands on a card. The first of a type is the one an edge with no handle takes. */
export function anchorsFor(view: ElementView, width: number): Anchor[] {
  const side = (type: Anchor["type"], y: number, id?: string): Anchor => ({ id, type, position: type === "target" ? Position.Left : Position.Right, x: type === "target" ? 0 : width - 1, y, width: 1, height: 1 })
  if (view.entity) {
    const top = HEAD + ENTITY_DESC + (hasSlots(view.entity.links) ? ENTITY_SLOTS : 0)
    const subs = (view.entity.subs ?? []).flatMap((sub, i) => {
      const y = top + i * SUB_ROW + 14
      return [side("target", y, subTarget(sub.name)), side("source", y, subSource(sub.name))]
    })
    return [side("target", HEAD / 2), side("source", HEAD / 2), ...subs]
  }
  const media = Math.round(width / aspectOf(view))
  const middle = HEAD + Math.round(media / 2)
  return [
    side("target", middle),
    side("source", middle),
    { id: BELOW, type: "source", position: Position.Bottom, x: Math.round(width / 2), y: HEAD + media - 1, width: 1, height: 1 },
    { id: ABOVE, type: "target", position: Position.Top, x: Math.round(width / 2), y: 0, width: 1, height: 1 },
  ]
}
