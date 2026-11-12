import type { ErrorBody, MainFile, Tool } from "../api/calcifer"
import type { ElementView } from "./CanvasStore"
import type { Box } from "./graph"

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
export function shownOf(view: ElementView, selectedOnly = false): Shown | null {
  const asset = view.asset
  if (!asset) return view.landed ? { output: view.landed.field, path: "", sha256: view.landed.sha256, field: view.landed.field } : null
  // Looking at a take still being made: nothing of it to show yet, and no other take's picture stands in.
  const watch = selectedOnly ? undefined : watched(view)
  if (watch && watch.number === undefined) return null
  const versions = asset.prefixes ?? []
  const version = (number?: number | null) => versions.find((held) => held.number === number)
  const held = (selectedOnly ? undefined : version(watch?.number) ?? version(view.viewed)) ?? version(asset.version)
  if (held?.main) return { ...held.main, number: held.number, run: held.run ?? undefined }
  if (asset.sha256 && asset.path && asset.field) {
    const { path, sha256, field, width, height, duration_ms, alpha, absent, machine } = asset
    return { output: field, path, sha256, field, width, height, duration_ms, alpha, absent, machine, ...(asset.kind === "image" ? { number: 1 } : {}) }
  }
  const newest = versions.at(-1)
  return newest?.main ? { ...newest.main, number: newest.number, run: newest.run ?? undefined } : null
}

/** Inputs use the completed selected state, independently of a displayed pending run. */
export const selectedOf = (view: ElementView): Shown | null => shownOf(view, true)

/** A card's size, unmeasured, so a card culled away still counts in a fit. */
export const HEAD = 32

/**
 * What hangs off a box at rest, its name over it and its tool, shape and render's lines under it (cl-932), is drawn at the canvas's scale
 * like the rest of it. A box keeps that room clear, once: its name never lies on the box above, nor what hangs under one on the box below,
 * and boxes stacked are as close as boxes side by side, clear's gap apart (Paul, 11-12: "allow for tiles to get a little closer on the top
 * and bottoms").
 */
const HANG = 1
// As app.css draws them: tokens.css's title and body steps at the frame's --ui, on lines of 1.25, and its × 30 square.
const TITLE_LINE = 15.5 * 1.35 * 1.25
const BODY_LINE = 12.5 * 1.35 * 1.25
const CLOSE = 30
/** A selected box's ring, outside its picture, that the line under it starts past. */
const RING = 9
/** How far its head rises over its box at most, its name or its × whichever is taller: HEAD tall, it keeps 24 of it under them, clear of the picture. */
const RISE = Math.ceil(Math.max(TITLE_LINE, CLOSE) * HANG - (HEAD - 24))
/** A render's two lines (RunFace) at the body step on lines of 1.35, the second as tall as its 28px Cancel, with the gaps over them. */
const RUN = 12.5 * 1.35 * 1.35 + 28 + 6

/** The room a box keeps clear under it (cl-932): its tool and shape, and if it renders, how its render goes, then the room the name of a box
 *  below it rises into. */
export function underOf(view: ElementView): number {
  const asset = view.asset
  const line = asset && (asset.tool || asset.kind !== "text") ? RING + BODY_LINE + 2 : 0
  return Math.ceil((line + (makes(view) ? RUN : 0)) * HANG) + RISE
}
const ENTITY_DESC = 60
const ENTITY_SLOTS = 72
export const SUB_ROW = 136

/** What a tool makes, once it's run, as anything on the canvas is. */
export const makes = (view: ElementView | undefined) => Boolean(view?.asset?.tool) && (!madeHere(view?.asset?.tool) || Boolean(view?.asset?.timeline))

/** A tool named away from its package's heading: `Qwen Image 2 · Image`. */
export const toolName = (tool: Tool) => (tool.package_title && tool.package_title !== tool.label ? `${tool.package_title} · ${tool.label}` : tool.label)

/** What Calcifer makes itself, free, with no machine. */
export const madeHere = (tool?: string | null) => tool === "cozy" || Boolean(tool?.startsWith("cozy/"))

/** A box that makes something is what it's asked for; approved, it's that file, until its words are asked for. */
export const faceOf = (view: ElementView): "edit" | "media" => view.asset?.kind === "image" ? (view.hereWords ? "edit" : "media") : (makes(view) && (view.hereWords || !view.asset?.pinned) ? "edit" : "media")

/** No box is narrower: its corner stops here, and nothing shrinks one past it. */
export const NARROWEST = 160

export const widthFor = (view: ElementView | undefined): number => (view?.entity ? 240 : view?.asset?.kind === "video" ? 360 : makes(view) ? 300 : 240)

/** `16:9` as a number, if it's one. */
export function ratioOf(value: unknown): number | undefined {
  const parts = typeof value === "string" ? value.split(":").map(Number) : []
  return parts.length === 2 && parts[0] > 0 && parts[1] > 0 ? parts[0] / parts[1] : undefined
}

/** What its fields show: what's typed, else what was sent, else what it holds. */
export const valuesOf = (view: ElementView | undefined): Record<string, unknown> => ({ ...view?.asset?.fields, ...view?.sent, ...view?.drafts })

/** What a field shows: what's typed, else what was sent, else what it holds. */
export function valueIn(view: ElementView | undefined, field: string): unknown {
  if (view?.drafts && field in view.drafts) return view.drafts[field]
  if (view?.sent && field in view.sent) return view.sent[field]
  return view?.asset?.fields?.[field]
}

/** The take its frame looks at while it's made, open, by its id (cl-945): its run once that has a number, and its number once it's landed.
 *  At rest a box looks at what it showed, so a render never changes its size (cl-936): an approved one goes on showing what's approved,
 *  and what's made lands beside it, unseen (cl-928); with none, the new take is its state once it lands. */
export function watched(view: ElementView): { id: string; run?: number; number?: number } | undefined {
  const id = view.watch
  if (id === undefined || !view.here) return undefined
  const version = view.asset?.prefixes?.find((held) => held.id === id)
  const take = view.asset?.takes?.find((held) => held.id === id)
  return { id, run: take?.run ?? version?.run ?? undefined, number: version?.number }
}

/** A take not landed, by its id: what Calcifer lists, then this page's own it doesn't list yet, from the click (cl-945). */
export interface Taking {
  id: string
  run?: number
  queued?: boolean
  /** It failed, and why: listed until it's deleted (cl-954). */
  failure?: ErrorBody
}

/** Its takes not landed, in the order they were asked: a take is listed from the click to its version, and is never another's. */
export function takingOf(view: ElementView): Taking[] {
  const landed = new Set([...(view.asset?.prefixes ?? []).map((held) => held.id).filter(Boolean), ...(view.gone ?? [])])
  const listed = (view.asset?.takes ?? []).filter((take) => !landed.has(take.id)).map((take) => ({ id: take.id, run: take.run ?? undefined, queued: take.queued, failure: take.failure ?? undefined }))
  const known = new Set(listed.map((take) => take.id))
  return [...listed, ...(view.taking ?? []).filter((take) => !known.has(take.id) && !landed.has(take.id)).map((take) => ({ id: take.id, queued: true }))]
}

/** Why the take its frame looks at failed, when it did (cl-954): what the frame shows in its place. */
export function failureOf(view: ElementView): ErrorBody | undefined {
  const watch = watched(view)
  return watch ? takingOf(view).find((take) => take.id === watch.id)?.failure : undefined
}

/** A take's id, made where it's asked for, as Calcifer makes one: `project-` and 32 hex digits. */
export function newTakeId(): string {
  return `project-${Array.from(crypto.getRandomValues(new Uint8Array(16)), (byte) => byte.toString(16).padStart(2, "0")).join("")}`
}

/** The run making the take a box's frame is on while it's made: the one it looks at, else the first going. None: not sent yet. */
export function makingRun(view: ElementView): number | undefined {
  const watch = watched(view)
  const making = view.asset?.making ?? []
  return watch ? watch.run : making.length ? Math.min(...making) : undefined
}

/** A box an image tool makes: open, its takes have a + beside them, which writes the next one in its frame. */
export const paints = (view: ElementView, tools: Map<string, Tool>) => makes(view) && !view.asset?.timeline && tools.get(view.asset?.tool ?? "")?.makes === "image"

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
  // Showing a take, its own shape, whatever the last one measured was. A take just landed and not measured yet (its file on its way here)
  // keeps the shape of the newest one that is: a box never changes size because a render landed (cl-936).
  const shown = shownOf(view)
  const measured = shown?.width && shown.height ? shown.width / shown.height : undefined
  const before = [...(view.asset?.prefixes ?? [])].reverse().find((held) => held.main?.width && held.main.height)?.main
  return measured ?? view.aspect ?? (before ? before.width! / before.height! : undefined) ?? ((shown?.field ?? view.asset?.kind ?? view.placing?.media) === "audio" ? 3 : 16 / 9)
}

/** Image navigation keeps the height established by v1 and the saved canvas width.
 * Editing still draws the requested output frame, so its corner can change shape. */
function imageViewHeight(view: ElementView, canonicalWidth: number): number | undefined {
  if (view.asset?.kind !== "image" || view.hereWords || !shownOf(view)) return undefined
  const versions = view.asset.prefixes ?? []
  const original = versions.find((version) => version.number === 1)?.main ?? versions[0]?.main
  const first = original?.width && original.height ? original.width / original.height : undefined
  const lone = !versions.length && view.asset.width && view.asset.height ? view.asset.width / view.asset.height : undefined
  // Not measured yet: a dropped picture's shape, as the page read it.
  return canonicalWidth / (first ?? lone ?? view.aspect ?? 16 / 9)
}

/** The displayed body width; the saved canvas width continues to describe v1. */
export function bodyWidth(view: ElementView, canonicalWidth: number): number {
  const height = imageViewHeight(view, canonicalWidth)
  return height === undefined ? canonicalWidth : height * aspectOf(view)
}

/** Version widths expand around the saved body's center, without moving its top. */
export function bodyBox(view: ElementView, canonical: Box): Box {
  const width = bodyWidth(view, canonical.w)
  if (imageViewHeight(view, canonical.w) === undefined) return canonical
  return { ...canonical, x: canonical.x + (canonical.w - width) / 2, w: width, h: heightFor(view, canonical.w) }
}

/** A displayed fit result written back to the v1-based canvas row. */
export function canvasBox(view: ElementView, displayed: Box): Box {
  const width = displayed.w / bodyWidth(view, 1)
  if (imageViewHeight(view, 1) === undefined) return displayed
  return { ...displayed, x: displayed.x + (displayed.w - width) / 2, w: width, h: heightFor(view, width) }
}

const hasSlots = (links?: Record<string, string>) => Object.keys(links ?? {}).length > 0

export function heightFor(view: ElementView, width: number): number {
  if (view.entity) {
    const subs = view.entity.subs ?? []
    return HEAD + ENTITY_DESC + (hasSlots(view.entity.links) ? ENTITY_SLOTS : 0) + subs.length * SUB_ROW
  }
  return HEAD + Math.round(imageViewHeight(view, width) ?? width / aspectOf(view)) + (isPicture(view) ? 40 : 0)
}

/** A picture's box, from its drop on: its row under it is there from the first frame. */
export const isPicture = (view: ElementView) => (view.asset?.kind ?? view.placing?.media ?? view.landed?.field) === "image"

/** A digest as a file is named by it. */
export const hexOf = (sha256: string) => sha256.replace(/^sha256:/, "").toLowerCase()
