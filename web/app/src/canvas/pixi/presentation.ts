import type { Machine, Operation, Run, Tool } from "../../api/calcifer"
import type { CanvasStore, ElementView } from "../CanvasStore"
import type { Box } from "../graph"
import { fieldsOf } from "../controls"
import { ratioWords } from "../ratios"
import { faceOf as runFace, short, type Face } from "../RunFace"
import { type NodeState, viewState } from "../StateDot"
import { aspectOf, HEAD, hexOf, isPicture, makes, ratioOf, shownOf, toolName, valueIn, valuesOf } from "../shown"
import { clock, lineAt, mentionsIn } from "../text"

export interface RunContext {
  run?: Run
  machine?: Machine
  on?: Operation
  installing?: Operation
}

/** Logical geometry remains in CanvasStore/controller even when its display objects are culled. */
export interface SceneNode {
  id: string
  name: string
  /** The displayed body's world box, including its HEAD and existing image-row space. */
  box: Box
  view: ElementView
  selected: boolean
  /** Transient presentation flags; box remains the canonical target used by the store. */
  gliding?: boolean
  dragging?: boolean
  offering?: boolean
  /** Host query subscriptions can supply the same machine/install context as native RunFace. */
  runContext?: RunContext
}

export interface ScenePicture {
  sha256?: string
  path?: string
  url?: string
  kind: string
  alpha?: boolean
  shape?: number
  /** A landed image glimpse is read directly from the run's output endpoint. */
  glimpse?: NonNullable<ElementView["glimpse"]>
}

export interface EntitySlot {
  field: string
  target: string
  picture?: ScenePicture
  label: string
}

export interface EntityRow {
  name: string
  description: string
  slots: EntitySlot[]
}

export interface Presentation {
  title: string
  version?: string
  state: NodeState | null
  unseen: number
  kind: string
  picture?: ScenePicture
  references: ScenePicture[]
  words: string
  /** Mentions in `words` its tool can't use there (cl-944): struck through, as Focus strikes them. */
  unused: string[]
  note: string
  error?: string
  tool: string
  shape: string
  from: string
  progress?: Face & { time: string; fraction?: number; cancel: boolean }
  duration: string
  entity?: { kind: string; description: string; slots: EntitySlot[]; rows: EntityRow[] }
}

/** Media bounds, excluding the title and an image's existing lower row. No DOM measurement or duplicate layout owner. */
export function pictureBox(node: SceneNode): Box {
  const { box, view } = node
  if (view.entity) return { ...box }
  return { x: box.x, y: box.y + HEAD, w: box.w, h: Math.max(1, box.w / aspectOf(view)) }
}

function pictureOf(view: ElementView): ScenePicture | undefined {
  const shown = shownOf(view)
  const own = view.preview && (!view.preview.sha256 || !shown || hexOf(shown.sha256) === view.preview.sha256)
  if (own && view.preview) return { url: view.preview.url, kind: view.placing?.media ?? shown?.field ?? view.asset?.kind ?? "image" }
  if (shown && !shown.absent && ["image", "video", "audio"].includes(shown.field)) {
    return { sha256: shown.sha256, path: shown.path, kind: shown.field, alpha: shown.alpha ?? undefined }
  }
  if (!shown && view.glimpse?.media === "image" && view.viewed === undefined) {
    return { kind: "image", glimpse: view.glimpse }
  }
  const first = view.asset?.timeline?.tracks[0]?.clips.find((clip) => clip.sha256 && !clip.missing)
  if (!shown && first?.sha256) return { sha256: first.sha256, kind: "video" }
  return undefined
}

function shapeOf(view: ElementView, tools: Map<string, Tool>): string {
  const asset = view.asset
  if (!asset || view.shape) return view.shape ? ratioWords(view.shape) : ""
  const field = fieldsOf(asset, tools).find((held) => held.type === "shape")
  const value = field ? valueIn(view, field.field) : undefined
  if (typeof value === "string" && ratioOf(value)) return ratioWords(value)
  const shown = shownOf(view)
  const asked = asset.prefixes?.find((version) => version.number === shown?.number)?.aspect_ratio
  if (asked && ratioOf(asked)) return ratioWords(asked)
  if (shown?.width && shown.height) return ratioWords(shown.width / shown.height)
  if (!field) return ""
  if (field.auto) return view.auto ? `Auto · ${ratioWords(view.auto)}${view.autoFrom ? ` from ${view.autoFrom}` : ""}` : "Auto"
  return ratioWords(typeof field.default === "string" && ratioOf(field.default) ? field.default : String(field.ratios?.[0] ?? "1:1"))
}

function slotsOf(store: CanvasStore, links?: Record<string, string>): EntitySlot[] {
  return Object.entries(links ?? {}).map(([field, target]) => {
    const view = store.get(target)
    const shown = view && shownOf(view)
    return {
      field, target,
      picture: view && shown?.field !== "audio" ? pictureOf(view) : undefined,
      label: view?.asset?.standing === "making" ? "making" : field,
    }
  })
}

/** Same domain helpers as Card/Media/RunFace. This projection does not mutate, fetch, subscribe or keep document state. */
export function present(store: CanvasStore, node: SceneNode, now = Date.now()): Presentation {
  const { view, name } = node
  const asset = view.asset
  const tools = store.tools.get()
  const shown = shownOf(view)
  const kind = view.entity?.kind ?? shown?.field ?? view.placing?.media ?? asset?.kind ?? "file"
  const line = asset ? lineAt(valuesOf(view), fieldsOf(asset, tools)) : null
  let words = line?.words ?? "", at = line?.at
  const tool = asset?.tool ? tools.get(asset.tool) : undefined
  const failed = viewState(view) === "failed" ? asset?.failure : undefined
  const output = pictureOf(view)
  const run = node.runContext?.run ?? view.run
  const onItsWay = asset?.standing === "making" || Boolean(view.queued) || view.asked !== undefined
  const references: ScenePicture[] = []
  if (onItsWay && asset && makes(view) && !asset.timeline && ["image", "video"].includes(asset.kind) && asset.rendering == null) {
    const control = fieldsOf(asset, tools).find((field) => field.type === "prompt")
    const input = run?.request?.input
    const said = control && input?.[control.field]
    const asked = said && typeof said === "object" && !Array.isArray(said) ? (said as { text?: unknown }).text : said
    // A known run's own input is authoritative. Until its full read arrives, another edit cannot stand in for what it was sent.
    if (run) { words = typeof asked === "string" ? asked : ""; at = control?.field }
    const files = new Set<string>()
    let specified = false
    const visit = (value: unknown) => {
      if (Array.isArray(value)) { value.forEach(visit); return }
      if (!value || typeof value !== "object") return
      const held = (value as { references?: unknown }).references
      if (Array.isArray(held)) {
        specified = true
        for (const item of held as ({ type?: unknown; file?: unknown } | null)[]) if ((item?.type === "image" || item?.type === "video") && typeof item.file === "string" && /^(sha256:)?[0-9a-f]{64}$/i.test(item.file)) files.add(item.file)
      }
      Object.values(value).forEach(visit)
    }
    visit(input)
    if (specified) references.push(...[...files].map((sha256) => ({ sha256, kind: "image" })))
    else if (words) references.push(...store.mentionedPictures(name, mentionsIn(words)).map((picture) => ({ sha256: picture.sha256, kind: "image", shape: picture.shape })))
  }
  const arriving = shown?.absent && run?.status === "completed" && run.output.some((item) => !item.stays && !item.checkpoint && !item.url)
  let progress: Presentation["progress"]
  if (asset && !view.placing && (onItsWay || arriving) && asset.rendering == null) {
    const waits = asset.standing === "making" || !onItsWay ? null : store.waitsFor(name)
    const face = runFace({
      view, ...node.runContext, run,
      waits: waits && !node.runContext?.on ? waits : undefined,
      ahead: store.aheadOf(name), preparing: store.preparing(name), canceling: Boolean(view.canceling),
      boxOf: (number) => store.boxOfRun(number),
    })
    const left = run?.status === "completed" ? run.arriving?.eta_s : run?.progress?.eta_s
    const since = run ? Math.max(0, (now - run.created_at_ms) / 1000) : undefined
    const time = since !== undefined && left != null ? ` - ${short(since)} / ${short(since + left)}` : ""
    progress = {
      ...face, time, cancel: !["canceling", "failed", "result"].includes(face.phase),
      fraction: since !== undefined && left != null && left > 0 ? Math.min(0.99, since / (since + left)) : undefined,
    }
  } else if (asset?.rendering != null) {
    progress = { phase: "local", machine: "", state: `Rendering ${Math.round(asset.rendering * 100)}%`, time: "", fraction: asset.rendering, cancel: false }
  }
  const pickup = asset?.prefixes?.find((version) => version.number === shown?.number)?.pickup
  const continued = pickup && store.mentionOf(pickup.take.replace(/^<|>$/g, ""), name)
  const from = continued && continued.name !== store.resolveName(name) ? `from ${continued.name}` : ""
  let note = ""
  if (shown?.absent && !arriving) note = shown.machine ? `Kept on ${shown.machine}` : "Kept on another computer"
  else if (view.placing && !output) note = ({ waiting: "Waiting", reading: "Reading file", sending: "Sending file", adding: "Adding file" })[view.placing.stage]
  else if (!output && !words) note = asset?.timeline ? "Drag videos and stills here" : kind === "audio" ? "Audio" : kind === "file" ? "A file" : makes(view) ? kind === "video" ? "Video" : kind === "image" ? "Image" : "" : ""
  return {
    title: view.placing ? store.shownName(store.resolveName(name)) : view.name,
    version: asset?.kind !== "text" && shown?.number !== undefined ? `v${shown.number}` : !asset && isPicture(view) ? "v1" : undefined,
    state: view.entity ? view.linked === "failed" ? "failed" : view.linked === "making" ? "running" : null : viewState(view),
    unseen: asset?.unseen?.length ?? 0, kind, picture: output, references, words, note,
    unused: (asset?.unused ?? []).filter((one) => one.at === at).map((one) => one.mention),
    error: failed?.message.replace(`${name}: `, "") ?? view.refused,
    tool: asset?.tool ? tool ? toolName(tool) : asset.tool : "",
    shape: shapeOf(view, tools), from, progress,
    duration: shown?.duration_ms ? clock(shown.duration_ms) : "",
    entity: view.entity ? {
      kind: view.entity.kind, description: view.entity.description ?? "", slots: slotsOf(store, view.entity.links),
      rows: (view.entity.subs ?? []).map((sub) => ({ name: sub.name, description: sub.description ?? "", slots: slotsOf(store, sub.links) })),
    } : undefined,
  }
}
