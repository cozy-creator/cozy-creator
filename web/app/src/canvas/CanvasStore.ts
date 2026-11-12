import type { Edge, Node, ReactFlowInstance, Viewport } from "@xyflow/react"
import { flushSync } from "react-dom"
import { ApiError, type Client, type Write } from "../api/client"
import { type Action, lost, type OutboxPort } from "../api/outbox"
import { nextProjectOrder } from "../api/projectOrder"
import type {
  ToolField as BoxField,
  Fit,
  Imported,
  AssetDeleted,
  AssetPatched,
  Asset,
  HistoryStepped,
  MainFile,
  Pickup,
  RentFor,
  Card,
  Run,
  Standing,
  Tool,
  VersionMoved,
  VersionPrefix,
} from "../api/calcifer"
import type { Entity, Project } from "../types"
import { dismiss, say } from "../ui/notes"
import { signal } from "../ui/signal"
import { carriedFile } from "./carry"
import { addOnsOf } from "./checkpoints"
import { Field, fit, type Give, GLIDE_MS, pushAside } from "./clear"
import { type Box, homesFor, readsOf, warningsOf } from "./graph"
import type { ReadableProjection } from "./readableSurface"
import { fieldsOf, fullRows, heldTo, shapesOf, snapped } from "./controls"
import { angleOf, baseOf } from "./ranges"
import { asName, freeName, nameKey } from "./names"
import { faceOf, heightFor, madeHere, makes, NARROWEST, paints, ratioOf, refusal, reshapes, sentShape, type Shown, shownOf, toolName, underOf, valueIn, valuesOf, watched, widthFor } from "./shown"
import { mentionsIn, rowsIn, textsIn } from "./text"
import { clipOf, wire } from "./timeline/model"
import { PageHistory, type Target } from "./history"
import { comingOff, goesBeside, goingOn, holdsPictures, startsFrom, type ToolStep } from "./toolchange"
import { isVideoEdit, toolFitsNode, type Operation } from "./palette"
import { isSelf, mentionFrom, numbersOf, SELF, versionIn, versionOf } from "./versions"

/** The body width in this geometry publication; the node width may be an opened envelope. */
export type ElementData = { name: string; width: number }
export type ElementNode = Node<ElementData, "element" | "entity">

/** A box that takes elements dragged onto it, at a point: a text at its caret, a slot. A tool can register one of its own. */
export interface DropTarget {
  over(names: string[], x: number, y: number): void
  drop(names: string[], x: number, y: number): void
  /** Why it won't take these, said as they're carried over it and at the drop, or null when it will. */
  whyNot?(names: string[]): string | null
}

export type MediaKind = "image" | "video" | "audio" | "file"
export type Stage = "waiting" | "reading" | "sending" | "adding"

/** An import under way: from the drop until the project holds it. */
export interface Placing {
  media: MediaKind
  stage: Stage
  /** How far this stage is, 0 to 1. */
  done: number
  /** Which try of the current call, when it isn't the first. */
  attempt?: number
  /** The name with nothing typed. */
  stem: string
  abort: AbortController
}

/** Everything a card shows that isn't a position. Culling unmounts cards, so a card holds none of it. */
export interface ElementView {
  name: string
  /** The operation chosen on this page; a model can support both image creation and editing. */
  operation?: Operation
  asset?: Asset
  entity?: Entity
  /** An entity's: how the assets it links stand, the worst of them. */
  linked?: Standing | "failed"
  /** The version being looked at, which changes only this page, until it's pinned. */
  viewed?: number
  /** Its turn hasn't come: nothing is making it yet. */
  queued?: boolean
  /** What reading the project said of it. */
  warnings?: string[]
  /** A piece of a cut was clicked: where to play from. `at` is seconds, `ask` tells one click from the next. */
  seek?: { at: number; ask: number }
  /** Known from the import's answer until the project is read again. */
  landed?: { sha256: string; field: string }
  /** The project doesn't hold it, as far as reads say. Until when: a read begun after that says. Forever until Calcifer answers. */
  pending?: number
  /** A render is on its way, until when, as `pending`: it says Queued from the click on. */
  asked?: number
  /** What was asked of its row and not read back yet, oldest first. */
  over?: Asking[]
  /** How many writes of its fields are on their way. */
  writing?: number
  placing?: Placing
  /** A blob URL, until the thumbnail is up. */
  preview?: string
  /** The name field's text, while it's open. */
  draft?: string
  aspect?: number
  /** Being worked on where it stands, the canvas drawn in on it and all it can do around it. */
  here?: boolean
  /** Worked on where it stands, and showing its words, not a take. */
  hereWords?: boolean
  run?: Run
  /** What's typed and not saved, by field. A card culled away keeps it here. */
  drafts?: Record<string, unknown>
  /** What was written and not read back yet, by field. */
  sent?: Record<string, unknown>
  /** The `etag` the drafts began from, moved along by writes of its own. */
  base?: string
  /** The `etag` its own last write left, until a read catches up: what a new draft begins from. */
  known?: string
  /** The file moved under the drafts. */
  conflict?: boolean
  /** Its render waits for a machine, with its pickup if it has one. */
  waiting?: { pickup?: Pickup }
  /** This computer would render it poorly, so it asks before it does, with its pickup if it has one. */
  asking?: { fit: Fit; also?: { pickup?: Pickup } }
  /** Seconds into a video, scrubbed to. */
  scrub?: number
  /** Its ratio while it's being reshaped. */
  shape?: string
  /** The shape it makes with none set: `autoOf`. */
  auto?: number
  /** Its warnings are open under it. */
  warned?: boolean
  /** Something is being dragged over it. */
  hot?: boolean
  /** What's dragged over it can't go in, and why. */
  refused?: string
  /** Rendered from its words: the newest take asked after this run is the one looked at (`watched`). */
  watchFrom?: number
  /** A take being made, picked by its tile: its run, looked at until it lands, and then as it lands. */
  watchRun?: number
  /** How many files dropped on it are on their way to being its versions. */
  joining?: number
  /** It just arrived, and lands. */
  landing?: boolean
  /** What its render has shown so far, the newest: kept, dimmed, if the render fails. */
  glimpse?: { run: string; output: string; index: number; media: string; label?: string; sha256?: string }
  /** Another box's take let go on it, to pick up from: that take, as a mention of it, and where it's from. */
  pickingUp?: { take: string; from: string; number: number }
  /** The take that just landed, sliding in beside the last. */
  slid?: number
}

/** One thing asked of a box's row: shown over what's read until a read begun after its answer, `until`, as `pending`. */
interface Asking {
  row: Partial<Asset>
  until: number
}

/** A row as read, under what's been asked of it and not read back. */
const asked = (read: Asset, over: Asking[] = []): Asset => over.reduce((asset, ask) => ({ ...asset, ...ask.row }), read)

/** A take or version carried out of its box. */
export interface CarriedFile {
  sha256: string
  field: string
  name: string
  description?: string
  /** The box it's from, and its number there. */
  from?: string
  number?: number
}

/** Rows an edit pushed aside, sent with the box it makes: by name, the rows by node id, and by name the revision each was worked out from. */
export interface Pushes {
  moves: Record<string, Card>
  ids: Map<string, Card>
  placed: Record<string, number>
}

/** A deleted box: what it was, and where, to show again. */
interface Gone {
  name: string
  view: ElementView
  row: Card
}

/** A box as a step found it: what it showed (none: it wasn't there) and where, by its node if it had one, so a rename since still finds it. */
interface Snap {
  name: string
  node?: string
  view?: ElementView
  row?: Card
}

/** What a box's history puts back of its row: what it's made by, with, and from. */
const ROWED = ["tool", "fields", "timeline", "pinned", "approved", "version", "versions", "prefixes", "sha256"] as const
const rowOf = (asset: Asset): Partial<Asset> => Object.fromEntries(ROWED.map((field) => [field, asset[field]]))

export const CARD_WIDTH = 240

type Flow = ReactFlowInstance<ElementNode, Edge>
const NOT_THERE = Symbol("the box went before its turn came")
const NOT_YET = Number.POSITIVE_INFINITY
export const UNDO_MS = 10_000
const TYPING_PAUSE_MS = 800
const CLEAR_OF_FRAME = 24
/** The gap between a box and one made beside it. */
const BESIDE = 32

/** A picture its words mention, faded behind them: its file, or with none yet its frame's shape and whether it's being made. */
export interface Ghost {
  mention: string
  sha256?: string
  shape?: number
  making?: boolean
}

/** What moves the shape a box makes, or the one a mention of it sends. */
const SHAPING = ["asset", "drafts", "sent", "aspect"] as const

const samePlace = (a: { x: number; y: number }, b: { x: number; y: number }) => a.x === b.x && a.y === b.y

/** Only this computer can take it: what it would do poorly is asked first. */
const onlyHere = (on: string[]) => on.length === 1 && on[0] === "local"
const unsaid = (fit: Fit | null): fit is Fit => Boolean(fit?.tools.some((tool) => !tool.here))

/** What the state says of one element: an asset, or an entity. */
type Placed = Pick<ElementView, "name" | "asset" | "entity" | "linked" | "queued" | "warnings">

const viewOf = (element: Placed): ElementView => ({ ...element })

/** Its stack's chips: its tool's, and each add-on on that. */
const chipsOf = (view: ElementView) => {
  const tool = view.asset?.tool
  return addOnsOf(valueIn(view, "lora"), valueIn(view, "models")).length + (tool && !madeHere(tool) ? 1 : 0)
}

function linksOf(entity: Entity): string[] {
  return [...Object.values(entity.links ?? {}), ...(entity.subs ?? []).flatMap(linksOf)]
}

const WORST = ["making", "failed", "out_of_date", "missing", "current"] as const

/** How an entity stands: the worst of what it links. */
const worst = (standings: (Standing | "failed" | undefined)[]) => WORST.find((standing) => standings.includes(standing)) ?? "current"

/**
 * What the canvas knows beyond React Flow's positions, viewport and selection: a plain store with per-name subscribers. `node.data` is
 * `{name, width}` carries only its identity and published body geometry. Other view state never touches React Flow, and an import or a rename stays the same node.
 */
export class CanvasStore {
  private retired = false
  projectId: string | null
  private client: Client
  private flow: Flow | null = null
  private views = new Map<string, ElementView>()
  private aliases = new Map<string, string>()
  private subscribers = new Map<string, Set<() => void>>()
  private shapes = new Map<string, string>()
  /** Each view's stack, by the view: a change is a new view. */
  private stacks = new WeakMap<ElementView, number>()
  private pauses = new Map<string, ReturnType<typeof setTimeout>>()
  private asked = new Set<number>()
  /** What's been said of a render that never reached a machine. */
  private unsentSaid = new Set<string>()
  private runs = new Map<number, Run>()
  /** What a person did, in order, on its way to Calcifer. */
  readonly outbox: OutboxPort
  /** The project's history, and what this page did in it: Undo and Redo. */
  readonly history: PageHistory<Snap[]>
  /** The edit what's typed in a box saves under while it goes on being typed there: one entry for all of it. */
  private typing: { name: string; fields: string; edit: string } | null = null
  /** The edit of the act putting a box down: what it pushes aside is part of it. */
  private gesture: string | null = null
  /** What edits joined before their step was made: a file's drop, whose step is made when it's sent. */
  private early = new Map<string, Snap[]>()
  /** The box whose corner is being pulled, where it was, what's around it then, and what the pull pushes aside as it goes. */
  private pulling: { name: string; was?: Box; field?: Field; shown: Set<string> } | null = null
  /** The note offering Undo of each edit, and the one saying what Undo or Redo last did: each goes once that's moved on. */
  private offers = new Map<string, number>()
  private stepped = 0
  /** The tools known so far, by id. */
  readonly tools = signal<Map<string, Tool>>(new Map())
  /** Names came or went: a text shows a name as a chip or as words by it. */
  readonly names = signal(0)
  /** The menu open, and where. */
  readonly menu = signal<{ name: string; x: number; y: number } | null>(null)
  /** The box whose file is being looked at large, over everything. */
  readonly viewing = signal<string | null>(null)
  /** Zoomed this far out a box is its picture, its name and marks for its stack, which is all that can be seen: thousands are drawn at once. */
  readonly far = signal(false)
  /** The canvas's zoom as pictures are asked for it: a power of two, so a zoom asks for a sharper picture only as it doubles. */
  readonly sharpness = signal(1)
  /** What takes a drop, by its element: a box marks itself `data-drop-target`. */
  readonly dropTargets = new Map<Element, DropTarget>()
  /** Each open text's way to take a name at its caret, by `<node>:<place>`. */
  private carets = new Map<string, (name: string) => void>()
  /** Has these assets, and what's said of the project, read. */
  readAgain: (names: string[]) => void = () => {}
  /** Asks for a machine, for a render with none up. */
  askMachine: () => void = () => {}
  /** Opens Rent, with a GPU that holds it chosen when one is known. */
  rent: (pick?: RentFor | null) => void = () => {}
  /** What Render all asks first, on its own control, once for every tool this computer would do poorly. */
  readonly allAsking = signal<{ fit: Fit } | null>(null)
  /** The box being worked on where it stands: the view to go back to, the one fitted to it, and whether a hand has since moved the canvas or is pulling its corner. */
  inPlace: { name: string; back: Viewport; fitted?: Viewport; moved?: boolean; pulled?: boolean } | null = null
  /** Only the opened card has a viewport-dependent presentation. The cached value owns no DOM. */
  private surface: { id: string; current?: ReadableProjection; pose?: Box; zoom?: number; update?: (room?: Box, at?: Viewport) => ReadableProjection | null } | null = null
  private surfaceFrame = 0
  /** Done with the box where it stands: what's typed is saved, and the canvas goes back. */
  leaveHere: () => void = () => {}
  /** Opens a box where it stands, as a click on it does. */
  openHere: (name: string) => void = () => {}
  /** A new box of `tool` at a point, starting with `fields`, as a tool dropped there makes. */
  addToolAt: (tool: Tool, point: { x: number; y: number }, fields?: Record<string, unknown>, operation?: Operation) => void = () => {}
  /** The box to open on its next take's words: an image tool was just put on its pictures. */
  writeOnOpen: string | null = null
  /** Fits the canvas to the box being worked on, and all around it. `only`: only if something no longer fits. */
  fitHere: (only?: boolean) => void = () => {}
  /** The box being worked on has another name now. */
  hereRenamed: (name: string) => void = () => {}
  /** The name a just-dropped tool will have: its text takes the keys. */
  focusing: string | null = null
  /** The machines a render may go to, a rental before this computer: the walk places it on whichever is free. */
  machines: () => string[] = () => []
  private ids = new Map<string, string>()
  /** The last `GET canvas` taken in: a canvas drawn again is handed it again, and what's happened since stands. */
  private cardsRead?: Record<string, Card>
  /** Each box by its node's id, the same whatever it's called: an action waits for its turn by this. */
  private namesById = new Map<string, string>()
  /** What Calcifer calls each, by id: a rename on its way shows here first. */
  private served = new Map<string, string>()
  private nextId = 0
  private state: Project | null = null
  private began = 0
  private rows: Map<string, Card> | null = null
  /** Canonical rows, including an unadopted card and an unsaved corner pull. Never the presentation envelope. */
  private poses = new Map<string, Card>()
  private owners = new Map<number, string>()
  private dragging = new Set<string>()
  /** Where what's being dragged started, for a drop that isn't a move. */
  private dragFrom = new Map<string, { x: number; y: number }>()
  /** On its way out, until a read begun after when its delete was answered: one from before doesn't bring it back, and one from after says, as
   * another writer may have put it back. */
  private deleting = new Map<string, number>()
  /** The etags an asset's own writes left, since a read last caught up. */
  private own = new Map<string, Set<string>>()
  /** Names about to land: what was dropped here, and nothing a read found. */
  private landings = new Set<string>()
  /** A position of ours that the stream hasn't echoed: it wins until then. */
  private masks = new Map<string, Card>()
  private gliding = new Map<string, ReturnType<typeof setTimeout>>()
  /** Boxes to clear of what they lie on, as soon as this turn's changes are in: true for one whose size is its own, which doesn't shrink. */
  private unsettled = new Map<string, boolean>()
  /** When each of this page's moves was answered, by name. */
  private movedAt = new Map<string, number>()
  /** Rows to write, by node id. */
  private moving = new Map<string, Card>()
  /** Of those, what this page worked one out from, where it's this page's own doing: written only if Calcifer still has it, so it can't
   * clobber another writer's newer row. A person's move has none: it's newest. */
  private movingFrom = new Map<string, Card | null>()
  /** Of those, the edits each is part of: a request names the edits of what it writes. */
  private movingEdits = new Map<string, Set<string>>()
  /** The newest revision of the canvas this page has heard, by a read or the stream: what its own placing is worked out from. */
  private canvasSeq = 0
  /** For each row this page worked out itself, the revision of what it was worked out from: another's row written since stands. */
  private movingSince = new Map<string, number>()
  private movesSoon = false
  private movesOwed = false
  private renames = new Map<string, string>()
  private adding = 0
  private seen = false
  private selected: Set<string>
  /** The boxes with More open. */
  private moreOpen: Set<string>
  private named = new Set<() => void>()
  /** The project was read for the first time; whether it held anything. */
  onFirstRead?: (held: boolean) => void
  /** Elements with no row were just placed. */
  onPlaced?: () => void
  /** More was opened or shut. */
  onMore?: (names: string[]) => void

  constructor(client: Client, outbox: OutboxPort, projectId: string | null, selected: Iterable<string>, moreOpen: Iterable<string>) {
    this.client = client
    this.outbox = outbox
    this.projectId = projectId
    this.history = new PageHistory(client, () => this.projectId)
    this.selected = new Set(selected)
    this.moreOpen = new Set(moreOpen)
  }

  /** The project has been read, so what isn't here isn't in it. */
  get hasRead() {
    return this.seen
  }

  /** No known or optimistic element, including one still waiting for its row. */
  get empty() {
    return !this.views.size && !this.state?.assets.length && !this.state?.entities?.length
  }

  /** A canvas drawn for it, the first or again: back on a project, its boxes are as it left them, whatever's still on its way to Calcifer. */
  attach(flow: Flow) {
    if (this.retired) return
    this.flow = flow
    if (this.state && this.rows) this.reconcile()
    // React Flow forgets what it's told as it mounts, which a project read already has in time for: what's held is told again once it's up.
    setTimeout(() => {
      if (this.flow !== flow) return
      flow.setNodes((nodes) => {
        const there = new Set(nodes.map((node) => node.id))
        return [
          ...nodes,
          ...[...this.ids].filter(([, id]) => !there.has(id)).flatMap(([name, id]) => (this.rows?.has(name) ? [this.node(name, this.rows.get(name)!, this.selected.has(name), id)] : [])),
        ]
      })
    })
  }

  /** Its canvas goes, and the store stays for when it's drawn again: nothing is open or held in a drag by then. */
  detach() {
    cancelAnimationFrame(this.surfaceFrame)
    this.surfaceFrame = 0
    this.surface = null
    this.flow = null
    if (this.inPlace) this.patch(this.inPlace.name, { here: undefined, hereWords: undefined })
    this.inPlace = null
    this.dragging.clear()
    this.dragFrom.clear()
    this.setHot(null)
    this.menu.set(null)
    this.viewing.set(null)
  }

  /** A confirmed vault lock retires even an off-route store. Navigation alone keeps its work. */
  revokePrivateView() {
    this.retire()
  }

  async prepareClose() {
    if (this.retired) return
    const saved = await Promise.all([...this.views.keys()].map((name) => this.save(name)))
    this.sendMoves()
    if (saved.some((saved) => !saved)) throw new Error("A canvas edit still needs attention before the browser can close")
  }

  retire() {
    if (this.retired) return
    this.retired = true
    for (const pause of this.pauses.values()) clearTimeout(pause)
    this.pauses.clear()
    for (const view of this.views.values()) {
      view.placing?.abort.abort()
      if (view.preview) {
        URL.revokeObjectURL(view.preview)
        view.preview = undefined
      }
    }
    for (const timer of this.gliding.values()) clearTimeout(timer)
    this.gliding.clear()
    this.flow?.setNodes([])
    this.detach()
    this.projectId = null
    this.state = null; this.rows = null; this.cardsRead = undefined
    this.typing = null; this.gesture = null; this.pulling = null
    this.history.retire()
    this.views.clear(); this.aliases.clear(); this.shapes.clear(); this.asked.clear(); this.unsentSaid.clear(); this.runs.clear()
    this.early.clear(); this.offers.clear(); this.dropTargets.clear(); this.carets.clear(); this.ids.clear(); this.namesById.clear(); this.served.clear()
    this.poses.clear(); this.owners.clear(); this.deleting.clear(); this.own.clear(); this.landings.clear(); this.masks.clear(); this.unsettled.clear()
    this.movedAt.clear(); this.moving.clear(); this.movingFrom.clear(); this.movingEdits.clear(); this.movingSince.clear(); this.renames.clear()
    this.selected.clear(); this.moreOpen.clear(); this.tools.set(new Map()); this.allAsking.set(null)
    for (const listeners of this.subscribers.values()) for (const listener of listeners) listener()
    this.subscribers.clear(); this.named.clear()
    this.menu.set(null)
    this.viewing.set(null)
  }

  // Reading, by name

  resolveName(name: string): string {
    return this.aliases.get(name) ?? name
  }

  get = (name: string): ElementView | undefined => this.views.get(this.resolveName(name))

  subscribe(name: string, listener: () => void): () => void {
    let listeners = this.subscribers.get(name)
    if (!listeners) this.subscribers.set(name, (listeners = new Set()))
    listeners.add(listener)
    return () => listeners.delete(listener)
  }

  private notify(name: string) {
    this.subscribers.get(name)?.forEach((listener) => listener())
    for (const [alias, target] of this.aliases) {
      if (target === name) this.subscribers.get(alias)?.forEach((listener) => listener())
    }
  }

  patch(name: string, change: Partial<ElementView>) {
    if (this.retired) return
    const key = this.resolveName(name)
    const view = this.views.get(key)
    if (!view) return
    this.views.set(key, { ...view, ...change })
    this.notify(key)
    if (SHAPING.some((field) => field in change)) this.follow(key, view)
  }

  /** Its auto shape again, and, if what a mention of it sends moved, theirs. */
  private follow(key: string, was: ElementView, depth = 0) {
    const view = this.views.get(key)
    if (!view || depth > 8) return
    if (sentShape(was) === sentShape(this.reauto(key, view))) return
    for (const reader of this.readersOf(key)) this.follow(reader, this.views.get(reader)!, depth + 1)
  }

  /** Every box's auto shape, once names or tools have changed under them: again while any moves, as one can follow another. */
  private followAll() {
    for (let pass = 0; pass < 4; pass++) if (![...this.views].filter(([key, view]) => this.reauto(key, view) !== view).length) return
  }

  /** Its auto shape, kept on its view: the view it has now. Only a box showing no picture is drawn by it; one showing a picture keeps it for what reads it. */
  private reauto(key: string, view: ElementView): ElementView {
    const auto = this.autoOf(view)
    if (auto === view.auto) return view
    const now = { ...view, auto }
    this.views.set(key, now)
    if (reshapes(now)) {
      this.notify(key)
      this.reshape(key)
    }
    return now
  }

  /** The shape a box makes with none set, as Cozy's image packages take it: the first picture it mentions, held to what its tool makes, else its tool's own. */
  private autoOf(view: ElementView): number | undefined {
    const asset = view.asset
    const tools = this.tools.get()
    const control = asset && makes(view) ? fieldsOf(asset, tools).find((held) => held.type === "shape") : undefined
    if (!asset || !control) return undefined
    const kind = tools.get(asset.tool ?? "")?.makes ?? asset.kind
    for (const mention of this.mentionsOf(view)) {
      const read = this.mentionOf(mention, view.name)
      const of = read ? this.views.get(read.name) : undefined
      const ratio = read && of?.asset?.kind === kind ? sentShape(of, read.number) : undefined
      if (ratio) return heldTo(control, ratio)
    }
    // The SDK's own default with none: 1:1.
    return heldTo(control, ratioOf(control.default) ?? 1)
  }

  /** The pictures a box's words mention, or `mentions` of it, in order: the version named, else what each shows. */
  mentionedPictures(name: string, mentions?: string[]): Ghost[] {
    const view = this.get(name)
    if (!view) return []
    const seen = new Set<string>()
    return (mentions ?? this.mentionsOf(view)).flatMap((mention): Ghost[] => {
      // The box by its exact name first, as Calcifer reads it: `<kyogre.primordial>` is that box, not a take of `kyogre`.
      const read = this.mentionOf(mention, view.name)
      const of = read ? this.views.get(read.name) : undefined
      if (!of?.asset || of.asset.kind !== "image" || this.refusalOf(name, mention) || seen.has(mention)) return []
      seen.add(mention)
      const file = read?.number !== undefined ? of.asset.prefixes?.find((held) => held.number === read.number)?.main : shownOf(of)
      if (file && !file.absent && file.field === "image") return [{ mention, sha256: file.sha256 }]
      // Nothing of it yet: its frame, at the shape it will be, and whether it's being made.
      return [{ mention, shape: sentShape(of) ?? 1, making: Boolean(of.asset.making?.length || of.queued || of.asked !== undefined) }]
    })
  }

  /** What a box asked to render waits on: the first box it reads with nothing made yet, or that's being made. */
  waitsFor(name: string): string | null {
    const asset = this.get(name)?.asset
    for (const read of asset?.reads ?? []) {
      const of = this.get(this.mentionOf(read.name, name)?.name ?? "")
      if (of?.asset?.tool && (of.asset.making?.length || of.queued || of.asked !== undefined || (!of.asset.prefixes?.length && !of.asset.sha256))) return read.name
    }
    return null
  }

  /** Writing its next take in its frame, the caret in its words, which takes them again when it's already there. Words that hold nothing but a take of its
   * own begin from the one looked at, as `<self.N>`; `lead`: whatever they hold, that take is the first they mention. */
  writeNext(name: string, lead = false) {
    const view = this.get(name)
    const asset = view?.asset
    if (!view || !asset?.tool) return
    const field = this.promptOf(asset.tool)
    const said = this.valueOf(name, field)
    const words = typeof said === "string" ? said : ""
    const number = shownOf(view)?.number
    const self = `<${SELF}.${number}>`
    const first = mentionsIn(words)[0]
    const next = /^\s*(<self\.\d+>\s*)?$/i.test(words) ? `${self} ` : !lead ? words : first && isSelf(versionOf(first)?.name ?? "") ? words.replace(`<${first}>`, self) : `${self} ${words}`
    if (number !== undefined && next !== words) this.setDraft(name, field, next)
    this.focusing = name
    this.patch(name, { hereWords: true })
  }

  /** What a box's fields mention, in order. */
  private mentionsOf(view: ElementView): string[] {
    return view.asset?.tool ? fieldsOf(view.asset, this.tools.get()).flatMap((field) => textsIn(valueIn(view, field.field))).flatMap(mentionsIn) : []
  }

  /** The field a tool's next take is written in: its first prompt. */
  private promptOf(tool: string): string {
    return this.tools.get().get(tool)?.fields.find((field) => field.type === "prompt")?.field ?? "instructions"
  }

  /** The boxes whose fields mention `key`. */
  private readersOf(key: string): string[] {
    return [...this.views.values()].filter((view) => view.name !== key && this.mentionsOf(view).some((mention) => this.mentionOf(mention, view.name)?.name === key)).map((view) => view.name)
  }

  patchPlacing(key: string, change: Partial<Placing>) {
    const view = this.views.get(key)
    if (view?.placing) this.patch(key, { placing: { ...view.placing, ...change } })
  }

  /** The names an element can't take. */
  takenBesides(name: string): string[] {
    return [...this.views].filter(([key, view]) => key !== name && !view.placing).map(([key]) => key)
  }

  /** Its node's id, the same whatever it's called. */
  idOf(name: string): string | undefined {
    return this.ids.get(this.resolveName(name))
  }

  position(name: string): Card | null {
    const key = this.resolveName(name)
    const row = this.poses.get(key) ?? this.masks.get(key) ?? this.rows?.get(key)
    return row ? { ...row } : null
  }

  private rememberPose(name: string, row: Card) {
    const view = this.get(name)
    const w = row.w ?? this.poses.get(name)?.w ?? (view ? widthFor(view) : CARD_WIDTH)
    const pose = { x: row.x, y: row.y, w }
    this.poses.set(name, pose)
    return pose
  }

  /** Its canonical body, before any opened surface expands around it. */
  poseOf(name: string): Box | null {
    const key = this.resolveName(name)
    const row = this.position(key)
    const view = this.get(key)
    if (!row || !view) return null
    const w = row.w ?? widthFor(view)
    const id = this.ids.get(key)
    const at = this.dragging.has(key) && id ? this.flow?.getNode(id)?.position : undefined
    return { x: at?.x ?? row.x, y: at?.y ?? row.y, w, h: heightFor(view, w) }
  }

  /** Only a person's drag reads the node's anchor; its width remains the canonical row's. */
  private draggedRow(node: ElementNode): Card {
    const row = this.position(node.data.name)
    return { x: Math.round(node.position.x), y: Math.round(node.position.y), w: row?.w ?? this.poseOf(node.data.name)?.w ?? CARD_WIDTH }
  }

  /** Mount just the opened node for its first real measurement, even when its saved picture is offscreen. */
  openSurface(name: string) {
    const id = this.ids.get(this.resolveName(name))
    if (!id) return
    this.surface = { id }
    this.measureSurface()
  }

  private measureSurface() {
    const id = this.surface?.id
    const owner = this.inPlace
    if (!id || this.surfaceFrame) return
    this.surfaceFrame = requestAnimationFrame(() => {
      this.surfaceFrame = 0
      // An on-screen card binds before this frame. Only a culled one needs mounting.
      if (this.inPlace !== owner || this.surface?.id !== id || this.surface.update) return
      const flow = this.flow
      flushSync(() => flow?.updateNode(id, { height: 0, measured: { width: flow.getNode(id)?.width, height: 0 } }))
    })
  }

  bindSurface(id: string, update: (room?: Box, at?: Viewport) => ReadableProjection | null) {
    if (this.surface?.id !== id) return () => {}
    this.surface.update = update
    return () => {
      if (this.surface?.id === id && this.surface.update === update) this.surface.update = undefined
    }
  }

  surfaceOf(name: string): ReadableProjection | undefined {
    const held = this.surface
    return held && this.ids.get(this.resolveName(name)) === held.id ? held.current : undefined
  }

  drawSurface(room?: Box, at?: Viewport): ReadableProjection | null {
    if (this.surface?.update) return this.surface.update(room, at)
    if (room) this.measureSurface()
    return null
  }

  refreshSurface(viewport: Viewport) {
    if (this.surface?.current && this.surface.zoom === viewport.zoom) this.surface.current = { ...this.surface.current, view: viewport }
    else if (this.surface?.update) this.surface.update()
    else if (this.surface?.current && this.surface.current.view.zoom !== viewport.zoom) this.measureSurface()
  }

  presentSurface(id: string, current: ReadableProjection) {
    if (this.surface?.id !== id) return
    const name = this.namesById.get(id)
    const pose = name && this.poseOf(name)
    const node = this.flow?.getNode(id)
    if (!pose || !node) return
    this.surface.current = current
    this.surface.pose = pose
    this.surface.zoom = this.flow?.getZoom()
    const { x, y, w: bodyWidth } = pose
    const { x: left, y: top, w: width, h: height } = current.bounds
    const origin: [number, number] = [(x - left) / width, (y - top) / height]
    if (node.data.width === bodyWidth && node.width === width && node.height === height && node.position.x === x && node.position.y === y && node.origin?.[0] === origin[0] && node.origin?.[1] === origin[1]) return
    // The card's frame and its React Flow envelope are one layout change, before size observers run.
    flushSync(() => this.flow?.updateNode(id, (queued) => ({ data: { ...queued.data, width: bodyWidth }, position: { x, y }, width, height, origin, measured: { width, height } })))
  }

  closeSurface() {
    const id = this.surface?.id
    cancelAnimationFrame(this.surfaceFrame)
    this.surfaceFrame = 0
    this.surface = null
    const name = id && this.namesById.get(id)
    if (name) requestAnimationFrame(() => {
      if (this.surface?.id !== id) flushSync(() => this.paintPose(name))
    })
  }

  private paintPose(name: string) {
    const id = this.ids.get(name)
    const pose = this.poseOf(name)
    if (!id || !pose) return
    if (this.surface?.id === id) {
      if (this.surface.update) {
        if (!this.surfaceFrame) this.surfaceFrame = requestAnimationFrame(() => {
          this.surfaceFrame = 0
          this.surface?.update?.()
        })
        return
      }
      const before = this.surface.pose
      if (before?.w !== pose.w || before.h !== pose.h) return this.measureSurface()
      this.flow?.updateNode(id, { position: { x: pose.x, y: pose.y } })
      return
    }
    const { x, y, w: width, h: height } = pose
    this.flow?.updateNode(id, (queued) => ({ data: { ...queued.data, width }, position: { x, y }, width, height: undefined, origin: [0, 0], measured: { width, height } }))
  }

  // From Calcifer

  applyState(state: Project) {
    if (this.retired) return
    this.state = state
    this.began = state.began ?? 0
    if (this.rows && this.flow) this.reconcile()
  }

  /** `GET canvas`: every row, but what this page moved since that read began: it can't know. */
  applyCanvas(cards: Record<string, Card>, began = 0, seq?: number) {
    if (this.retired) return
    this.heard(seq)
    if (cards === this.cardsRead) return
    this.cardsRead = cards
    const first = !this.rows
    const rows = new Map(Object.entries(cards))
    for (const [key, view] of this.views) if (view.placing && this.rows?.has(key)) rows.set(key, this.rows.get(key)!)
    for (const [name, at] of this.movedAt) {
      const mine = this.rows?.get(name)
      if (at >= began && mine) rows.set(name, mine)
    }
    this.rows = rows
    if (first) {
      if (this.state && this.flow) this.reconcile()
      return
    }
    for (const [name, row] of this.rows) this.applyRow(name, row)
  }

  /** Heard the canvas at revision `seq`. */
  heard(seq?: number) {
    if (seq !== undefined && seq > this.canvasSeq) this.canvasSeq = seq
  }

  /** A `canvas` event: the rows that changed. Stream order is the truth. */
  applyRows(cards: Record<string, Card | null>, seq?: number) {
    if (this.retired) return
    this.heard(seq)
    if (!this.rows) return
    for (const [name, row] of Object.entries(cards)) {
      if (row) {
        this.rows.set(name, row)
        this.applyRow(name, row)
        if (!this.masks.has(name)) this.unsettle(name)
      } else {
        this.rows.delete(name)
      }
    }
  }

  private applyRow(name: string, row: Card) {
    if (this.dragging.has(name)) return
    const mask = this.masks.get(name)
    if (mask) {
      if (samePlace(mask, row)) this.masks.delete(name)
      return
    }
    this.rememberPose(name, row)
    const id = this.ids.get(name)
    if (id) this.paintPose(name)
    if (row.w) this.reshape(name)
  }

  /** Fell behind: what the stream would have told us is about to be read again, and what a run says now is asked again. */
  fellBehind() {
    const owed = new Set([...this.moving.keys()].map((id) => this.namesById.get(id)))
    for (const name of this.masks.keys()) if (!owed.has(name)) this.masks.delete(name)
    this.asked.clear()
  }

  /** Tools the list didn't have, as they're found. */
  addTools(tools: Tool[]) {
    if (this.retired) return
    const known = this.tools.get()
    const fresh = tools.filter((tool) => JSON.stringify(known.get(tool.id)) !== JSON.stringify(tool))
    if (!fresh.length) return
    this.tools.set(new Map([...known, ...fresh.map((tool): [string, Tool] => [tool.id, tool])]))
    this.followAll()
  }

  /** What a run says of the asset it's making: the lowest going, when there are several. */
  applyRun(run: Run) {
    if (this.retired) return
    if (!this.projectId || run.project_id !== this.projectId) return
    this.runs.set(run.number, run)
    const owner = this.owners.get(run.number)
    if (!owner) return
    const making = this.get(owner)?.asset?.making ?? []
    if (run.number !== Math.min(...making)) return
    // Only what's here: an output of a run on another machine, or a revision that hasn't landed, has nothing yet to show. A preview published
    // as it goes is a revision that landed while its output is still in progress.
    const shown = [...(run.output ?? [])].reverse().find((item) => Boolean(item.url) && /^(image|video)\//.test(item.media_type))
    const glimpse = shown && { run: run.id, output: shown.name, index: shown.index, media: shown.media_type.split("/")[0], label: shown.label, sha256: shown.sha256 ?? undefined }
    const held = this.get(owner)?.glimpse
    this.patch(owner, glimpse && JSON.stringify(glimpse) !== JSON.stringify(held) ? { run, glimpse } : { run })
  }

  private reconcile() {
    const state = this.state!
    const rows = this.rows!
    const entities = state.entities ?? []
    const present = new Set([...state.assets, ...entities].map((element) => element.name))
    const before = [...this.views.keys()]
    for (const [name, until] of this.deleting) if (!present.has(name) || until < this.began) this.deleting.delete(name)
    for (const name of this.deleting.keys()) present.delete(name)
    for (const [from, to] of this.renames) if (present.has(to)) this.renames.delete(from)
    const becoming = new Set(this.renames.values())
    const fresh: Placed[] = []
    // A row written first, as a rename's is: the same box under a new name, where it was, with whatever's typed in it.
    for (const { name } of state.assets) {
      const row = rows.get(name)
      if (this.views.has(name) || !row) continue
      const was = [...this.views.keys()].find((held) => !present.has(held) && !this.views.get(held)?.placing && !becoming.has(held) && samePlace(this.position(held) ?? { x: NaN, y: NaN }, row))
      if (was) this.moveName(was, name)
    }
    if (!state.unsent?.length) this.unsentSaid.clear()
    for (const failure of state.unsent ?? []) {
      if (this.unsentSaid.has(failure.message)) continue
      this.unsentSaid.add(failure.message)
      say(failure.message, new ApiError(0, failure))
    }
    const queued = new Set((state.queue ?? []).filter((item) => !item.running).map((item) => item.name))
    const warnings = warningsOf(state).byElement
    this.owners.clear()
    const said = (view: ElementView, name: string) => {
      const now = warnings.get(name)
      return (view.warnings ?? []).join("\n") === (now ?? []).join("\n") ? {} : { warnings: now }
    }
    for (const asset of state.assets) {
      for (const number of asset.making ?? []) {
        this.owners.set(number, asset.name)
        this.askRun(number)
      }
      if (this.renames.has(asset.name) || this.deleting.has(asset.name)) continue
      const view = this.views.get(asset.name)
      if (view) {
        const change: Partial<ElementView> = { ...said(view, asset.name) }
        // What a new take's run warned of is said as it lands: a box's own mark is only seen opened.
        if (this.hasRead && view.asset && asset.version !== view.asset.version) for (const line of warnings.get(asset.name) ?? []) if (!view.warnings?.includes(line)) say(line)
        // A render that fails says so as it fails, wherever the person is looking, the same failure again too once this page asked for it. One that failed before this page opened is the box's to show.
        const answered = view.asked !== undefined && view.asked < this.began
        if (asset.failure && this.hasRead && (answered || asset.failure.message !== view.asset?.failure?.message)) say(`${asset.name} didn't render`, new ApiError(0, { ...asset.failure, message: asset.failure.message.replace(`${asset.name}: `, "") }))
        if (Boolean(view.queued) !== queued.has(asset.name)) change.queued = queued.has(asset.name)
        if (view.pending !== undefined && view.pending < this.began) change.pending = undefined
        if (answered) change.asked = undefined
        const over = view.over?.filter((ask) => ask.until >= this.began)
        if (view.over && over?.length !== view.over.length) change.over = over?.length ? over : undefined
        if (view.asset !== asset || view.landed) {
          Object.assign(change, { asset: asked(asset, over), landed: undefined })
          if (!asset.making?.length) change.run = undefined
          const newest = asset.prefixes?.at(-1)?.number
          if (newest && newest !== view.asset?.prefixes?.at(-1)?.number && view.asset) Object.assign(change, { slid: newest, glimpse: undefined })
          // Its first take, landing while it's open on its words with nothing typed: it shows the take. An image tool's words are its next take's, and stay.
          if (newest && !view.asset?.prefixes?.length && view.here && view.hereWords && !view.drafts && !asset.timeline && !paints(view, this.tools.get())) change.hereWords = undefined
          // The take it was rendered to look at landed, with nothing newer being made: it's the one looked at.
          const seen = watched({ ...view, asset })
          if (seen?.number !== undefined) Object.assign(change, { watchFrom: undefined, watchRun: undefined, viewed: seen.number === asset.version ? undefined : seen.number })
          // The take picked stopped without one: nothing's being looked at.
          else if (view.watchRun !== undefined && !asset.making?.includes(view.watchRun)) change.watchRun = undefined
          // Read back: what was sent is what it holds, or someone wrote since. A read of a write of its own, older than its last, says neither.
          const between = asset.etag !== view.known && this.own.get(asset.name)?.has(asset.etag ?? "")
          if (asset.etag !== view.asset?.etag && !between) {
            // A box this page made has no one else's words under it yet: what's typed begins from this.
            Object.assign(change, { sent: view.writing ? view.sent : undefined, known: undefined, ...(view.pending !== undefined ? { base: view.drafts ? asset.etag : undefined } : this.typedUnder(view, asset)) })
            this.own.delete(asset.name)
          }
          // Pinned where it was being looked: there's nothing left to look at.
          if (view.viewed !== undefined && (view.viewed === asset.version || !asset.prefixes?.some((held) => held.number === view.viewed))) change.viewed = undefined
        }
        if (Object.keys(change).length) {
          this.patch(asset.name, change)
          this.reshape(asset.name)
        }
      } else if (this.adding === 0) {
        // While an import is being asked, a new name may be that import's: its answer says, then this runs again.
        fresh.push({ name: asset.name, asset, queued: queued.has(asset.name), warnings: warnings.get(asset.name) })
      }
    }
    const standing = new Map(state.assets.map((asset) => [asset.name, asset.failure ? ("failed" as const) : asset.standing]))
    for (const entity of entities) {
      const linked = worst(linksOf(entity).map((name) => standing.get(name)))
      const view = this.views.get(entity.name)
      if (view) {
        if (view.entity !== entity || view.linked !== linked || Object.keys(said(view, entity.name)).length) {
          this.patch(entity.name, { entity, linked, ...said(view, entity.name) })
          this.reshape(entity.name)
        }
      } else if (this.adding === 0) {
        fresh.push({ name: entity.name, entity, linked, warnings: warnings.get(entity.name) })
      }
    }
    for (const [name, view] of this.views) {
      if (!present.has(name) && !view.placing && !view.landed && !(this.began <= (view.pending ?? -1)) && !becoming.has(name)) this.removeNode(name)
    }
    // What comes after the first read arrived while this was open, from a dropped tool, a command or a hand.
    for (const element of this.seen ? fresh : []) this.landings.add(element.name)
    this.addNodes(fresh.filter((element) => rows.has(element.name)).map((element) => ({ element, row: rows.get(element.name)! })))
    const homeless = fresh.filter((element) => !rows.has(element.name))
    if (homeless.length) void this.findHomes(homeless)
    if (before.some((name) => !this.views.has(name))) this.followAll()
    if (!this.seen) {
      this.seen = true
      this.onFirstRead?.(present.size > 0)
      this.separate()
    } else {
      for (const { name } of fresh) this.unsettle(name)
    }
    this.names.set(this.names.get() + 1)
  }

  /** Someone wrote this asset, and what that means for what's typed in it: a field typed in is now a conflict, any other only moves what it began from. */
  private typedUnder(view: ElementView, asset: Asset): Partial<ElementView> {
    if (!view.drafts && !view.writing) return { base: undefined }
    if (asset.etag === view.base) return {}
    const same = (field: string) => JSON.stringify(asset.fields?.[field]) === JSON.stringify(view.asset?.fields?.[field])
    // What's on its way is met by `If-Match`: Calcifer refuses it, and that's this conflict too. It only needs its base to stay, unless no field of it moved.
    const typed = Object.keys({ ...(view.writing ? view.sent : {}), ...view.drafts })
    if (typed.every(same)) return { base: asset.etag ?? undefined }
    return view.drafts ? { conflict: true } : {}
  }

  /** A run this page hasn't heard of: its progress comes on the stream, but what it says now has to be read. */
  private askRun(number: number) {
    if (this.asked.has(number)) return
    this.asked.add(number)
    if (this.projectId) this.client.run({ project: this.projectId, run: number }).then((run) => this.applyRun(run), () => {})
  }

  /** A name with no row may just be ahead of its row on the stream, so ask once. Still none: place it, once, and write the row. */
  private async findHomes(elements: Placed[]) {
    if (!this.projectId) return
    let cards: Record<string, Card> = {}
    let readSeq = this.canvasSeq
    try {
      const read = await this.client.canvas(this.projectId)
      this.heard(read.seq)
      readSeq = read.seq
      cards = read.cards
    } catch (error) {
      if (!lost(error)) say("Couldn't read where things sit", error)
    }
    const rows = this.rows!
    const state = this.state!
    const named = new Set([...state.assets, ...(state.entities ?? [])].map((element) => element.name))
    const wanted = elements.filter(({ name }) => !this.views.has(name) && named.has(name))
    const found = wanted.filter(({ name }) => cards[name])
    for (const { name } of found) rows.set(name, cards[name])
    this.addNodes(found.map((element) => ({ element, row: cards[element.name] })))
    const left = wanted.filter(({ name }) => !cards[name])
    if (!left.length) return
    // Opened here for the first time, with nothing placed yet: its whole new layout is what to look at. Anything after that arrives where someone's looking.
    const nothingYet = !this.ids.size
    const homes = this.homes(left)
    for (const [name, row] of homes) rows.set(name, row)
    this.addNodes(left.map((element) => ({ element, row: homes.get(element.name)! })))
    // Each where there's room, if it's still without a row: one another writer gave it meanwhile stands.
    for (const [name, row] of homes) this.move(name, row, null, this.gesture, readSeq)
    if (nothingYet) this.onPlaced?.()
  }

  private homes(left: Placed[]): Map<string, Card> {
    const boxes = new Map<string, Box>()
    for (const [name, id] of this.ids) {
      // A node just added isn't in the flow until it next draws; its row is.
      const node = this.flow?.getNode(id)
      const row = this.rows?.get(name)
      const view = this.views.get(name)
      if ((!node && !row) || !view) continue
      const pose = this.poseOf(name)
      if (pose) boxes.set(name, this.laid(name, pose))
    }
    const sizes = new Map(left.map((element) => {
      const view = viewOf(element)
      const w = widthFor(view)
      return [element.name, { w, h: heightFor(view, w) + underOf(view, chipsOf(view)) }]
    }))
    const names = left.map((element) => element.name)
    return homesFor(names, readsOf(this.state!), boxes, (name) => sizes.get(name)!, boxes.size ? this.onScreen() : undefined)
  }

  /** The canvas as far as it shows, clear of the top bar and the tools. */
  private onScreen(): Box | undefined {
    const seen = this.flow?.getViewport()
    if (!seen) return undefined
    const left = (document.querySelector(".tools")?.getBoundingClientRect().right ?? 0) + CLEAR_OF_FRAME
    const top = (document.querySelector(".top")?.getBoundingClientRect().bottom ?? 0) + CLEAR_OF_FRAME
    return { x: (left - seen.x) / seen.zoom, y: (top - seen.y) / seen.zoom, w: (innerWidth - left) / seen.zoom, h: (innerHeight - top) / seen.zoom }
  }

  private addNodes(items: { element: Placed; row: Card }[]) {
    if (!items.length) return
    const nodes = items.map(({ element, row }) => {
      const making = element.asset?.making ?? []
      // A name a box had before it was renamed, now another's.
      this.aliases.delete(element.name)
      // Its card may be placed after what its run said came back.
      this.views.set(element.name, { ...viewOf(element), landing: this.landings.delete(element.name) || undefined, run: making.length ? this.runs.get(Math.min(...making)) : undefined })
      for (const number of making) this.owners.set(number, element.name)
      return this.node(element.name, row, this.selected.has(element.name))
    })
    this.flow?.addNodes(nodes)
    this.followAll()
  }

  private node(name: string, row: Card, selected = false, id = `e${++this.nextId}`): ElementNode {
    this.ids.set(name, id)
    this.namesById.set(id, name)
    // Drawn again, a box keeps what Calcifer calls it: a rename on its way hasn't reached it.
    if (!this.served.has(id)) this.served.set(id, name)
    const view = this.views.get(name)!
    const width = row.w ?? widthFor(view)
    this.rememberPose(name, { ...row, w: width })
    const height = heightFor(view, width)
    this.shapes.set(name, this.shapeOf(name, width, height))
    return {
      id,
      type: view.entity ? "entity" : "element",
      position: { x: row.x, y: row.y },
      data: { name, width },
      width,
      // Its size from the row and the lock: a culled node never gets measured, and React Flow's fit leaves out what isn't.
      measured: { width, height },
      selected,
    }
  }

  /** What's on it changed, in one write: the room it takes changes with it. */
  putOn(name: string, fields: Record<string, unknown>) {
    void this.setFields(name, fields)
    this.reshape(name)
  }

  /** Its size, unmeasured, for a fit and for culling: the node hears only when it moved. */
  private reshape(name: string) {
    const key = this.resolveName(name)
    const id = this.ids.get(key)
    const view = this.views.get(key)
    const node = id ? this.flow?.getNode(id) : undefined
    if (!id || !view || !node) return
    const width = this.poseOf(key)?.w ?? widthFor(view)
    const height = heightFor(view, width)
    const shape = this.shapeOf(key, width, height)
    if (this.shapes.get(key) === shape) return
    this.shapes.set(key, shape)
    this.paintPose(key)
    this.unsettle(key, true)
  }

  // Keeping clear: no box lies on another

  /** What a box takes on the canvas, as one key: the same from its first node on, so only a real change moves anything. */
  private shapeOf(name: string, width: number, height: number) {
    return `${width}:${height}:${this.stacked(name)}`
  }

  /** How many are in the stack under it: its tool and what's on that. Read once per view, as every box's is read wherever one moves. */
  stacked(name: string): number {
    const view = this.get(name)
    if (!view) return 0
    let held = this.stacks.get(view)
    if (held === undefined) {
      held = chipsOf(view)
      this.stacks.set(view, held)
    }
    return held
  }

  /** A box and the room it keeps under it, which nothing else lies on: what hangs from it, and what the name of a box below rises into. */
  private laid(name: string, box: Box): Box {
    const view = this.get(name)
    return view ? { ...box, h: box.h + underOf(view, this.stacked(name)) } : box
  }

  /** Where a block of boxes goes to lie on nothing: there, with what it lies on pushed aside under `edit`, unless something that can't be is in the way. */
  clearFor(box: Box, edit: string): { x: number; y: number } {
    const field = this.field()
    const fitted = fit(box, field, { pushes: this.pushable(new Set()) })
    this.gesture = edit
    this.push(field, fitted.pushed)
    this.gesture = null
    return fitted
  }

  /** Its place and size, from what every tab holds alike, and the room it keeps under it unless `bare`. */
  boxOf(name: string, bare = false): Box | null {
    const id = this.ids.get(name)
    const view = this.views.get(name)
    const node = id ? this.flow?.getNode(id) : undefined
    const row = this.rows?.get(name)
    if ((!node && !row) || !view) return null
    // A row is what the page last said of it, and a node may not have heard yet.
    const box = this.poseOf(name)
    if (!box) return null
    return bare ? box : this.laid(name, box)
  }

  private field(besides?: Set<string>, bare = false): Field {
    const field = new Field()
    for (const name of this.ids.keys()) {
      const box = besides?.has(name) ? null : this.boxOf(name, bare)
      if (box) field.add(name, box)
    }
    return field
  }

  /** Its height at a width, stack and all, when what it shows keeps its shape as it narrows: a box that may shrink to fit. */
  private tallOf(name: string): ((w: number) => number) | undefined {
    const view = this.views.get(name)
    if (!view) return undefined
    const tall = (w: number) => this.laid(name, { x: 0, y: 0, w, h: heightFor(view, w) }).h
    return tall(NARROWEST) < tall(NARROWEST * 2) ? tall : undefined
  }

  /** What may be pushed aside: not what settles with it, what a hand holds, the box open where it stands, nor what's still being made. */
  private pushable(besides: Set<string>) {
    return (name: string) => {
      const view = this.views.get(name)
      return Boolean(view && this.ids.has(name)) && !besides.has(name) && !this.dragging.has(name) && name !== this.pulling?.name && name !== this.inPlace?.name && !view!.placing && view!.pending !== NOT_YET
    }
  }

  /** Slides there, briefly. */
  private glide(name: string, at: { x: number; y: number; w?: number | null }) {
    const id = this.ids.get(name)
    if (!id) return
    const was = this.position(name)
    this.rememberPose(name, { ...was, x: at.x, y: at.y, ...(at.w == null ? {} : { w: at.w }) })
    clearTimeout(this.gliding.get(id))
    this.flow?.updateNode(id, { className: "glide" })
    this.paintPose(name)
    this.gliding.set(id, setTimeout(() => this.flow?.updateNode(id, { className: undefined }), GLIDE_MS))
  }

  /** Slides to `to`, at its width, and that's its row: a shrink sticks as a pulled corner does. */
  private slide(name: string, to: Box) {
    const id = this.ids.get(name)
    if (!id) return
    const row = { x: Math.round(to.x), y: Math.round(to.y), w: Math.round(to.w) }
    const resized = this.poseOf(name)?.w !== row.w
    this.glide(name, resized ? row : { x: row.x, y: row.y })
    if (resized) this.reshape(name)
    if (this.views.get(name)?.placing) this.rows?.set(name, row)
    else this.move(name, row, this.rows?.get(name) ?? null)
  }

  /** Each slides where it's pushed, its row the page's own doing, from where it was. */
  private push(field: Field, pushed: Map<string, Box>) {
    for (const [name, to] of pushed) {
      field.add(name, to)
      this.slide(name, to)
    }
  }

  /** It gives way: it stays and pushes aside what it lies on, or moves or shrinks, as `give` says. Where it is now. */
  private giveWay(name: string, field: Field, box: Box, give: Give): Box {
    const { pushed, ...fitted } = fit(box, field, give)
    this.push(field, pushed)
    if (fitted.x === box.x && fitted.y === box.y && fitted.w === box.w) return box
    this.slide(name, fitted)
    return fitted
  }

  private unsettle(name: string, sized = false) {
    if (!this.seen) return
    if (!this.unsettled.size) queueMicrotask(() => this.settle())
    this.unsettled.set(name, sized || Boolean(this.unsettled.get(name)))
  }

  /**
   * What arrived lying on something gives way, by name order, so every tab does the same: it moves or shrinks. What changed its size
   * keeps it and stays, and pushes aside what it lies on. What's held in a drag, or by its corner, waits to be let go.
   */
  private settle() {
    const batch = [...this.unsettled].filter(([name]) => this.ids.has(name) && !this.dragging.has(name) && name !== this.pulling?.name).sort(([a], [b]) => (a < b ? -1 : 1))
    this.unsettled.clear()
    if (!batch.length) return
    const names = new Set(batch.map(([name]) => name))
    const field = this.field(names)
    for (const [name, sized] of batch) {
      const box = this.boxOf(name)
      if (box) field.add(name, this.giveWay(name, field, box, sized ? { pushes: this.pushable(names) } : { tall: this.tallOf(name) }))
    }
  }

  /**
   * Opened: what lies on something comes apart in the smallest moves, the later name moving, and that's saved. Only where things sit changes, so nothing goes out of date.
   * Only boxes themselves: the room each keeps around it is for placing and settling, so a project laid out closer than that stays as it is.
   */
  private separate() {
    const field = this.field(undefined, true)
    for (const name of [...this.ids.keys()].sort()) {
      const box = this.boxOf(name, true)
      if (!box) continue
      field.remove(name)
      if ([...field.under(box).keys()].some((other) => other < name)) field.add(name, this.giveWay(name, field, box, {}))
      else field.add(name, box)
    }
  }

  /** What's selected now. */
  select(names: string[]) {
    this.selected = new Set(names)
  }

  // Looking at versions, and asking for them

  /** Looks at version `number`, or with none its state again. Nothing leaves this page. */
  view(name: string, number?: number) {
    this.patch(name, { viewed: number })
    this.reshape(name)
  }

  /** Looks at take `number`, and the first `<self.N>` its words edit becomes that take. */
  pickTake(name: string, number: number) {
    const asset = this.get(name)?.asset
    if (!asset) return
    this.view(name, number === asset.version ? undefined : number)
    if (!asset.tool || asset.timeline) return
    const field = this.promptOf(asset.tool)
    const words = this.valueOf(name, field)
    if (typeof words !== "string") return
    const next = words.replace(/<(self)\.\d+>/i, (_, self) => `<${self}.${number}>`)
    if (next !== words) this.setDraft(name, field, next)
  }

  /** Looks at a take still being made: the run picked, or with none the next one asked. Its tile is picked, its progress on the frame. */
  watchTake(name: string, run?: number) {
    const asset = this.get(name)?.asset
    if (!asset) return
    const runs = [...(asset.making ?? []), ...(asset.prefixes ?? []).map((held) => held.run ?? 0)]
    this.patch(name, { hereWords: undefined, viewed: undefined, watchRun: run, watchFrom: run === undefined ? Math.max(0, ...runs) : undefined })
  }

  seek(name: string, at: number) {
    const held = this.get(name)
    if (held) this.patch(name, { seek: { at, ask: (held.seek?.ask ?? 0) + 1 } })
  }

  /**
   * A new take or version: the box says Queued now, and anything typed goes first. What Calcifer runs itself needs no machine; with none up for the rest, it asks
   * for one and goes when one is. Its key is made here, once, so however often this is tried it asks one render.
   */
  run(name: string, also: { pickup?: Pickup } = {}, here = false) {
    const key = this.resolveName(name)
    const view = this.views.get(key)
    const asset = view?.asset
    if (!asset || view.conflict) return
    const on = here || madeHere(asset.tool) ? ["local"] : this.machines()
    const saved = this.save(key)
    if (!on.length) {
      this.patch(key, { waiting: also, asking: undefined })
      this.askMachine()
      return
    }
    // Queued from the click on, as any render, until Calcifer says whether to ask first.
    if (!here && !madeHere(asset.tool) && onlyHere(on)) {
      this.patch(key, { waiting: undefined, asking: undefined, asked: NOT_YET })
      void this.askFirst(key, also, saved)
      return
    }
    // From an image tool's words, the box looks at the take being made, and its + goes back to them. Not before it goes: what it waits
    // for or asks first is said under its words.
    if (view.here && view.hereWords && paints(view, this.tools.get())) this.watchTake(key)
    this.patch(key, { waiting: undefined, asking: undefined, scrub: undefined, pickingUp: undefined, glimpse: undefined, asked: NOT_YET })
    const id = this.ids.get(key)
    const settle = (asked?: number) => this.patchId(id, { asked })
    const once = `take-${crypto.randomUUID()}`
    this.act(key, {
      send: async (served, write) => ((await saved) ? this.client.render(this.projectId!, [served], on, once, also, write.once) : null),
      done: (answer) => {
        settle(answer ? nextProjectOrder() : undefined)
        this.readAfter([id])
      },
      refused: (error) => {
        settle()
        say(`Couldn't render ${name}`, error)
      },
    })
  }

  /** How this computer would do it, by Calcifer, once what's typed is in: poorly, and nothing said for that, the box asks; else it renders. */
  private async askFirst(key: string, also: { pickup?: Pickup }, saved: Promise<boolean>) {
    const id = this.ids.get(key)
    if (!id) return
    if (!(await saved) || !this.projectId) return this.patchId(id, { asked: undefined })
    const fit = await this.client.fit(this.projectId, [this.served.get(id) ?? key]).catch(() => null)
    const name = this.namesById.get(id)
    if (!name) return
    if (unsaid(fit)) this.patchId(id, { asked: undefined, asking: { fit, also } })
    else this.run(name, also, true)
  }

  /** Render here: kept for its tools on this computer, so it isn't asked again, and it renders. */
  renderHere(name: string) {
    const key = this.resolveName(name)
    const asking = this.views.get(key)?.asking
    this.act(key, { send: (served) => this.client.renderHere(this.projectId!, [served]), refused: (error) => say("Couldn't keep that", error) })
    this.run(key, asking?.also, true)
  }

  /** Rent a GPU: Rent opens with one that holds it, and the box waits for a machine; nothing renders here. */
  rentInstead(name: string) {
    const asking = this.get(name)?.asking
    this.patch(name, { asking: undefined, waiting: asking?.also ?? {} })
    this.rent(asking?.fit.rent)
  }

  /** A machine came up: each render that waited for one goes. */
  machineUp() {
    for (const [name, view] of this.views) if (view.waiting) this.run(name, view.waiting)
  }

  /** Render all. What's approved stays. */
  runAll(here = false) {
    const on = here ? ["local"] : this.machines()
    const id = this.projectId
    if (!id) return
    this.allAsking.set(null)
    if (!on.length) return this.askMachine()
    const saved = [...this.views.keys()].map((name) => this.save(name))
    const going = [...this.views].filter(([, view]) => view.asset?.tool && !madeHere(view.asset.tool) && !view.asset.pinned).map(([name]) => this.ids.get(name))
    for (const box of going) this.patchId(box, { asked: NOT_YET })
    const settle = (asked?: number) => going.forEach((box) => this.patchId(box, { asked }))
    if (!here && onlyHere(on)) return void this.askAllFirst(id, saved, settle)
    const once = `all-${crypto.randomUUID()}`
    this.outbox.add({
      send: async (signal) => ((await Promise.all(saved)).every(Boolean) ? this.client.render(id, ["all"], on, once, {}, signal) : null),
      done: (answer) => {
        settle(answer ? nextProjectOrder() : undefined)
        this.readAfter(going)
      },
      refused: (error) => {
        settle()
        say("Couldn't render them", error)
      },
    })
  }

  /** Render all's question, asked once on its control for every tool of it this computer would do poorly; else it renders. */
  private async askAllFirst(id: string, saved: Promise<boolean>[], settle: () => void) {
    if (!(await Promise.all(saved)).every(Boolean)) return settle()
    const fit = await this.client.fit(id, ["all"]).catch(() => null)
    if (this.projectId !== id) return
    if (!unsaid(fit)) return this.runAll(true)
    settle()
    this.allAsking.set({ fit })
  }

  /** Render here, for all of them: kept for each tool on this computer, and they render. */
  renderAllHere() {
    const asking = this.allAsking.get()
    const id = this.projectId
    if (!asking || !id) return
    this.outbox.add({ send: () => this.client.renderHere(id, ["all"]), refused: (error) => say("Couldn't keep that", error) })
    this.runAll(true)
  }

  /** Rent a GPU, for all of them: Rent opens with one that holds them all, and nothing renders. */
  rentForAll() {
    const asking = this.allAsking.get()
    this.allAsking.set(null)
    this.rent(asking?.fit.rent)
  }

  /**
   * What holds a box's look until a read begun after the answer has to get that read: the write's own event can reach a read that began first, and then there's no
   * other. One read of these, whatever else is said.
   */
  private readAfter(boxes: (string | undefined)[]) {
    const names = boxes.flatMap((id) => this.served.get(id ?? "") ?? [])
    if (names.length) this.readAgain(names)
  }

  private patchId(id: string | undefined, change: Partial<ElementView>) {
    const name = id && this.namesById.get(id)
    if (name) this.patch(name, change)
  }

  /** `row` shown over what `key`'s row says, until a read begun after `ask.until` says for itself, over and under what else is asked of it. What takes it off. */
  private overlay(key: string, row: Partial<Asset>): { ask: Asking; drop(): void } {
    const view = this.views.get(key)
    const was = view?.asset
    const ask: Asking = { row, until: NOT_YET }
    const id = this.ids.get(key)
    if (view && was) this.patch(key, { over: [...(view.over ?? []), ask], asset: { ...was, ...row } })
    const drop = () => {
      const held = this.views.get(this.namesById.get(id ?? "") ?? "")
      if (!held?.over?.includes(ask) || !was) return
      const left = held.over.filter((other) => other !== ask)
      const read = this.state?.assets.find((asset) => asset.name === this.served.get(id ?? ""))
      this.patch(held.name, { over: left.length ? left : undefined, asset: asked(read ?? was, left) })
      this.reshape(held.name)
    }
    return { ask, drop }
  }

  /** `row` over what its row says, now, until a read begun after the answer says for itself, as `overlay`, and asked of Calcifer under `edit`. Refused, it goes, and the rest stays. */
  private over<T extends { stale?: string[]; etag?: string | null }>(name: string, row: Partial<Asset>, send: (served: string, write: Write) => Promise<T>, what: string, edit: string | undefined, then?: { done?(answer: T): void; refused?(): void }) {
    const key = this.resolveName(name)
    const view = this.views.get(key)
    if (!view?.asset) return
    const was = view.asset
    const id = this.ids.get(key)
    const now = () => this.views.get(this.namesById.get(id ?? "") ?? "")
    const { ask, drop } = this.overlay(key, row)
    this.act(key, {
      send,
      edit,
      done: (answer) => {
        ask.until = nextProjectOrder()
        this.readAfter([id])
        // A write of its own: what a typed field began from moves with it, so it's no conflict.
        const held = now()
        if (held && answer.etag) {
          this.own.set(held.name, (this.own.get(held.name) ?? new Set()).add(answer.etag))
          this.patch(held.name, { known: answer.etag, ...(held.base === was.etag ? { base: held.drafts || held.writing ? answer.etag : undefined } : {}) })
        }
        if (answer.stale?.length) say(`That puts ${answer.stale.join(", ")} out of date`)
        then?.done?.(answer)
      },
      refused: (error) => {
        drop()
        if (edit) this.history.forget(edit)
        say(`Couldn't ${what} ${name}`, error)
        then?.refused?.()
      },
    })
  }

  /** One take or version off its box, at once, with no dialog: the strip closes up, an approved one's box stands on the newest, and a note offers Undo for a while. */
  discardTake(name: string, number: number) {
    const key = this.resolveName(name)
    const asset = this.views.get(key)?.asset
    if (!asset?.prefixes?.some((held) => held.number === number)) return
    const edit = this.step(`Discarded ${asset.noun} ${number} of ${key}`, [key])
    let note = 0
    this.dropTake(key, number, edit, () => dismiss(note))
    const stands = asset.approved === number ? ` It was approved: ${name} now shows its newest.` : ""
    const readers = (this.state?.assets ?? []).filter((other) => other.name !== key && other.reads?.some((read) => read.name === key && read.version === number)).map((other) => other.name)
    const read = readers.length ? ` ${readers.join(", ")} ${readers.length === 1 ? "reads" : "read"} it, and can't render until it's back.` : ""
    note = this.offer(edit, `Discarded ${asset.noun} ${number} of ${name}.${stands}${read}`)
  }

  /** What take or version `number` of `name` says of its file, whole, at once: orbit viewing on for any video, or off. With no number, the
   *  file a box with no version shows, which Calcifer numbers first. A note offers Undo. */
  setMetadata(name: string, number: number | undefined, metadata: Record<string, unknown>, words: string) {
    const key = this.resolveName(name)
    const asset = this.views.get(key)?.asset
    if (!asset) return
    const prefixes = asset.prefixes?.map((version) => (version.number === number && version.main ? { ...version, main: { ...version.main, metadata } } : version))
    const edit = this.step(words, [key])
    this.over(key, number === undefined || !prefixes ? {} : { prefixes }, (served, write) => this.client.patchVersion(this.projectId!, served, number ?? 0, metadata, write).then(() => ({})), `change ${asset.noun} ${number ?? ""} of`, edit)
    this.offer(edit, words)
  }

  /** Take `number` off its box, in its turn, under `edit`: what's approved going leaves it following its inputs. */
  private dropTake(key: string, number: number, edit: string, refused?: () => void) {
    const asset = this.views.get(key)?.asset
    const left = asset?.prefixes?.filter((held) => held.number !== number)
    if (!asset || !left) return
    const approved = asset.approved === number
    const gone: Partial<Asset> = { prefixes: left, versions: left.length, version: asset.version === number ? left.at(-1)?.number : asset.version, ...(approved ? { approved: undefined, pinned: false } : {}) }
    this.patch(key, { viewed: this.views.get(key)?.viewed === number ? undefined : this.views.get(key)?.viewed })
    this.over(key, gone, (served, write) => this.client.discardVersion(this.projectId!, served, number, write), `discard ${asset.noun} ${number} of`, edit, { refused })
  }

  /**
   * A take carried out of `from` onto the canvas is moved, not copied: its new box `to` holds it, it leaves `from` as a discard does (an approved
   * one leaving `from` following its inputs), and words that named it, `<from.N>` or `from`'s own `<self.N>`, name `to`. All of it is the one edit
   * that made `to`, which a note's Undo takes back. Called once `to` is made, its writes follow in line.
   */
  movedOut(from: string, number: number, to: string, edit: string) {
    const key = this.resolveName(from)
    const asset = this.views.get(key)?.asset
    if (!asset?.prefixes?.some((held) => held.number === number)) return
    const named = (mention: string, reader: string) => {
      const version = versionOf(mention)
      return version?.number === number && (nameKey(version.name) === nameKey(key) || (reader === key && isSelf(version.name)))
    }
    const renamed = (value: unknown, reader: string): unknown =>
      typeof value === "string"
        ? value.replace(/<([^<>\s]+)>/g, (whole, mention: string) => (named(mention, reader) ? `<${to}>` : whole))
        : Array.isArray(value)
          ? value.map((item) => renamed(item, reader))
          : value && typeof value === "object"
            ? Object.fromEntries(Object.entries(value).map(([field, item]) => [field, renamed(item, reader)]))
            : value
    const readers: { name: string; next: Record<string, unknown> }[] = []
    for (const [name, view] of this.views) {
      if (!view.asset || name === to) continue
      const fields = valuesOf(view)
      const next = Object.fromEntries(Object.entries(fields).flatMap(([field, value]) => (JSON.stringify(renamed(value, name)) === JSON.stringify(value) ? [] : [[field, renamed(value, name)]])))
      if (Object.keys(next).length) readers.push({ name, next })
    }
    this.joinStep(edit, [key, ...readers.map((one) => one.name)])
    let note = 0
    this.dropTake(key, number, edit, () => dismiss(note))
    for (const one of readers) void this.write(one.name, one.next, `set ${one.name}'s words`, { edit })
    const stands = asset.pinned && asset.approved === number ? ` It was approved: ${from} now follows its inputs.` : ""
    const read = readers.length ? ` ${readers.map((one) => one.name).join(", ")} ${readers.length === 1 ? "names" : "name"} ${to} now.` : ""
    note = this.offer(edit, `Moved ${asset.noun} ${number} of ${from} out as ${to}.${stands}${read}`)
  }
  /** Whether a picture dropped here joins it: a box of pictures with no tool. */
  joins(name: string): boolean {
    const asset = this.get(name)?.asset
    return Boolean(asset && !asset.tool && !asset.timeline && ["image", "video", "audio"].includes(asset.kind))
  }

  /** A slot in its row while a file is on its way to it. What it answers says that's over. */
  joining(name: string): () => void {
    const count = (by: number) => this.patch(name, { joining: (this.get(name)?.joining ?? 0) + by || undefined })
    count(1)
    return () => count(-1)
  }

  /**
   * A picture is `name`'s next version, there at once, a note offering Undo. `file` needs a path to show before the project holds it. `source` is what Calcifer finds
   * it by. `merged`: the edit of a box merging in, so on an approved box it's the one approved, and the merge's own note says it.
   */
  joinVersion(name: string, file: Pick<MainFile, "sha256" | "field"> & Partial<MainFile>, source: string, what: string, merged?: string): boolean {
    const key = this.resolveName(name)
    const asset = this.views.get(key)?.asset
    if (!asset) return false
    const had = asset.prefixes ?? []
    const same = had.find((held) => held.main?.sha256 === file.sha256)
    if (same || asset.sha256 === file.sha256) {
      say(`${name} has that picture already`)
      return false
    }
    const first: VersionPrefix[] = had.length || !asset.sha256 ? had : [{ number: 1, prefix: asset.sha256.slice(0, 12) }]
    const number = Math.max(0, ...first.map((held) => held.number)) + 1
    const main = file.path === undefined ? undefined : { output: file.field, path: file.path, ...file }
    const prefixes = [...first, { number, prefix: file.sha256.slice(0, 12), main }]
    const was = asset.pinned ? (asset.approved ?? 1) : undefined
    const approves = merged !== undefined && was !== undefined
    const stands = was !== undefined && !approves ? was : number
    const edit = merged ?? this.step(`Added ${what} to ${key}`, [key])
    let note = 0
    this.over<VersionMoved>(key, { prefixes, versions: prefixes.length, version: stands, ...(was !== undefined ? { approved: stands } : {}) }, (served, write) => this.client.addVersion(this.projectId!, served, source, write), "add to", edit, {
      done: (answer) => {
        if (answer.already) dismiss(note)
        if (approves) this.pin(key, answer.number, edit)
      },
      refused: () => dismiss(note),
    })
    if (merged === undefined) note = this.offer(edit, `${what} joined ${name}`)
    return true
  }

  /** A take let go on a box: one with no tool takes it as a version, anything else with words mentions it. Whether anything did. */
  takePicture(name: string, file: CarriedFile): boolean {
    const key = this.resolveName(name)
    const asset = this.views.get(key)?.asset
    if (!asset || asset.timeline || madeHere(asset.tool)) return false
    if (!asset.tool && asset.kind !== file.field) {
      say(`${key} is a ${asset.kind} node and cannot take ${file.field}`)
      return true
    }
    const own = file.from !== undefined && nameKey(file.from) === nameKey(key)
    // Another box's take on one that picks up: from there, at a frame chosen on it.
    if (!own && file.from && file.number && file.field === "video" && this.tools.get().get(asset.tool ?? "")?.pickup) {
      this.patch(key, { pickingUp: { take: mentionFrom(key, file.from, file.number), from: file.from, number: file.number }, scrub: 0 })
      this.openHere(key)
      return true
    }
    if (this.joins(key)) {
      if (!own) this.joinVersion(key, { sha256: file.sha256, field: file.field, path: "" }, file.sha256, file.name)
      return true
    }
    this.takeInto(key, [file.from && file.number ? mentionFrom(key, file.from, file.number) : file.name])
    return true
  }

  // What makes a box

  /** `tool` goes on it, or takes the place of the one there. What it can't take is said, with Undo. On files of another kind it's a new box beside
   *  them that mentions them (tools §9). `operation`: the palette's row it came from. */
  putTool(name: string, tool: Tool, operation?: Operation) {
    const key = this.resolveName(name)
    const view = this.views.get(key)
    if (!view?.asset || view.asset.tool === tool.id) return
    if (goesBeside(tool, view.asset)) return this.toolBeside(key, view.asset, tool, view.viewed, operation)
    if (!toolFitsNode(tool, view.asset)) return void say(`${toolName(tool)} makes ${tool.makes}; it cannot attach to this ${isVideoEdit(view.asset) ? "Video Edit" : view.asset.kind} node`)
    void this.save(key)
    const step = goingOn(view.asset, { ...view.asset.fields, ...view.sent, ...view.drafts }, this.tools.get().get(view.asset.tool ?? ""), tool, view.viewed)
    this.retool(key, step, step.lost.length ? `${name} runs ${toolName(tool)} now, which can't take ${step.lost.join(" or ")}` : undefined)
    if (operation) this.patch(key, { operation })
    // On pictures, an image tool opens on its next take's words; anything else opens as it is.
    if (tool.makes !== "image" || !holdsPictures(view.asset)) return this.openHere(key)
    if (this.inPlace?.name === key) return this.writeNext(key, true)
    this.writeOnOpen = key
    this.openHere(key)
  }

  /** A box of `tool` beside `key`, mentioning the version looked at there, else the box, where the tool reads that kind, else first in its prompt. */
  private toolBeside(key: string, asset: Asset, tool: Tool, looked?: number, operation?: Operation) {
    const box = this.boxOf(key, true)
    if (!box) return
    const mention = `<${looked ? `${key}.${looked}` : key}>`
    this.addToolAt(tool, { x: box.x + box.w + BESIDE + CARD_WIDTH / 2, y: box.y + 16 }, startsFrom(tool, asset.kind, {}, mention), operation)
    say(`${toolName(tool)} makes ${tool.makes === "image" ? "an image" : tool.makes}, so it's a new box that reads ${key}`)
  }

  /** Its tool's X leaves a typed container and its words. A note offers Undo for a while. */
  takeToolOff(name: string) {
    const key = this.resolveName(name)
    const view = this.views.get(key)
    if (!view?.asset) return
    if (isVideoEdit(view.asset)) return void say("Video Edit keeps its built-in timeline engine")
    void this.save(key)
    const known = this.tools.get().get(view.asset.tool ?? "")
    const word = known ? toolName(known) : "Its tool"
    const words = holdsPictures(view.asset) ? `Its pictures stay.` : `It stays a ${view.asset.kind} node.`
    this.retool(key, comingOff(view.asset, { ...view.asset.fields, ...view.sent, ...view.drafts }), `${word} is off ${name}. ${words}`)
  }

  private retool(key: string, step: ToolStep, said?: string) {
    const edit = this.step(said ?? `Changed what makes ${key}`, [key])
    let note = 0
    this.over(key, step.row, (served, write) => this.client.patchAsset(this.projectId!, served, { tool: step.tool, fields: step.fields }, undefined, write), "change what makes", edit, {
      done: (answer) => answer.warnings?.forEach((line) => say(line)),
      refused: () => dismiss(note),
    })
    if (!step.row.tool) this.patch(key, { hereWords: undefined })
    this.reshape(key)
    if (said) note = this.offer(edit, said)
  }

  /** The take or version being looked at in `name`, if one is chosen: what Delete means while it's open. */
  lookingAt(name: string): number | undefined {
    return this.get(name)?.viewed
  }

  /** Approves it: this version is what `name` is, until it's unapproved. It turns into that file now, and can flip back. `edit`: what it's part of. */
  pin(name: string, number: number, edit?: string) {
    const key = this.resolveName(name)
    this.patch(key, { hereWords: undefined, viewed: undefined })
    const noun = this.views.get(key)?.asset?.noun ?? "version"
    const by = edit ?? this.step(`Approved ${noun} ${number} of ${key}`, [key])
    this.over(key, { pinned: true, approved: number, version: number }, (served, write) => this.client.approve(this.projectId!, served, String(number), write), "approve", by)
  }

  /** A double tap on a take: it's approved, or if it was, nothing is, and it's still the one looked at. */
  toggleApproved(name: string, number: number) {
    const asset = this.get(name)?.asset
    if (!asset?.pinned || asset.approved !== number) return this.pin(name, number)
    this.unpin(name)
    this.view(name, number)
  }

  unpin(name: string) {
    const key = this.resolveName(name)
    const edit = this.step(`Unapproved ${key}`, [key])
    this.over(key, { pinned: false, approved: undefined }, (served, write) => this.client.unpin(this.projectId!, served, write), "unapprove", edit)
  }

  /** Stops one of its runs, gone from the box at once, and its others go on. */
  cancelRun(name: string, run: number) {
    const making = this.get(name)?.asset?.making
    if (making && this.projectId) {
      const project = this.projectId
      this.over(name, { making: making.filter((number) => number !== run) }, (_, write) => this.client.cancelRun({ project, run }, write.once).then(() => ({})), `cancel run ${run} of`, undefined)
    }
  }

  cancelRender(name: string) {
    this.patch(name, { asked: undefined })
    this.act(name, {
      send: (served, write) => this.client.cancelRender(this.projectId!, [served], write.once),
      refused: (error) => say(`Couldn't cancel ${name}`, error),
    })
  }

  private removeNode(name: string) {
    const id = this.ids.get(name)
    if (this.surface?.id === id) this.surface = null
    this.unsettled.delete(name)
    this.views.delete(name)
    this.poses.delete(name)
    this.ids.delete(name)
    if (id) {
      this.namesById.delete(id)
      this.moving.delete(id)
      this.movingFrom.delete(id)
      this.movingSince.delete(id)
      this.movingEdits.delete(id)
    }
    this.masks.delete(name)
    this.shapes.delete(name)
    if (this.moreOpen.delete(name)) this.onMore?.([...this.moreOpen])
    this.notify(name)
    if (id) this.flow?.setNodes((nodes) => nodes.filter((node) => node.id !== id))
  }

  /** The same node and view under a new name. `ours`: Calcifer doesn't know yet. */
  private moveName(from: string, to: string, ours = false) {
    const view = this.views.get(from)
    const id = this.ids.get(from)
    if (!view || !id) return
    if (!ours) this.served.set(id, to)
    this.views.delete(from)
    this.ids.delete(from)
    if (this.dragging.delete(from)) this.dragging.add(to)
    this.views.set(to, { ...view, name: to })
    this.ids.set(to, id)
    this.namesById.set(id, to)
    const pose = this.poses.get(from)
    this.poses.delete(from)
    if (pose) this.poses.set(to, pose)
    const mask = this.masks.get(from)
    this.masks.delete(from)
    if (mask) this.masks.set(to, mask)
    this.aliases.set(from, to)
    this.aliases.delete(to)
    const shape = this.shapes.get(from)
    this.shapes.delete(from)
    if (shape) this.shapes.set(to, shape)
    if (this.moreOpen.delete(from)) {
      this.moreOpen.add(to)
      this.onMore?.([...this.moreOpen])
    }
    const row = this.rows?.get(from)
    if (row) {
      this.rows!.delete(from)
      this.rows!.set(to, row)
    }
    if (this.inPlace?.name === from) {
      this.inPlace.name = to
      this.hereRenamed(to)
    }
    this.notify(to)
    this.flow?.updateNodeData(id, { name: to })
  }

  // A node's inputs

  /** Whether its More is open: as it was left, box by box. */
  showsMore(name: string): boolean {
    return this.moreOpen.has(this.resolveName(name))
  }

  setMore(name: string, open: boolean) {
    const key = this.resolveName(name)
    if (open) this.moreOpen.add(key)
    else this.moreOpen.delete(key)
    this.patch(key, {})
    this.onMore?.([...this.moreOpen])
  }

  /** What a control shows: what's typed, else what was sent, else what it holds. */
  valueOf(name: string, field: string): unknown {
    return valueIn(this.get(name), field)
  }

  /** What's typed shows at once and writes itself at the first pause, on leaving the text, or as its box shuts. */
  setDraft(name: string, field: string, value: unknown) {
    const view = this.get(name)
    if (!view) return
    this.patch(name, { drafts: { ...view.drafts, [field]: value }, base: view.base ?? view.known ?? view.asset?.etag ?? undefined })
    clearTimeout(this.pauses.get(name))
    this.pauses.set(name, setTimeout(() => void this.save(name), TYPING_PAUSE_MS))
  }

  /** Writes what's typed. Whether it stands: a refusal leaves it typed. */
  save(name: string): Promise<boolean> {
    clearTimeout(this.pauses.get(name))
    this.pauses.delete(name)
    const key = this.resolveName(name)
    const view = this.views.get(key)
    if (!view?.drafts) return Promise.resolve(true)
    // The person says which: leaving the text doesn't.
    if (view.conflict) return Promise.resolve(false)
    return this.write(key, view.drafts, `save ${view.name}`, { typed: true })
  }

  /** The edit a write of `fields` is: `typed`, what goes on being typed in the same fields of the same box is the one it began, else a step of its own. */
  private writeStep(key: string, fields: Record<string, unknown>, typed?: boolean): string {
    const said = Object.keys(fields).sort().join()
    if (typed && this.typing?.name === key && this.typing.fields === said && this.history.latest()?.edit === this.typing.edit) return this.typing.edit
    const edit = this.step(`Set ${Object.keys(fields).join(", ")} on ${key}`, [key])
    this.typing = typed ? { name: key, fields: said, edit } : null
    return edit
  }

  /**
   * `fields`, shown as written now, go in their turn, over what they began from, under `edit` (else a step of their own). Whether they stand: refused, they're
   * typed again, and a file that moved under them is a conflict. `force`: written over it.
   */
  private write(key: string, fields: Record<string, unknown>, what: string, opts: { force?: boolean; edit?: string; typed?: boolean } = {}): Promise<boolean> {
    const view = this.views.get(key)
    const id = this.ids.get(key)
    if (!view?.asset || !id) return Promise.resolve(true)
    const edit = opts.edit ?? this.writeStep(key, fields, opts.typed)
    const drafts = Object.fromEntries(Object.entries(view.drafts ?? {}).filter(([field]) => !(field in fields)))
    this.patch(key, { drafts: Object.keys(drafts).length ? drafts : undefined, sent: { ...view.sent, ...fields }, conflict: undefined, base: view.base ?? view.known ?? view.asset.etag ?? undefined, writing: (view.writing ?? 0) + 1 })
    const now = () => this.views.get(this.namesById.get(id) ?? "")
    return new Promise((settled) => {
      this.act(key, {
        send: (served, write) => this.client.patchAsset(this.projectId!, served, { fields }, opts.force ? undefined : now()?.base, write),
        edit,
        done: (answer) => {
          const held = now()
          if (held) {
            this.wrote(held.name, answer)
            const writing = (now()?.writing ?? 1) - 1
            this.patch(held.name, { writing: writing || undefined, base: now()?.drafts || writing ? now()?.base : undefined })
          }
          settled(true)
        },
        refused: (error) => {
          const held = now()
          if (held) {
            const sent = Object.fromEntries(Object.entries(held.sent ?? {}).filter(([field]) => !(field in fields)))
            const changed = error instanceof ApiError && error.body.code === "asset_changed"
            this.patch(held.name, { drafts: { ...fields, ...held.drafts }, sent, writing: (held.writing ?? 1) - 1 || undefined, conflict: changed || held.conflict })
            if (!changed) say(`Couldn't ${what}`, error)
          }
          this.history.forget(edit)
          settled(false)
        },
      })
    })
  }

  /** A write of its own moves what the drafts began from; anyone else's doesn't, so that still conflicts. */
  private wrote(key: string, answer: AssetPatched) {
    const view = this.views.get(key)
    if (view) this.patch(key, { known: answer.etag, ...(view.base === answer.was ? { base: answer.etag } : {}) })
    this.own.set(key, (this.own.get(key) ?? new Set()).add(answer.etag))
    for (const warning of answer.warnings ?? []) say(warning)
  }

  /** The file changed under what's typed: this is written over it. */
  keepMine(name: string) {
    const key = this.resolveName(name)
    return this.write(key, this.views.get(key)?.drafts ?? {}, `save ${name}`, { force: true })
  }

  /** The file changed under what's typed: what's typed goes. */
  takeTheFiles(name: string) {
    this.patch(name, { drafts: undefined, base: undefined, conflict: undefined })
  }

  /** A slider let go, a choice made: one field, whatever else is typed. */
  setField(name: string, field: string, value: unknown, edit?: string) {
    return this.setFields(name, { [field]: value }, edit)
  }

  /** A write by what keeps its own `etag`, in its turn like any other. */
  patchInTurn(name: string, fields: Record<string, unknown>, read?: string): Promise<AssetPatched> {
    const key = this.resolveName(name)
    const edit = this.step(`Set ${Object.keys(fields).join(", ")} on ${key}`, [key])
    return new Promise((done, refused) => this.act(key, { send: (served, write) => this.client.patchAsset(this.projectId!, served, { fields }, read, write), edit, done, refused }))
  }

  /** What belongs together, in one write. `edit`: what it's part of. */
  setFields(name: string, fields: Record<string, unknown>, edit?: string) {
    return this.write(this.resolveName(name), fields, `set ${name}'s ${Object.keys(fields).join(", ")}`, { edit })
  }

  /** What's typed in one field goes, and what it holds shows again. */
  discard(name: string, field: string) {
    const view = this.get(name)
    if (!view?.drafts) return
    const { [field]: _, ...drafts } = view.drafts
    this.patch(name, { drafts: Object.keys(drafts).length ? drafts : undefined })
  }

  /** What makes it, among those of its kind: its fields stay. */
  setTool(name: string, tool: string) {
    const key = this.resolveName(name)
    if (this.views.get(key)?.asset?.tool === tool) return
    const known = this.tools.get().get(tool)
    const label = known ? toolName(known) : tool.split(/[/\\]/).at(-1)
    const edit = this.step(`Switched ${key} to ${label}`, [key])
    this.over(key, { tool }, (served, write) => this.client.patchAsset(this.projectId!, served, { tool }, undefined, write), "change what makes", edit, { done: (answer) => answer.warnings?.forEach((line) => say(line)) })
    this.reshape(key)
  }

  // Names in a text

  /** Why the box `name`'s tool can't send a mention of `mention`, if it can't. */
  refusalOf(name: string, mention: string): string | null {
    const asset = this.get(name)?.asset
    const read = this.mentionOf(mention, name)
    return read ? refusal(this.tools.get().get(asset?.tool ?? ""), this.get(read.name)) : null
  }

  /** Each name a mention could be, for the `<` list: nodes only, and none its tool can't send. */
  namesLike(typed: string, besides: string): string[] {
    return this.mentionable(typed, besides).filter((name) => !this.refusalOf(besides, name))
  }

  private mentionable(typed: string, besides: string): string[] {
    const version = /^(.+)\.(\d*)$/.exec(typed)
    if (version) {
      const of = isSelf(version[1]) ? besides : this.nameOf(version[1])
      const numbers = numbersOf(this.get(of ?? "")?.asset).filter((number) => String(number).startsWith(version[2]))
      // A box whose own name has the dot goes first, as an exact name wins.
      return [...this.boxesLike(nameKey(typed), besides), ...numbers.reverse().map((number) => `${version[1]}.${number}`)]
    }
    const key = nameKey(typed)
    const own = key && SELF.startsWith(key) ? numbersOf(this.get(besides)?.asset).slice(-1).map((number) => `${SELF}.${number}`) : []
    return [...this.boxesLike(key, besides), ...own]
  }

  private boxesLike(key: string, besides: string): string[] {
    const names = [...this.views.entries()].filter(([name, view]) => name !== besides && !view.placing && !view.entity).map(([name]) => name)
    return names.filter((name) => nameKey(name).startsWith(key)).sort()
  }

  /** The name a mention is, in any case, with `_` and `-` alike. Names differ by more than that, so one written as it's named is that box, found
   *  without reading every box's name: each box's shape asks of each mention at once. */
  nameOf(mention: string): string | null {
    const exact = this.views.get(mention)
    if (exact && !exact.placing) return mention
    const key = nameKey(mention)
    for (const [name, view] of this.views) if (!view.placing && nameKey(name) === key) return name
    return null
  }

  /** What a mention in `asset`'s text names, as far as this page knows: a box, a version of it, and whether that's gone. A box written with a dotted
   *  name is read as its `_` form, as Calcifer reads it (format.rs `dotted_box`): `<kyogre.primordial>` is `kyogre_primordial`, not a take of kyogre. */
  mentionOf(said: string, asset: string): { name: string; number?: number; gone?: boolean } | null {
    const mention = baseOf(said)
    const name = this.nameOf(mention)
    if (name) return { name }
    const version = versionOf(mention)
    const of = version && (this.nameOf(version.name) ?? (isSelf(version.name) ? this.resolveName(asset) : null))
    if (of && version) return { name: of, number: version.number, gone: !numbersOf(this.get(of)?.asset).includes(version.number) }
    const dotted = mention.includes(".") ? this.nameOf(asName(mention)) : null
    return dotted ? { name: dotted } : null
  }

  /** The file a mention means, as this page knows it: a version's, else what the box shows. */
  fileOf(mention: string, asset: string): Shown | null {
    const said = this.mentionOf(mention, asset)
    const view = said && this.get(said.name)
    if (!view) return null
    if (said.number === undefined) return shownOf(view)
    const main = versionIn(view.asset, said.number)?.main
    return main ? { ...main, number: said.number } : null
  }

  /** What a mention is: `image`, `video`, `audio` or `text`. An orbit's frame at an angle is a picture. */
  kindOf(mention: string, asset: string): string | undefined {
    const said = this.mentionOf(mention, asset)
    const view = said && this.get(said.name)
    if (!view) return undefined
    const angled = angleOf(mention)
    if (angled && angled.to === undefined) return "image"
    return this.fileOf(mention, asset)?.field ?? view.asset?.kind
  }

  /** A take carried out of its box, as `asset`'s text would mention it. */
  carriedVersion(asset?: string): string | null {
    const held = carriedFile.get()
    if (!held?.from || !held.number) return null
    return asset === undefined ? held.from : mentionFrom(asset, held.from, held.number)
  }

  /** A text says where its caret takes a name. The first of a box is what a drop on the box means. */
  openCaret(node: string, place: string, insert: (name: string) => void): () => void {
    const key = `${node}:${place}`
    this.carets.set(key, insert)
    return () => void this.carets.delete(key)
  }

  // A drag

  /** Whatever box is under the pointer, with `dragged` taken away. One being worked on takes a drop in the part it's let go on. */
  dropTargetAt(x: number, y: number, dragged: Element[], names: string[] = dragged.flatMap((held) => held.querySelector<HTMLElement>(".card[data-asset]")?.dataset.asset ?? [])): DropTarget | null {
    for (const el of document.elementsFromPoint(x, y)) {
      if (dragged.some((held) => held.contains(el))) continue
      const card = el.closest<HTMLElement>(".card[data-asset]")
      if (!card) continue
      const name = this.resolveName(card.dataset.asset ?? "")
      const view = this.views.get(name)
      // A part of an open box takes what it says it takes (`whyNot`).
      const at = view?.here && el.closest("[data-drop-target]")
      const part = at && this.dropTargets.get(at)
      if (part) {
        this.setHot(name)
        this.overPart = at
        return part
      }
      // What its tool's words don't take is refused, saying why, unless it merges in as one of its own.
      const tool = this.tools.get().get(view?.asset?.tool ?? "")
      const merges = (one: ElementView | undefined) => Boolean(view?.asset && holdsPictures(view.asset) && one?.asset?.kind === view.asset.kind)
      const refused = names.map((one) => this.get(one)).map((one) => (merges(one) ? null : refusal(tool, one))).find(Boolean)
      if (refused) {
        this.setHot(name, refused)
        return { over: () => {}, drop: () => void say(refused) }
      }
      // A box of pictures takes a box of the same kind: it merges in.
      if (view?.asset && holdsPictures(view.asset) && !view.asset.timeline && !madeHere(view.asset.tool)) {
        this.setHot(name)
        return { over: () => {}, drop: (names) => this.mergeBoxes(name, names) }
      }
      // Only a box being made takes anything: what's made, approved or only words is just in the way.
      if (!view || !makes(view) || faceOf(view) !== "edit") break
      this.setHot(name)
      return { over: () => {}, drop: (names) => this.takeInto(name, names) }
    }
    this.setHot(null)
    return null
  }

  private overPart: Element | null = null
  /** What's carried over a part of a box that won't take it, and why: shown on that part until the drag moves off it. */
  readonly refusing = signal<{ el: Element; why: string } | null>(null)

  /** Carried over a point: what's there to take it, and why it won't. */
  offered(x: number, y: number, names: string[], dragged: Element[]): { target: DropTarget | null; why: string | null } {
    this.overPart = null
    const target = this.dropTargetAt(x, y, dragged)
    const why = target?.whyNot?.(names) ?? null
    const el = why ? this.overPart : null
    const now = this.refusing.get()
    if (now?.el !== el || now?.why !== why) this.refusing.set(el && why ? { el, why } : null)
    if (why) this.setHot(null)
    return { target, why }
  }

  /** Boxes let go on a box of pictures: each of its kind merges in, what it shows the newest take, approved if the box was, and it's gone. Any other a tool reads. */
  private mergeBoxes(target: string, names: string[]) {
    const key = this.resolveName(target)
    const kind = this.get(key)?.asset?.kind
    const merging = names.map((one) => this.resolveName(one)).filter((one) => one !== key)
    const edit = this.step(`Merged ${merging.join(", ")} into ${key}`, [key, ...merging])
    const merged: string[] = []
    const rest: string[] = []
    for (const name of merging) {
      const view = this.get(name)
      const file = view?.asset && view.asset.kind === kind && !view.asset.timeline ? shownOf(view) : undefined
      if (file && this.joinVersion(key, { ...file, path: file.path || "" }, file.sha256, name, edit)) merged.push(name)
      else rest.push(name)
    }
    if (merged.length) this.remove(merged, { into: key, edit })
    else this.history.forget(edit)
    if (rest.length && this.get(key)?.asset?.tool) this.takeInto(key, rest)
  }

  /** Names into a box that's shut: its first place for one, after what's there. */
  takeInto(target: string, names: string[]) {
    const key = this.resolveName(target)
    const asset = this.views.get(key)?.asset
    const tool = this.tools.get().get(asset?.tool ?? "")
    const refused = names.map((name) => refusal(tool, this.get(name))).find(Boolean)
    if (refused) say(refused)
    const given = names.map((name) => this.resolveName(name)).filter((name) => name !== key && !refusal(tool, this.get(name)))
    if (!asset || !given.length) return
    const tags = given.map((name) => `<${name}>`)
    const asText = (held: unknown) => [typeof held === "string" ? held.trimEnd() : "", ...tags].filter(Boolean).join(" ")
    const fields = fieldsOf(asset, this.tools.get())
    if (asset.timeline) {
      const clips = asset.timeline.tracks[0]?.clips ?? []
      const taken = given.flatMap((name) => clipOf(this, name, asset.timeline?.fps ?? 24) ?? [])
      if (taken.length < given.length) say("Only video and stills go in an Edit for now")
      if (taken.length) void this.setFields(key, { timeline: wire([...clips, ...taken]) })
      return
    }
    // A mention goes in its first prompt, at the top or in its first row. Only a box with no prompt has a slot filled by a drop on it.
    const isText = (field: { type: string }) => field.type === "prompt" || field.type === "words"
    const text = fields.find(isText)
    if (text) return void this.setFields(key, { [text.field]: asText(this.valueOf(key, text.field)) })
    const group = fields.find((field) => field.type === "group" && field.list && field.members?.some(isText))
    if (group) {
      const words = group.members!.find(isText)!
      const rows = fullRows(group, rowsIn(this.valueOf(key, group.field)))
      return void this.setFields(key, { [group.field]: rows.map((row, i) => (i ? row : { ...row, [words.field]: asText(row[words.field]) })) })
    }
    const slot = fields.find((field) => field.type === "slot" && (field.multiple || !this.valueOf(key, field.field)))
    if (!slot) return
    const wrong = given.find((name) => slot.accept && this.kindOf(name, key) !== slot.accept)
    if (wrong) return void say(`${slot.label} takes ${slot.accept === "image" ? "a picture" : slot.accept === "video" ? "a video" : "a sound"}, and ${wrong} isn't one`)
    const held = this.valueOf(key, slot.field)
    void this.setFields(key, { [slot.field]: slot.multiple ? [...(Array.isArray(held) ? held : []), ...tags] : tags.at(-1) })
  }

  private hot: string | null = null

  setHot(name: string | null, refused?: string) {
    if (this.hot === name && this.get(name ?? "")?.refused === refused) return
    if (this.hot) this.patch(this.hot, { hot: undefined, refused: undefined })
    if (name) this.patch(name, { hot: true, refused })
    this.hot = name
  }

  // Deleting

  /** Gone at once, with no dialog. Files stay: a note offers Undo for a while, and says if something still needs what went. `merged`: the edit of a merge, whose note this says. */
  remove(names: string[], merged?: { into: string; edit: string }) {
    // One taken out or pasted a moment ago may have no row read yet: its delete waits behind its import.
    const going = names.map((name) => this.resolveName(name)).filter((name) => (this.views.get(name)?.asset || this.views.get(name)?.landed) && !this.views.get(name)?.placing)
    if (!going.length) return
    const readers = this.readers(going)
    const what = going.length === 1 ? going[0] : `${going.length} assets`
    const edit = merged?.edit ?? this.step(`Deleted ${what}`, going)
    let note = 0
    for (const name of going) {
      const id = this.ids.get(name)!
      const box: Gone = { name, view: this.views.get(name)!, row: this.position(name) ?? this.rows?.get(name) ?? { x: 0, y: 0 } }
      this.deleting.set(name, NOT_YET)
      this.rows?.delete(name)
      this.removeNode(name)
      this.outbox.add<AssetDeleted>({
        send: (once, action) => this.client.deleteAsset(this.projectId!, this.served.get(id) ?? name, { once, edit, action }),
        done: () => {
          if (this.deleting.has(name)) this.deleting.set(name, nextProjectOrder())
        },
        // Its answer was lost: its first try did it.
        landed: (error) => (error.status === 404 ? { name } : undefined),
        refused: (error) => {
          this.deleting.delete(name)
          this.bringBack(box)
          this.history.forget(edit)
          dismiss(note)
          say(`Couldn't delete ${name}`, error)
        },
      })
    }
    const said = readers.length ? ` ${readers.join(", ")} ${readers.length === 1 ? "reads" : "read"} this and ${readers.length === 1 ? "isn't" : "aren't"} approved yet.` : ""
    note = this.offer(edit, merged ? `${what} merged into ${merged.into}.${said}` : `Deleted ${what}.${said}`)
  }

  /** A box again, as it was, where it was. */
  private bringBack(box: Gone, pending?: number) {
    const { view, name, row } = box
    this.deleting.delete(name)
    this.rows?.set(name, row)
    this.views.set(name, { ...view, pending })
    this.flow?.addNodes(this.node(name, row))
  }

  /** What reads these and has nothing of its own held: it can't run again without them. */
  private readers(names: string[]): string[] {
    const gone = names.map(nameKey)
    const reads = (mention: string) => gone.some((key) => nameKey(mention) === key || nameKey(mention).startsWith(`${key}.`))
    const after = (this.state?.assets ?? []).filter((asset) => !names.includes(asset.name) && !asset.pinned && asset.reads?.some((read) => reads(read.name)))
    return after.map((asset) => asset.name)
  }

  // Undo and redo, of the project's history

  /** One thing a person does: its writes share an edit, which the project's history keeps as one entry, and Undo shows at once what it was. */
  private step(words: string, names: string[], edit: string = crypto.randomUUID()): string {
    const before = this.early.get(edit) ?? []
    this.early.delete(edit)
    this.history.did({ edit, words, before: [...before, ...names.map((name) => this.snap(name)).filter((snap) => !before.some((held) => held.name === snap.name))] })
    return edit
  }

  /** A step for what the placer does: several boxes pasted, a file dropped. Its boxes join it as they're placed. `edit`: the one its drop began. */
  newStep(words: string, names: string[] = [], edit?: string): string {
    return this.step(words, names, edit)
  }

  /** A note saying what `edit` did, its Undo the history's: one that's undone goes. */
  private offer(edit: string, message: string): number {
    if (!this.history.kept.get()) return say(message)
    const note = say(message, undefined, { action: { label: "Undo", run: () => this.undo({ edit }) }, ms: UNDO_MS })
    this.offers.set(edit, note)
    return note
  }

  /** These boxes, as they are now, are what `edit` began from too: kept for its step if that's still to be made. */
  joinStep(edit: string, names: string[]) {
    const before = this.history.stepOf({ edit })?.before ?? this.early.get(edit) ?? []
    if (!this.history.stepOf({ edit })) this.early.set(edit, before)
    for (const name of names) if (!before.some((held) => held.name === this.resolveName(name))) before.push(this.snap(name))
  }

  private snap(name: string): Snap {
    const key = this.resolveName(name)
    const view = this.deleting.has(key) ? undefined : this.views.get(key)
    return { name: key, node: this.ids.get(key), view: view?.placing ? undefined : view, row: this.rows?.get(key) ?? this.position(key) ?? undefined }
  }

  /**
   * Undo: the newest edit of the project that stands, this page's or anyone's, or `target`. What this page did shows undone at once; anyone else's once Calcifer has
   * it. It goes in its turn like any write, so it waits for Calcifer like one. A note says what it took back, with Redo.
   */
  undo(target = this.history.nextUndo()?.target) {
    if (target) this.historyStep(target, true)
  }

  /** Redo: what Undo took back last, or `target`, done again. */
  redo(target = this.history.nextRedo()?.target) {
    if (target) this.historyStep(target, false)
  }

  /** Every edit back to `entry` undone, or every one undone up to it done again: one ask, and what this page did shows so at once. */
  stepTo(entry: number, back: boolean) {
    const id = this.projectId
    if (!id) return
    const taken = back ? this.history.backTo(entry) : this.history.forthTo(entry)
    if (!taken.length) return
    for (const target of taken.filter((held) => held.edit)) this.historyStep(target, back)
    const listed = taken.filter((held) => held.entry !== undefined)
    const shown = listed.map((target) => this.showStep(target, back))
    for (const target of listed) this.history.mark(target, back)
    this.typing = null
    const verb = back ? "undo" : "redo"
    dismiss(this.stepped)
    const note = say(`${back ? "Undone" : "Redone"}: ${taken.length} ${taken.length === 1 ? "edit" : "edits"}`)
    this.stepped = note
    this.outbox.add<HistoryStepped>({
      send: (once, action) => this.client[verb](id, { to: entry }, { once, action }),
      done: (answer) => {
        shown.forEach((one) => one.settle())
        if (answer.stopped) say(`Couldn't ${verb} them all`, new ApiError(409, answer.stopped))
        this.history.read()
      },
      refused: (error) => {
        shown.forEach((one) => one.release())
        for (const target of listed) this.history.unmark(target)
        dismiss(note)
        say(`Couldn't ${verb} them`, error)
        this.history.read()
      },
    })
  }

  private historyStep(target: Target, back: boolean) {
    const id = this.projectId
    if (!id || !this.history.kept.get()) return
    const words = this.history.wordsOf(target)
    const shown = this.showStep(target, back)
    this.history.mark(target, back)
    this.typing = null
    const verb = back ? "undo" : "redo"
    for (const edit of this.history.editsOf(target)) dismiss(this.offers.get(edit) ?? 0)
    dismiss(this.stepped)
    const note = say(`${back ? "Undone" : "Redone"}: ${words}`, undefined, { action: { label: back ? "Redo" : "Undo", run: () => (back ? this.redo(target) : this.undo(target)) }, ms: UNDO_MS })
    this.stepped = note
    this.outbox.add<HistoryStepped>({
      send: (once, action) => this.client[verb](id, target, { once, action }),
      done: () => {
        shown.settle()
        this.history.read()
      },
      refused: (error) => {
        shown.release()
        this.history.unmark(target)
        dismiss(note)
        // An edit of this page's that made nothing: there was nothing to undo.
        if (error instanceof ApiError && error.body.code === "history_unknown" && target.edit) this.history.forget(target.edit)
        else say(`Couldn't ${verb} ${words}`, error)
        this.history.read()
      },
    })
  }

  /** What `target` showed before it (`back`) or after, shown now, if this page made it: what settles it once Calcifer has it, or takes it off. */
  private showStep(target: Target, back: boolean): { settle(): void; release(): void } {
    const step = this.history.stepOf(target)
    if (!step) return { settle: () => {}, release: () => {} }
    if (back) step.after = step.before.map((held) => this.snap((held.node && this.namesById.get(held.node)) || held.name))
    return step.after && !back ? this.showAs(step.after) : back ? this.showAs(step.before) : { settle: () => {}, release: () => {} }
  }

  /**
   * Each box as `snaps` found it, shown now: one that wasn't there goes, one that went is back, the rest as it was made and where it was. What Calcifer answers
   * settles it, as any write's answer does; refused, all of it goes and what Calcifer holds is read again.
   */
  private showAs(snaps: Snap[]): { settle(): void; release(): void } {
    const overlays: { ask: Asking; drop(): void }[] = []
    const back: string[] = []
    const hidden: string[] = []
    const moved: string[] = []
    for (const snap of snaps) {
      const name = (snap.node && this.namesById.get(snap.node)) || snap.name
      const now = this.deleting.has(name) ? undefined : this.views.get(name)
      if (!snap.view) {
        if (!now || now.placing) continue
        this.deleting.set(name, NOT_YET)
        this.rows?.delete(name)
        this.removeNode(name)
        hidden.push(name)
        continue
      }
      if (!now) {
        this.bringBack({ name: snap.name, view: snap.view, row: snap.row ?? { x: 0, y: 0 } }, NOT_YET)
        back.push(snap.name)
        continue
      }
      if (name !== snap.name) {
        this.renames.set(name, snap.name)
        this.moveName(name, snap.name, true)
        const id = this.ids.get(snap.name)
        if (id) back.push(snap.name)
      }
      if (snap.view.asset && now.asset) {
        this.patch(snap.name, { sent: undefined })
        overlays.push(this.overlay(snap.name, rowOf(snap.view.asset)))
        this.reshape(snap.name)
      }
      if (snap.row && !samePlace(snap.row, this.rows?.get(snap.name) ?? snap.row)) {
        this.masks.set(snap.name, snap.row)
        this.rows?.set(snap.name, snap.row)
        this.glide(snap.name, snap.row)
        moved.push(snap.name)
      }
    }
    const names = [...new Set([...snaps.map((snap) => snap.name), ...hidden])]
    return {
      settle: () => {
        const at = nextProjectOrder()
        for (const { ask } of overlays) ask.until = at
        for (const name of hidden) if (this.deleting.has(name)) this.deleting.set(name, at)
        for (const name of back) {
          const id = this.ids.get(name)
          if (id) this.served.set(id, name)
          this.patch(name, { pending: at })
        }
        const there = names.filter((name) => !hidden.includes(name))
        if (there.length) this.readAgain(there)
      },
      release: () => {
        for (const { drop } of overlays) drop()
        for (const name of back) this.patch(name, { pending: nextProjectOrder() })
        for (const name of hidden) this.deleting.delete(name)
        for (const name of moved) this.masks.delete(name)
        this.readAgain(names)
        if (this.projectId) void this.client.canvas(this.projectId).then(({ cards, seq }) => this.applyCanvas(cards, 0, seq), () => {})
      },
    }
  }

  // An import

  /** This project has that file, as far as the page has read. */
  holdsFile(sha256: string): boolean {
    for (const view of this.views.values()) if (view.landed?.sha256 === sha256) return true
    return Boolean(
      this.state?.assets.some((asset) => asset.sha256 === sha256 || Object.values(asset.files ?? {}).some((file) => file.sha256 === sha256)),
    )
  }

  /** A file being placed has a row of its own, under its key, until it lands: its canvas drawn again finds it there. What it pushes aside is part of `edit`. */
  addPlacing(key: string, name: string, placing: Placing, at: Card, opts: { naming: boolean; preview?: string }, edit: string) {
    this.views.set(key, { name, placing, draft: opts.naming ? name : undefined, preview: opts.preview, landing: true })
    const row = this.clearOf(key, at, edit)
    this.rows?.set(key, row)
    this.flow?.addNodes(this.node(key, row))
    this.joinStep(edit, [key])
  }

  /** `at` for a box not on the canvas yet: there, a little smaller at most, with what it lies on pushed aside under `edit`, unless something that can't be is in the way. */
  private clearOf(key: string, at: Card, edit: string): Card {
    const view = this.views.get(key)
    if (!view) return at
    const w = at.w ?? widthFor(view)
    const field = this.field()
    const { pushed, x, y, w: fitted } = fit(this.laid(key, { x: at.x, y: at.y, w, h: heightFor(view, w) }), field, { tall: this.tallOf(key), pushes: this.pushable(new Set([key])) })
    this.gesture = edit
    this.push(field, pushed)
    this.gesture = null
    return { x, y, w: fitted }
  }

  /** The name it will get, as far as this page can tell: placing elements take theirs in the order they were dropped. */
  shownName(key: string): string {
    const taken: string[] = []
    for (const [other, view] of this.views) {
      if (!view.placing) taken.push(other)
    }
    for (const [other, view] of this.views) {
      if (!view.placing) continue
      const name = freeName(asName(other === key ? (view.draft ?? view.name) : view.name), taken)
      if (other === key) return name
      taken.push(name)
    }
    return key
  }

  /** What the open name field would come to, when that isn't what it says: a taken name gets `_2`. */
  nameWillBe(name: string): string | null {
    const key = this.resolveName(name)
    const view = this.views.get(key)
    if (view?.draft === undefined) return null
    const typed = asName(view.draft)
    const will = view.placing ? this.shownName(key) : freeName(typed, this.takenBesides(key))
    return typed && will !== typed ? will : null
  }

  /** Resolves once its name field is shut. */
  async whenNamed(key: string, signal: AbortSignal) {
    while (this.views.get(key)?.draft !== undefined) {
      signal.throwIfAborted()
      await new Promise<void>((done, fail) => {
        const woke = () => {
          this.named.delete(woke)
          signal.removeEventListener("abort", stop)
          done()
        }
        const stop = () => {
          this.named.delete(woke)
          fail(signal.reason)
        }
        this.named.add(woke)
        signal.addEventListener("abort", stop, { once: true })
      })
    }
  }

  /** Asking Calcifer to import: until the answer, a name it adds may be this. */
  beginAdding() {
    this.adding += 1
  }

  endAdding() {
    this.adding -= 1
    if (this.adding === 0 && this.state && this.rows && this.flow) this.reconcile()
  }

  /** The project holds it now. If it moved while being asked, its row moves too. */
  land(key: string, answer: Imported, at: Card, edit?: string) {
    this.patch(key, { placing: undefined, draft: undefined, landed: { sha256: answer.sha256, field: answer.field } })
    this.moveName(key, answer.name)
    this.rows?.set(answer.name, at)
    const now = this.position(answer.name)
    // What it pushed aside goes with its own row, in one write: one entry of the history.
    if (now && (!samePlace(now, at) || this.pushedFor(edit))) this.move(answer.name, now, undefined, edit)
  }

  /** Gone before it landed: unless another box of its drop is still there, what it pushed aside goes back. `edit`: its drop's, once its step is made. */
  cancel(key: string, edit?: string) {
    const id = this.ids.get(key)
    const edits = edit ? [edit] : [...this.early].flatMap(([held, before]) => (before.some((snap) => snap.node === id) ? [held] : []))
    this.rows?.delete(key)
    const view = this.views.get(key)
    view?.placing?.abort.abort()
    if (view?.preview) URL.revokeObjectURL(view.preview)
    this.removeNode(key)
    for (const held of edits) if (!this.alive(held)) this.unpush(held)
  }

  /** A picture says its shape before it's measured: what a take's own size doesn't say. */
  setAspect(name: string, aspect: number) {
    const view = this.get(name)
    if (!view || view.aspect === aspect) return
    this.patch(name, { aspect })
    this.reshape(name)
  }

  /** Its thumbnail is up. */
  showThumb(name: string) {
    const view = this.get(name)
    if (!view?.preview) return
    URL.revokeObjectURL(view.preview)
    this.patch(name, { preview: undefined })
  }

  // Moving

  dragStarted(nodes: ElementNode[]) {
    for (const node of nodes) {
      this.dragging.add(node.data.name)
      this.dragFrom.set(node.data.name, { ...node.position })
    }
  }

  isDragging(name: string): boolean {
    return this.dragging.has(this.resolveName(name))
  }

  /** Let go: each lands where it was let go, a little smaller at most, and what it lies on is pushed aside, in the same step. Only with something that can't be pushed in the way does it move off. */
  dragStopped(nodes: ElementNode[]) {
    const names = nodes.map((node) => node.data.name)
    const held = new Set(names)
    const field = this.field(held)
    const moved = new Map(nodes.map((node) => [node.data.name, this.draggedRow(node)]))
    const boxes = names.filter((name) => this.views.get(name) && !this.views.get(name)?.placing && !samePlace(moved.get(name)!, this.rows?.get(name) ?? moved.get(name)!))
    this.gesture = boxes.length ? this.step(`Moved ${boxes.length > 3 ? `${boxes.length} boxes` : boxes.join(", ")}`, boxes) : null
    for (const node of [...nodes].sort((a, b) => (a.data.name < b.data.name ? -1 : 1))) {
      const name = node.data.name
      this.dragging.delete(name)
      this.dragFrom.delete(name)
      const view = this.views.get(name)
      if (!view) continue
      const row = this.draggedRow(node)
      const box = this.laid(name, { x: row.x, y: row.y, w: row.w ?? CARD_WIDTH, h: heightFor(view, row.w ?? CARD_WIDTH) })
      if (view.placing) this.rows?.set(name, row)
      else this.move(name, row)
      if (this.surface?.id === node.id) this.paintPose(name)
      field.add(name, this.giveWay(name, field, box, { tall: this.tallOf(name), pushes: this.pushable(held) }))
    }
    this.gesture = null
  }

  /** They were dropped into a box: each goes back where it started, and no move is saved. */
  dragDropped(nodes: ElementNode[]) {
    for (const node of nodes) {
      const name = node.data.name
      const from = this.dragFrom.get(name)
      const id = this.ids.get(name)
      this.dragging.delete(name)
      this.dragFrom.delete(name)
      if (from && id) this.glide(name, from)
    }
  }

  /**
   * A new width, its corner held: it stays where it is and what it comes onto is pushed aside as it grows, and back as it shrinks. Let go
   * (`keep`), that's its row and theirs, under `edit` if it's part of one, else a step of its own: one Undo puts them all back.
   */
  setWidth(name: string, width: number, keep = true, edit?: string) {
    const key = this.resolveName(name)
    const id = this.ids.get(key)
    const node = id ? this.flow?.getNode(id) : undefined
    if (!id || !node) return
    if (this.pulling?.name !== key) this.pulling = { name: key, was: this.boxOf(key) ?? undefined, shown: new Set() }
    const pulling = this.pulling
    const before = this.position(key)!
    this.rememberPose(key, { ...before, w: width })
    this.paintPose(key)
    const row = { x: Math.round(before.x), y: Math.round(before.y), w: Math.round(width) }
    const view = this.views.get(key)
    const box = view && this.laid(key, { ...row, h: heightFor(view, row.w) })
    const may = this.pushable(new Set([key]))
    if (!keep) {
      pulling.field ??= this.field(new Set([key]))
      const pushed = box ? pushAside(box, pulling.field, may, pulling.was) : new Map<string, Box>()
      this.letBack(pushed)
      for (const [other, to] of pushed) this.glide(other, to)
      pulling.shown = new Set(pushed.keys())
      return
    }
    // Let go: where things are now, as any act begins from.
    // Rebuild from committed neighbors, not their live preview, so every push joins this resize edit.
    this.letBack(new Map())
    const field = this.field(new Set([key]))
    const fitted = box && fit(box, field, { pushes: may, was: pulling.was })
    this.letBack(fitted?.pushed ?? new Map())
    this.pulling = null
    this.gesture = edit ?? this.step(`Resized ${key}`, [key])
    this.move(key, row)
    if (box && fitted) {
      this.push(field, fitted.pushed)
      // Only with something that can't be pushed in the way does it move off.
      if (fitted.x !== box.x || fitted.y !== box.y || fitted.w !== box.w) this.slide(key, fitted)
    }
    this.gesture = null
    this.reshape(key)
    this.unsettle(key, true)
  }

  /** What the pull pushed aside and no longer does goes back where it is. */
  private letBack(pushed: Map<string, Box>) {
    for (const other of this.pulling?.shown ?? []) {
      const row = this.rows?.get(other)
      if (!pushed.has(other) && row) this.glide(other, row)
    }
  }

  /** Its shape under the pointer: drawn, and nothing kept yet. */
  drawShape(name: string, width: number, height: number) {
    this.patch(name, { shape: `${width}:${height}` })
    this.setWidth(name, width, false)
    this.reshape(name)
  }

  /** Where a pulled corner lands: what its tool makes, at the width it was pulled to. Let go at its auto shape, none set, it stays auto. */
  setShape(name: string, control: BoxField, width: number, shape: string) {
    const held = this.valueOf(name, control.field)
    const auto = this.get(name)?.auto
    const kept = ratioOf(held) === undefined && auto !== undefined && snapped(auto, shapesOf(control)) === shape
    const edit = this.step(`Reshaped ${this.resolveName(name)}`, [name])
    if (held !== shape && !kept) void this.setField(name, control.field, shape, edit)
    this.patch(name, { shape: undefined })
    this.setWidth(name, width, true, edit)
  }

  /** Its shape unset: it takes its auto shape again, at the width it has. */
  unshape(name: string, field: string) {
    void this.setField(name, field, null)
    this.reshape(name)
  }

  /** Whatever's done to `name`, in its turn, under what it's called by then. Gone by then, there's nothing to do. */
  private act<T>(name: string, action: Omit<Action<T>, "send"> & { send(name: string, write: Write): Promise<T>; edit?: string }): Action<T | typeof NOT_THERE> {
    const id = this.ids.get(this.resolveName(name))
    const inTurn = {
      ...action,
      send: async (once: AbortSignal, outboxed: string) => {
        const served = id && this.namesById.has(id) ? this.served.get(id) : undefined
        return served ? action.send(served, { once, edit: action.edit, action: outboxed }) : NOT_THERE
      },
      done: (answer: T | typeof NOT_THERE) => {
        if (answer !== NOT_THERE) action.done?.(answer as T)
      },
    } as Action<T | typeof NOT_THERE>
    this.outbox.add(inTurn)
    return inTurn
  }

  /** The name a new box takes now, as Calcifer would give it: what's on the canvas, what's being placed, and what waits to be brought back is taken. */
  private nameFor(wanted: string): string {
    const taken = [...this.views].map(([key, view]) => (view.placing ? view.name : key))
    return freeName(wanted, [...taken, ...(this.state?.orphans ?? []).map((orphan) => orphan.name)])
  }

  /**
   * A box, now, whatever the network is doing. `view` is all that's known of it; `ask` makes it in the project, in its turn. The name is the page's until
   * the answer: another writer may have taken it, and then the box takes the one it got. Refused, it goes and `refused` says why. `edit`: what it's part of,
   * else a step of its own.
   */
  place<T extends { name: string; etag?: string; kept?: Record<string, Card | null> }>(
    wanted: string,
    view: Omit<ElementView, "name">,
    dropped: Card,
    ask: (at: Card, write: Write, pushes: Pushes) => Promise<T>,
    refused: (error: unknown) => void,
    done?: (name: string, answer: T, edit: string) => void,
    edit?: string,
  ): string {
    const name = this.nameFor(wanted)
    const asset = view.asset && { ...view.asset, name }
    this.adding += 1
    this.views.set(name, { ...view, name, asset, pending: NOT_YET, landing: true })
    this.reauto(name, this.views.get(name)!)
    const by = edit ?? this.step(`Added ${name}`, [])
    const at = this.clearOf(name, dropped, by)
    this.rows?.set(name, at)
    const node = this.node(name, at)
    this.flow?.addNodes(node)
    this.history.stepOf({ edit: by })?.before.push({ name, node: node.id })
    const left = () => {
      this.adding -= 1
      if (this.adding === 0 && this.state && this.rows && this.flow) this.reconcile()
    }
    // What its drop pushed aside goes with it, the same on every try: one request, one transaction, one entry.
    let pushes: Pushes | undefined
    this.outbox.add<T>({
      send: (once, action) => {
        pushes ??= this.pushesFor(by)
        return ask(at, { once, edit: by, action }, pushes)
      },
      done: (answer) => {
        if (pushes) this.pushesSent(pushes, answer.kept)
        this.served.set(node.id, answer.name)
        // Another writer took the name, unless a rename has moved it on since.
        if (answer.name !== name && this.namesById.get(node.id) === name) this.moveName(name, answer.name)
        const now = this.namesById.get(node.id)
        if (now) {
          const row = this.views.get(now)?.asset
          this.patch(now, { ...(answer.etag ? { known: answer.etag, asset: row && { ...row, etag: answer.etag } } : {}), pending: nextProjectOrder() })
          // What it pushed aside goes with its own row, in one write: one entry of the history.
          if (this.pushedFor(by)) this.move(now, this.rows?.get(now) ?? at, undefined, by)
          done?.(now, answer, by)
        }
        this.readAfter([node.id])
        left()
        this.sendMoves()
      },
      refused: (error) => {
        const now = this.namesById.get(node.id)
        if (now) {
          this.rows?.delete(now)
          this.removeNode(now)
        }
        left()
        if (!this.alive(by)) this.unpush(by)
        this.history.forget(by)
        refused(error)
        this.sendMoves()
      },
    })
    return name
  }

  /** Its row, written in its turn, with whatever else this turn moves. `from`: the page's own doing, worked out from that row (null: none), which
   * it's written over only if Calcifer still has it. A person's move, with none, is written whatever, and one pending makes what follows it so too. */
  private move(name: string, row: Card, from?: Card | null, edit = this.gesture, since?: number) {
    const key = this.resolveName(name)
    const id = this.ids.get(key)
    if (!id) return
    this.rememberPose(key, row)
    // What an act moves, pushed aside or put down, it began from where it was.
    if (edit) {
      this.joinStep(edit, [key])
      this.movingEdits.set(id, (this.movingEdits.get(id) ?? new Set()).add(edit))
    }
    if (from === undefined) {
      this.movingFrom.delete(id)
      this.movingSince.delete(id)
      this.movingSince.delete(id)
    } else if (!this.moving.has(id)) {
      this.movingFrom.set(id, from)
      this.movingSince.set(id, since ?? this.canvasSeq)
    }
    this.masks.set(key, row)
    this.rows?.set(key, row)
    this.moving.set(id, row)
    this.movedAt.set(key, NOT_YET)
    if (this.movesSoon) return
    this.movesSoon = true
    queueMicrotask(() => {
      this.movesSoon = false
      this.sendMoves()
    })
  }

  /** Some box `edit` makes is on its way to Calcifer: what it pushed aside waits to go with it. */
  private coming(edit: string): boolean {
    return this.madeBy(edit).some((node) => {
      const view = this.views.get(this.namesById.get(node) ?? "")
      return Boolean(view && (view.placing || view.pending === NOT_YET))
    })
  }

  /** Some box `edit` makes is still on the canvas. */
  private alive(edit: string): boolean {
    return this.madeBy(edit).some((node) => this.namesById.has(node))
  }

  /** The nodes of the boxes `edit` makes: what it began from with nothing there. */
  private madeBy(edit: string): string[] {
    const before = this.history.stepOf({ edit })?.before ?? this.early.get(edit) ?? []
    return before.flatMap((held) => (!held.view && held.node ? [held.node] : []))
  }

  /** What `edit` pushed aside and hasn't written, each by the name Calcifer has it under: sent with the box the edit makes, one request. */
  pushesFor(edit: string): Pushes {
    const ids = new Map([...this.moving].filter(([id]) => this.movingEdits.get(id)?.has(edit) && this.movingFrom.get(id) !== undefined && this.served.has(id)))
    const placed = Object.fromEntries([...ids.keys()].map((id) => [this.served.get(id)!, this.movingSince.get(id) ?? this.canvasSeq]))
    return { moves: Object.fromEntries([...ids].map(([id, row]) => [this.served.get(id)!, row])), ids, placed }
  }

  /** Those went with their box: what's still that row is written, and what another wrote first stands. */
  pushesSent(pushes: Pushes, kept: Record<string, Card | null> = {}) {
    const at = performance.now()
    for (const [id, row] of pushes.ids) {
      if (this.moving.get(id) !== row) continue
      const name = this.namesById.get(id) ?? ""
      this.moving.delete(id)
      this.movingFrom.delete(id)
      this.movingSince.delete(id)
      this.movingEdits.delete(id)
      if (!((this.served.get(id) ?? name) in kept)) this.movedAt.set(name, at)
    }
    for (const [served, row] of Object.entries(kept)) {
      const name = this.resolveName(served)
      this.masks.delete(name)
      this.movedAt.delete(name)
      if (row) this.applyRows({ [name]: row })
    }
  }

  /** A row `edit` moved is still to be written. */
  private pushedFor(edit?: string): boolean {
    return edit !== undefined && [...this.movingEdits.values()].some((edits) => edits.has(edit))
  }

  /** What `edit` alone pushed aside, still unwritten, goes back where it was. */
  private unpush(edit: string) {
    for (const [id, edits] of this.movingEdits) {
      const from = this.movingFrom.get(id)
      const name = this.namesById.get(id)
      if (!from || !name || edits.size !== 1 || !edits.has(edit)) continue
      this.moving.delete(id)
      this.movingFrom.delete(id)
      this.movingSince.delete(id)
      this.movingEdits.delete(id)
      this.masks.delete(name)
      this.movedAt.delete(name)
      this.rows?.set(name, from)
      this.glide(name, from)
    }
    this.early.delete(edit)
  }

  /** The newest position of each box, in one write when its turn comes, and only those Calcifer has answered for, with all their act moved: the rest go after. */
  private sendMoves() {
    if (this.movesOwed || !this.moving.size) return
    this.movesOwed = true
    const ready = (id: string) => this.views.get(this.namesById.get(id) ?? "")?.pending !== NOT_YET && ![...(this.movingEdits.get(id) ?? [])].some((edit) => this.coming(edit))
    let sent = new Map<string, Card>()
    let edits = new Map<string, Set<string>>()
    this.outbox.add({
      send: (once, action) => {
        sent = new Map([...this.moving].filter(([id]) => ready(id)))
        edits = new Map([...sent.keys()].map((id) => [id, new Set(this.movingEdits.get(id))]))
        const edit = [...new Set([...edits.values()].flatMap((held) => [...held]))].join(",") || undefined
        const cards = [...sent].flatMap(([id, row]): [string, Card, Card | null | undefined, string][] => {
          const name = this.served.get(id)
          return name && this.namesById.has(id) ? [[name, row, this.movingFrom.get(id), id]] : []
        })
        // What this page worked out itself stands aside for another's newer row (cl-800), checked in Calcifer's own transaction: each
        // by the revision it was worked out from.
        const placed = Object.fromEntries(cards.flatMap(([name, , from, id]) => (from === undefined ? [] : [[name, this.movingSince.get(id) ?? this.canvasSeq]])))
        return cards.length && this.projectId ? this.client.moveCards(this.projectId, Object.fromEntries(cards.map(([name, row]) => [name, row])), placed, { once, edit, action }) : Promise.resolve(null)
      },
      done: (answer) => {
        this.movesOwed = false
        const at = performance.now()
        const kept = answer?.kept ?? {}
        for (const [id, row] of sent) {
          const name = this.namesById.get(id) ?? ""
          if (this.moving.get(id) === row) {
            this.moving.delete(id)
            this.movingFrom.delete(id)
          this.movingSince.delete(id)
            this.movingSince.delete(id)
      this.movingSince.delete(id)
            this.movingEdits.delete(id)
            if (!((this.served.get(id) ?? name) in kept)) this.movedAt.set(name, at)
          } else for (const edit of edits.get(id) ?? []) this.movingEdits.get(id)?.delete(edit)
        }
        // Another writer's row came first: it stands, as that writer's move of it would have.
        for (const [served, row] of Object.entries(kept)) {
          const name = this.resolveName(served)
          this.masks.delete(name)
          this.movedAt.delete(name)
          if (row) this.applyRows({ [name]: row })
        }
        if ([...this.moving.keys()].some(ready)) this.sendMoves()
      },
      refused: (error) => {
        this.movesOwed = false
        say("A move didn't save", error)
        for (const id of sent.keys()) {
          this.moving.delete(id)
          this.movingFrom.delete(id)
          this.movingSince.delete(id)
      this.movingSince.delete(id)
          this.movingEdits.delete(id)
          this.masks.delete(this.namesById.get(id) ?? "")
          this.movedAt.delete(this.namesById.get(id) ?? "")
        }
        if (this.projectId) void this.client.canvas(this.projectId).then(({ cards, seq }) => this.applyCanvas(cards, 0, seq), () => {})
      },
    })
  }

  // Naming

  openName(name: string) {
    const view = this.get(name)
    if (view && view.placing?.stage !== "adding") this.patch(name, { draft: view.draft ?? view.name })
  }

  draft(name: string, draft: string) {
    this.patch(name, { draft })
  }

  /** The name field shuts with `text`: an import goes on, anything else is renamed. */
  acceptName(name: string, text: string) {
    const key = this.resolveName(name)
    const view = this.views.get(key)
    if (!view || view.draft === undefined) return
    const chosen = asName(text)
    if (view.placing) {
      this.patch(key, { name: chosen || view.placing.stem, draft: undefined })
      for (const wake of [...this.named]) wake()
    } else {
      this.patch(key, { draft: undefined })
      void this.rename(key, chosen)
    }
  }

  /** The page shows the new name at once; a refusal puts the old one back and says why. */
  private rename(from: string, to: string) {
    const free = to ? freeName(to, this.takenBesides(from)) : from
    const id = this.ids.get(from)
    if (free === from || !id) return
    const edit = this.step(`Renamed ${from} to ${free}`, [from])
    this.renames.set(from, free)
    this.moveName(from, free, true)
    this.act(free, {
      send: (served, write) => this.client.rename(this.projectId!, served, free, write),
      edit,
      done: () => this.served.set(id, free),
      // A try whose answer was lost: its first did it.
      landed: (error) => (error.status === 404 ? { from, to: free, versions: 0 } : undefined),
      refused: (error) => {
        this.history.forget(edit)
        say(`Couldn't rename ${from}`, error)
        this.renames.delete(from)
        this.moveName(this.namesById.get(id) ?? free, from, true)
      },
    })
  }
}
