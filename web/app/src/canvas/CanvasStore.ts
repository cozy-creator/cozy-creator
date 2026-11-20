import type { Board, CanvasNode } from "./board/Board"
import type { Viewport } from "./board/Camera"
import { ApiError, type Client, type Write } from "../api/client"
import { type Action, keepLastWord, type LastWord, lastWordsOf, lost, type OutboxPort } from "../api/outbox"
import { nextProjectOrder } from "../api/projectOrder"
import type {
  Agreed,
  Ahead,
  ToolField as BoxField,
  Imported,
  NoGpu,
  NoRoom,
  AssetDeleted,
  AssetPatch,
  AssetPatched,
  Asset,
  BoxEstimate,
  ErrorBody,
  HistoryStepped,
  Job,
  MainFile,
  Pickup,
  RentFor,
  Card,
  Run,
  RunQuote,
  Splice,
  Standing,
  Tool,
  VersionMoved,
  VersionPrefix,
} from "../api/calcifer"
import type { Entity, Project } from "../types"
import { dismiss, say } from "../ui/notes"
import { VideoPlayback } from "./timeline/Player"
import { signal } from "../ui/signal"
import { carriedFile } from "./carry"
import { Field, fit, type Give, GLIDE_MS, pushAside } from "./clear"
import { type Box, homesFor, readsOf, warningsOf } from "./graph"
import { fieldsOf, fullRows, heldTo, mainText, startingFields } from "./controls"
import { angleOf, baseOf, FPS, framesIn, pickupAt } from "./ranges"
import { asName, freeName, nameKey } from "./names"
import { bodyBox, bodyWidth, canvasBox, faceOf, heightFor, hexOf, madeHere, makes, NARROWEST, paints, ratioOf, reshapes, selectedOf, sentShape, type Shown, shownOf, toolName, newTakeId, type Taking, takingOf, underOf, valueIn, valuesOf, watched, widthFor } from "./shown"
import { mentionsIn, rowsIn, textsIn } from "./text"
import { clipOf, wire } from "./timeline/model"
import { PageHistory, type Target } from "./history"
import { comingOff, goingOn, holdsPictures, readsBox, readsFrom, startsFrom, type ToolStep, wontGoOn } from "./toolchange"
import { isSelf, mentionFrom, numbersOf, SELF, versionIn, versionOf } from "./versions"
import { endTrimmed, mergeValue, same, spliceOf, wordsChanged } from "./words"
import { chose } from "./triggers"
import { plainProject } from "../app/view"

/** The body width in this geometry publication; the node width may be an opened envelope. */
export type ElementData = { name: string; width: number }
export type ElementNode = CanvasNode

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
  /** The name with nothing typed. */
  stem: string
  abort: AbortController
}

/** Everything a card shows that isn't a position. Culling unmounts cards, so a card holds none of it. */
export interface ElementView {
  name: string
  asset?: Asset
  entity?: Entity
  /** An entity's: how the assets it links stand, the worst of them. */
  linked?: Standing | "failed"
  /** The version being looked at, which changes only this page, until it's pinned. */
  viewed?: number
  /** Its turn hasn't come: nothing is making it yet. */
  queued?: boolean
  /** Queued, none of its machines has room on its disk for its models (cl-1175): how much room it lacks. */
  noRoom?: NoRoom
  /** Queued, its machines have no GPU and nothing measured says it runs without one (cl-1279): why. */
  noGpu?: NoGpu
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
  /** Its render was canceled from here: it says Cancelling… until nothing of it is queued or making. */
  canceling?: boolean
  /** What was asked of its row and not read back yet, oldest first. */
  over?: Asking[]
  /** How many writes of its fields are on their way. */
  writing?: number
  placing?: Placing
  /** A dropped picture's own bytes as a blob URL, and once it has landed their digest: shown for that file in place of any copy of it, until it shows another. */
  preview?: { url: string; sha256?: string }
  /** The name field's text, while it's open. */
  draft?: string
  aspect?: number
  /** Being worked on where it stands, the canvas drawn in on it and all it can do around it. */
  here?: boolean
  /** Worked on where it stands, and showing its words, not a take. */
  hereWords?: boolean
  /** A new draft keeps this version's prefix while regenerating from its original beat clock. */
  continuing?: Pickup
  continuingFps?: number
  run?: Run
  /** What's typed and not saved, by field. A card culled away keeps it here. */
  drafts?: Record<string, unknown>
  /** What was written and not read back yet, by field. */
  sent?: Record<string, unknown>
  /** The `etag` its own last write left, until a read catches up: a read of an older write of its own isn't news. */
  known?: string
  /** Its render waits for a machine, with its pickup and its take's id. */
  waiting?: Also
  /** Its Slow here notice is open, under its fields (cl-1004). */
  slowOpen?: boolean
  /** Waiting for a machine, its Render on a new rental is open under it (cl-1066). */
  newOpen?: boolean
  /** Seconds into a video, scrubbed to. */
  scrub?: number
  /** Its ratio while it's being reshaped. */
  shape?: string
  /** The shape it makes with none set: `autoOf`. */
  auto?: number
  /** What sets that shape, in words (`first frame`, `<amy>`); none: its tool's own. */
  autoFrom?: string
  /** Its warnings are open under it. */
  warned?: boolean
  /** Something is being dragged over it. */
  hot?: boolean
  /** What's dragged over it can't go in, and why. */
  refused?: string
  /** The take its frame looks at while it's made, by id (cl-945): from its Render, or its tile picked, until it lands and is looked at. */
  watch?: string
  /** Takes this page asked for, by id, listed from the click (cl-945): each until Calcifer lists it, or a read begun after its render's
   *  answer (`until`, as `pending`) still doesn't. */
  taking?: { id: string; until: number }[]
  /** Takes canceled from this page, by id: never listed again, though Calcifer lists one until its run has stopped. */
  gone?: string[]
  /** How many files dropped on it are on their way to being its versions. */
  joining?: number
  /** It just arrived, and lands. */
  landing?: boolean
  /** What its render has shown so far, the newest revision that landed here, by its run's id and number: kept, dimmed, if the render fails. */
  glimpse?: { run: string; number: number; output: string; index: number; rev: number; media: string; label?: string; sha256?: string }
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
  /** A retained alternative dragged out makes an independent copy. */
  copy?: boolean
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

/** `asset` with `file` its next version: what its row says then, and the number it takes. None when it has that file already. */
function nextVersion(asset: Asset, file: Pick<MainFile, "sha256" | "field"> & Partial<MainFile>): { row: Partial<Asset>; number: number } | null {
  const had = asset.prefixes ?? []
  if (asset.sha256 === file.sha256 || had.some((held) => held.main?.sha256 === file.sha256)) return null
  const first: VersionPrefix[] = had.length || !asset.sha256 ? had : [{ number: 1, prefix: asset.sha256.slice(0, 12) }]
  const number = Math.max(0, ...first.map((held) => held.number)) + 1
  const main = file.path === undefined ? undefined : { output: file.field, path: file.path, ...file }
  const prefixes = [...first, { number, prefix: file.sha256.slice(0, 12), main }]
  return { row: { prefixes, versions: prefixes.length }, number }
}

export const CARD_WIDTH = 240
/** How far right of a box one made from it goes. */
const GAP_BESIDE = 48

const NOT_THERE = Symbol("the box went before its turn came")
const NOT_YET = Number.POSITIVE_INFINITY

/** What goes with one render: a pickup, its take's id, made at the click (cl-945), and a new rental it goes to (`new[:sku]`) at the price
 *  agreed (cl-1066). */
export type Also = { pickup?: Pickup; take?: string; on?: string; agreed?: Agreed }
/** A box's failure when its machine shut down under its run (Calcifer's `MACHINE_LOST`, cl-1085): its note offers Retry. */
const MACHINE_LOST = "machine_lost"
const TYPING_PAUSE_MS = 300
/** Whether a write can go on past the page (`keepalive`): a browser keeps at most 64 KiB of them on their way. */
const lasts = (body: AssetPatch) => JSON.stringify(body).length < 48_000
/** How long after the last take landed what landed is said: several at once are one note (cl-937). */
const LANDED_MS = 1200
const CLEAR_OF_FRAME = 24
/** The gap between a box and one made beside it. */

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

/** What an Undo or Redo brought back without its files, as Calcifer says it (cl-919). */
const lostSaid = (lost: string[]) => lost.map((said) => `${said}.`).join(" ")

/** What a job that ended with no run of its own on one of its box's takes says came of it (a cut, which no run makes either, is timed by
 *  its seconds; these say what they are by themselves): as `cozy render` reads it. */
const SAYS_ITSELF = ["it stands", "nothing to do", "stays pinned"]
const stood = (job: Job) => job.state === "done" && job.run == null && job.number != null && job.seconds == null && Boolean(job.why) && !SAYS_ITSELF.includes(job.why!)

/** What the state says of one element: an asset, or an entity. */
type Placed = Pick<ElementView, "name" | "asset" | "entity" | "linked" | "queued" | "noRoom" | "noGpu" | "warnings">

const viewOf = (element: Placed): ElementView => ({ ...element })

function linksOf(entity: Entity): string[] {
  return [...Object.values(entity.links ?? {}), ...(entity.subs ?? []).flatMap(linksOf)]
}

const WORST = ["making", "failed", "missing", "current"] as const

/** How an entity stands: the worst of what it links. */
const worst = (standings: (Standing | "failed" | undefined)[]) => WORST.find((standing) => standings.includes(standing)) ?? "current"

/**
 * The project's card state, independent of viewport geometry and rendering: a plain store with per-name subscribers. `node.data` is
 * `{name, width}` carries only its identity and published body geometry. Other view state never lives in the renderer, and an import or a rename stays the same node.
 */
export class CanvasStore {
  private retired = false
  projectId: string | null
  private client: Client
  private viewport: Board | null = null
  private views = new Map<string, ElementView>()
  private playbacks = new Map<string, VideoPlayback>()
  private aliases = new Map<string, string>()
  private subscribers = new Map<string, Set<() => void>>()
  readonly sceneRevision = signal(0)
  canvasAt: (x: number, y: number) => string | null = () => null
  private shapes = new Map<string, string>()
  /** Each view's stack, by the view: a change is a new view. */
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
  /** The failure said of each box since it was last asked to render: said once, though a read before its render's answer and one after both
   *  find it (a render that fails before it is sent, as one whose input has no take does, cl-1192). */
  private told = new Map<string, string>()
  /** Each job that ended on a take its box already had, said once; those on the board when this page first read it never (cl-1271). */
  private stoodSaid = new Set<string>()
  private stepped = 0
  /** The tools known so far, by id. */
  readonly tools = signal<Map<string, Tool>>(new Map())
  /** Names came or went: a text shows a name as a chip or as words by it. */
  readonly names = signal(0)
  /** What machines fetch ahead for this project's boxes waiting their turn, by run: each box shows its own (cl-905). */
  readonly ahead = signal<Map<number, Ahead>>(new Map())
  /** The menu open, and where: a box's, or with `number` its take's. */
  readonly menu = signal<{ name: string; x: number; y: number; number?: number } | null>(null)
  /** The box whose file is being looked at large, over everything. */
  readonly viewing = signal<string | null>(null)
  /** The one asset shown in the viewport-sized Focus surface. */
  readonly focusedName = signal<string | null>(null)
  readonly focusEntering = signal(false)
  /** Aligns the canvas media with the measured Focus viewer before revealing its controls. */
  enterFocus: (room: { x: number; y: number; width: number; height: number } | null) => void = () => {}
  /** The focused viewer owns its image camera separately from the canvas. */
  focusZoomBy?: (direction: number) => void
  focusZoomTo?: (scale: number) => void
  focusFit?: () => void
  /** Its boxes drawn once. What hangs on each, the line under it, its meter and its corner, comes the paint after: none of them takes room
   *  in it, and the frame a project opens in is its boxes alone (cl-982). */
  readonly drawn = signal(false)
  /** What takes a drop, by its element: a box marks itself `data-drop-target`. */
  readonly dropTargets = new Map<Element, DropTarget>()
  /** Each open text's way to take a name at its caret, by `<node>:<place>`. */
  private carets = new Map<string, (name: string) => void>()
  /** Has these assets, and what's said of the project, read. */
  readAgain: (names: string[]) => void = () => {}
  /** Has the project read whole. */
  readWhole: () => void = () => {}
  /** These boxes are off this page: no read asks for them. */
  forget: (names: string[]) => void = () => {}
  /** Calcifer answered taking these off: what the stream says of them after is another writer's, and read. */
  forgotten: (names: string[]) => void = () => {}
  /** Asks for a machine, for a render with none up. */
  askMachine: () => void = () => {}
  /** Opens Rent, with a GPU that holds it chosen when one is known. */
  rent: (pick?: RentFor | null) => void = () => {}
  /** Shows download or install `on` on `machine`, in its drawer (cl-1205, cl-1218): where a box's "Waiting for download" leads. */
  showWaited: (machine: string, on: string) => void = () => {}
  /** Each open box's typical render time where it would go now, by name, as Calcifer last said (cl-1004). */
  readonly estimates = signal<ReadonlyMap<string, BoxEstimate>>(new Map())
  /** The machines a render may go to, as text: an estimate is asked again when they change. */
  readonly machineSet = signal("")
  /** Each box's newest estimate asked, by name: an older answer is dropped. */
  private estimating = new Map<string, number>()
  /** The box being worked on where it stands: the view fitted to it, and whether a hand has since moved the canvas or is pulling its corner. */
  inPlace: { name: string; fitted?: Viewport; moved?: boolean; pulled?: boolean } | null = null
  /** While Focus is open and on the way back from it, the canvas's zoom outside it: what the board asks pictures at, so the boxes
   *  behind Focus keep the pictures they had and come back drawn at once, never asked again at Focus's zoom (cl-1207). */
  restZoom: number | null = null
  /** Done with the box where it stands: what's typed is saved, and the canvas goes back. */
  leaveHere: () => void = () => {}
  /** Opens a box where it stands, as a click on it does. */
  openHere: (name: string) => void = () => {}
  /** Each of these again beside where they stand, selected: Ctrl+D. */
  duplicate: (names: string[]) => void = () => {}
  /** Files from the computer, each a box on the canvas, beside `near`: `then` hears each one's name once it's there. */
  placeFiles: (files: File[], then: (name: string) => void, near?: string) => void = () => {}
  /** The box to open on its next take's words: an image tool was just put on its pictures. */
  writeOnOpen: string | null = null
  /** Fits the canvas to the box being worked on, and all around it. `only`: only if something no longer fits. */
  fitHere: (only?: boolean) => void = () => {}
  /** The box being worked on has another name now. */
  hereRenamed: (name: string) => void = () => {}
  /** The name a just-dropped tool will have: its text takes the keys. */
  focusing: string | null = null
  /** The machines a render may go to, for what the page asks first (none, or this computer alone); Calcifer says which (cl-894). */
  machines: () => string[] = () => []
  private ids = new Map<string, string>()
  /** The last `GET canvas` taken in: a canvas drawn again is handed it again, and what's happened since stands. */
  private cardsRead?: Record<string, Card>
  /** Each box by its node's id, the same whatever it's called: an action waits for its turn by this. */
  private namesById = new Map<string, string>()
  /** What Calcifer calls each, by id: a rename on its way shows here first. */
  private served = new Map<string, string>()
  /**
   * For each box, by node id, each field typed in: what it was last known to hold and the project document's heads that was at, which its words go
   * as splices of, wherever they are in it; and what's on its way. What's typed merges by character with anyone else's (cl-904).
   */
  private typedFrom = new Map<string, Map<string, { value: unknown; heads: string[]; sent?: unknown }>>()
  /** The page has gone (`pagehide`): what's typed went in its last flush. */
  private gone = false
  /** Each box's last word, by node id, as it stands: what goes as the page goes, and, in a plain project, what this tab keeps as it's typed for
   *  a reload at any instant to send again under the same key (cl-973). */
  private lastWords = new Map<string, LastWord>()
  private nextId = 0
  private state: Project | null = null
  private began = 0
  /** When the last read of the whole project began: only one such begun after a rename's answer can say it landed. */
  private wholeBegan = 0
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
  /** When each of `deleting` was taken off: a read begun before can only say it was there. */
  private deletingFrom = new Map<string, number>()
  /** Of `deleting`, those a read begun after they were taken off found there before their delete was answered: another writer's, read again
   *  once it is. */
  private heldBack = new Set<string>()
  /** The etags an asset's own writes left, since a read last caught up. */
  private own = new Map<string, Set<string>>()
  /** Names about to land: what was dropped here, and nothing a read found. */
  private landings = new Set<string>()
  /** A position of ours that the stream hasn't echoed: it wins until then. */
  private masks = new Map<string, Card>()
  private gliding = new Map<string, ReturnType<typeof setTimeout>>()
  /** Boxes to clear of what they lie on, as soon as this turn's changes are in, and why: placed by another writer, or grown. */
  private unsettled = new Map<string, "placed" | "grown">()
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
  /** When Calcifer answered each rename of `renames`, by its old name: a read begun after has it. */
  private renamedAt = new Map<string, number>()
  /** Told each time the project is read into the canvas, or the canvas is put away. */
  private hearing = new Set<() => void>()
  private adding = 0
  private seen = false
  private selected: Set<string>
  private named = new Set<() => void>()
  /** The project was read for the first time; whether it held anything. */
  onFirstRead?: (held: boolean) => void
  /** Elements with no row were just placed. */
  onPlaced?: () => void

  constructor(client: Client, outbox: OutboxPort, projectId: string | null, selected: Iterable<string>) {
    this.client = client
    this.outbox = outbox
    this.projectId = projectId
    this.history = new PageHistory(client, () => this.projectId)
    this.selected = new Set(selected)
    // What's typed goes as the tab is hidden, as the page is about to go (`beforeunload`: Firefox ends what's in flight as soon as it starts to
    // navigate, before `pagehide`), and once more as it goes, which is its last word (cl-904, cl-973). Back from the browser's cache, it goes on.
    document.addEventListener("visibilitychange", () => document.visibilityState === "hidden" && this.flush(true))
    addEventListener("beforeunload", () => this.flush(true))
    addEventListener("pagehide", () => this.leaving())
    addEventListener("pageshow", (event) => {
      if (event.persisted) this.gone = false
    })
  }

  get active() { return !this.retired }

  /** The project has been read, so what isn't here isn't in it. */
  get hasRead() {
    return this.seen
  }

  /** No known or optimistic element, including one still waiting for its row. */
  get empty() {
    return !this.views.size && !this.state?.assets.length && !this.state?.entities?.length
  }

  /** A canvas drawn for it, the first or again: back on a project, its boxes are as it left them, whatever's still on its way to Calcifer. */
  attach(viewport: Board) {
    if (this.retired) return
    this.viewport = viewport
    if (this.state && this.rows) this.reconcile()
  }

  /** Its canvas goes, and the store stays for when it's drawn again: nothing is open or held in a drag by then. */
  detach() {
    this.viewport = null
    if (this.inPlace) this.patch(this.inPlace.name, { here: undefined, hereWords: undefined })
    this.inPlace = null
    this.restZoom = null
    this.focusEntering.set(false)
    this.focusedName.set(null)
    this.dragging.clear()
    this.dragFrom.clear()
    this.setHot(null)
    this.menu.set(null)
    this.viewing.set(null)
    for (const told of [...this.hearing]) told()
  }

  /** A confirmed vault lock retires even an off-route store. Navigation alone keeps its work. */
  revokePrivateView() {
    this.retire()
  }

  /** Everything typed in it written now, and answered: before its project closes. */
  async prepareClose() {
    if (this.retired) return
    const saved = await Promise.all([...this.views.keys()].map((name) => this.save(name)))
    this.sendMoves()
    if (saved.some((saved) => !saved)) throw new Error("A canvas edit still needs attention")
  }

  retire() {
    if (this.retired) return
    this.retired = true
    for (const pause of this.pauses.values()) clearTimeout(pause)
    this.pauses.clear()
    for (const view of this.views.values()) {
      view.placing?.abort.abort()
      if (view.preview) {
        URL.revokeObjectURL(view.preview.url)
        view.preview = undefined
      }
    }
    for (const timer of this.gliding.values()) clearTimeout(timer)
    this.gliding.clear()
    this.viewport?.replaceNodes([])
    this.detach()
    this.projectId = null
    this.state = null; this.rows = null; this.cardsRead = undefined
    this.typing = null; this.gesture = null; this.pulling = null
    this.history.retire()
    this.views.clear(); this.aliases.clear(); this.shapes.clear(); this.asked.clear(); this.unsentSaid.clear(); this.runs.clear()
    this.early.clear(); this.offers.clear(); this.dropTargets.clear(); this.carets.clear(); this.ids.clear(); this.namesById.clear(); this.served.clear()
    this.poses.clear(); this.owners.clear(); this.deleting.clear(); this.own.clear(); this.landings.clear(); this.masks.clear(); this.unsettled.clear()
    this.movedAt.clear(); this.moving.clear(); this.movingFrom.clear(); this.movingEdits.clear(); this.movingSince.clear(); this.renames.clear(); this.renamedAt.clear()
    this.selected.clear(); this.tools.set(new Map())
    for (const listeners of this.subscribers.values()) for (const listener of listeners) listener()
    for (const told of [...this.hearing]) told()
    this.subscribers.clear(); this.named.clear(); this.hearing.clear()
    this.menu.set(null)
    this.viewing.set(null)
    if (this.inPlace) this.patch(this.inPlace.name, { here: undefined, hereWords: undefined })
    this.inPlace = null
    this.restZoom = null
    this.focusEntering.set(false)
    this.focusedName.set(null)
  }

  // Reading, by name

  resolveName(name: string): string {
    return this.aliases.get(name) ?? name
  }

  /** What Calcifer calls it: a rename on its way says the name it had until it lands. */
  servedName(name: string): string {
    const key = this.resolveName(name)
    return this.served.get(this.ids.get(key) ?? "") ?? key
  }

  get = (name: string): ElementView | undefined => this.views.get(this.resolveName(name))

  subscribe(name: string, listener: () => void): () => void {
    let listeners = this.subscribers.get(name)
    if (!listeners) this.subscribers.set(name, (listeners = new Set()))
    listeners.add(listener)
    return () => listeners.delete(listener)
  }

  private notify(name: string) {
    this.sceneRevision.set(this.sceneRevision.get() + 1)
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
    const { ratio: auto, from: autoFrom } = this.autoOf(view)
    if (auto === view.auto && autoFrom === view.autoFrom) return view
    const now = { ...view, auto, autoFrom }
    this.views.set(key, now)
    if (reshapes(now)) {
      this.notify(key)
      this.reshape(key)
    }
    return now
  }

  /** The shape a box makes with none set, and what sets it. A picture tool's, as Cozy's image packages take it: the first picture it mentions and can
   *  use (one crossed out sets nothing), held to what its tool makes, else its tool's own. A video tool's: the first picture or video in its slots
   *  (H3's first or last frame), never a mention, as a portrait of who's in it makes no vertical shot; else 16:9 (se-128). */
  private autoOf(view: ElementView): { ratio?: number; from?: string } {
    const asset = view.asset
    const tools = this.tools.get()
    const fields = asset && makes(view) ? fieldsOf(asset, tools) : []
    const control = fields.find((held) => held.type === "shape")
    if (!asset || !control) return {}
    if (!control.auto) return { ratio: heldTo(control, ratioOf(control.default) ?? 1) }
    const kind = tools.get(asset.tool ?? "")?.makes ?? asset.kind
    const shapeOf = (mention: string, media: string[]) => {
      const read = this.mentionOf(mention, view.name)
      const of = read ? this.views.get(read.name) : undefined
      return read && of?.asset && media.includes(of.asset.kind) ? sentShape(of, read.number) : undefined
    }
    if (kind === "video") {
      for (const slot of fields.filter((held) => held.type === "slot")) {
        for (const mention of textsIn(valueIn(view, slot.field)).flatMap(mentionsIn)) {
          const ratio = shapeOf(mention, ["image", "video"])
          if (ratio) return { ratio: heldTo(control, ratio), from: slot.label.toLowerCase() }
        }
      }
      return { ratio: heldTo(control, ratioOf(control.default) ?? 16 / 9) }
    }
    for (const mention of this.mentionsOf(view)) {
      if (asset.unused?.some((one) => one.mention === mention)) continue
      const ratio = shapeOf(mention, [kind])
      if (ratio) return { ratio: heldTo(control, ratio), from: `<${mention}>` }
    }
    // The SDK's own default with none: 1:1.
    return { ratio: heldTo(control, ratioOf(control.default) ?? 1) }
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
      if (!of?.asset || of.asset.kind !== "image" || view.asset?.unused?.some((one) => one.mention === mention) || seen.has(mention)) return []
      seen.add(mention)
      const file = read?.number !== undefined ? of.asset.prefixes?.find((held) => held.number === read.number)?.main : selectedOf(of)
      if (file && !file.absent && file.field === "image") return [{ mention, sha256: file.sha256, shape: file.width && file.height ? file.width / file.height : undefined }]
      // Nothing of it yet: its frame, at the shape it will be, and whether it's being made.
      return [{ mention, shape: sentShape(of) ?? 1, making: read?.number === undefined && !file && Boolean(of.asset.making?.length || of.queued || of.asked !== undefined) }]
    })
  }

  /** What a box asked to render waits on: the first box it reads with nothing made yet, or that's being made. */
  waitsFor(name: string): string | null {
    const asset = this.get(name)?.asset
    for (const read of asset?.reads ?? []) {
      const of = this.get(this.mentionOf(read.name, name)?.name ?? "")
      if (of?.asset && (read.version !== undefined && read.version !== null ? versionIn(of.asset, read.version)?.main : selectedOf(of))) continue
      if (of?.asset?.tool && (of.asset.making?.length || of.queued || of.asked !== undefined || (!of.asset.prefixes?.length && !of.asset.sha256))) return read.name
    }
    return null
  }

  /** Writing its next take in its frame, the caret in its words, which takes them again when it's already there. The words are those last written, word
   * for word (cl-909), from the take looked at: their first take of its own is that one, `<self.N>` (Paul 11-06), where its tool reads it (`selfOf`).
   * Words with none stay as they are; `lead` puts it first. A slot holds the take, or what was put there by hand unless it's the take that's edited. */
  writeNext(name: string, lead = false) {
    const view = this.get(name)
    const asset = view?.asset
    if (!view || !asset?.tool) return
    if (this.inPlace?.name === this.resolveName(name)) this.focusFit?.()
    const { field, slot } = this.selfOf(asset)
    const said = this.valueOf(name, field)
    const words = typeof said === "string" ? said : ""
    const number = shownOf(view)?.number
    const self = `<${SELF}.${number}>`
    const next = slot ? (!words.trim() || /^\s*<self\.\d+>\s*$/i.test(words) || lead ? self : words)
      : /^\s*(<self\.\d+>\s*)?$/i.test(words) ? `${self} ` : /<self\.\d+>/i.test(words) ? words.replace(/<self\.\d+>/i, self) : lead ? `${self} ${words}` : words
    if (number !== undefined && next !== words) this.setDraft(name, field, next)
    this.focusing = name
    this.patch(name, { hereWords: true })
  }

  /** What a box's fields mention, in order. */
  private mentionsOf(view: ElementView): string[] {
    return view.asset?.tool ? fieldsOf(view.asset, this.tools.get()).flatMap((field) => textsIn(valueIn(view, field.field))).flatMap(mentionsIn) : []
  }

  /** The field a tool's next take is written in: its main text. */
  private promptOf(tool: string): string {
    return mainText(this.tools.get().get(tool)?.fields ?? [])?.field ?? "instructions"
  }

  /** Where a box's take, `<self.N>`, goes in its tool's fields: where the tool reads a box of its kind, as putting it on one starts it there
   *  (`readsFrom`, Calcifer's `with_tool`): its first required slot of that kind (Upscale's `image`), else its main text (Qwen Image's). */
  private selfOf(asset: Asset): { field: string; slot: boolean } {
    const tool = this.tools.get().get(asset.tool ?? "")
    const read = tool ? readsFrom(tool, asset.kind) : undefined
    return read?.type === "slot" ? { field: read.field, slot: true } : { field: this.promptOf(asset.tool ?? ""), slot: false }
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
    const at = this.dragging.has(key) && id ? this.viewport?.getNode(id)?.position : undefined
    return { x: at?.x ?? row.x, y: at?.y ?? row.y, w, h: heightFor(view, w) }
  }

  /** Position is the canonical anchor; bounds hold the displayed body's offset. */
  private draggedRow(node: ElementNode): Card {
    const row = this.position(node.data.name)
    return { x: Math.round(node.position.x), y: Math.round(node.position.y), w: row?.w ?? this.poseOf(node.data.name)?.w ?? CARD_WIDTH }
  }

  private paintPose(name: string) {
    const id = this.ids.get(name)
    const pose = this.poseOf(name)
    const view = this.get(name)
    if (!id || !pose || !view) return
    const drawn = bodyBox(view, pose)
    this.viewport?.updateNode(id, (queued) => ({ data: { ...queued.data, width: pose.w }, position: { x: pose.x, y: pose.y }, bounds: drawn }))
  }

  /** What names run `number` of this project (cl-987): `KEY-N`, by its project's key; its number before the project is read. */
  runName(number: number): string {
    const key = this.state?.key
    return key ? `${key}-${number}` : (this.runs.get(number)?.name || String(number))
  }

  // From Calcifer

  applyState(state: Project) {
    if (this.retired) return
    this.state = state
    this.began = state.began ?? 0
    this.wholeBegan = state.wholeBegan ?? this.began
    const ahead = state.board?.ahead ?? []
    if (ahead.length || this.ahead.get().size) this.ahead.set(new Map(ahead.map((row) => [row.run, row])))
    if (this.rows && this.viewport) this.reconcile()
  }

  /** `GET canvas`: every row, but what this page moved since that read began: it can't know. */
  applyCanvas(cards: Record<string, Card>, began = 0, seq?: number) {
    if (this.retired) return
    this.heard(seq)
    if (cards === this.cardsRead) return
    this.cardsRead = cards
    const first = !this.rows
    const rows = new Map(Object.entries(cards))
    // A rename on its way: Calcifer's row under the old name is this box's, and one under the new name is another's.
    for (const [from, to] of this.renames) {
      const ours = rows.get(from) ?? this.rows?.get(to)
      rows.delete(from)
      if (ours) rows.set(to, ours)
      else rows.delete(to)
    }
    for (const [key, view] of this.views) if (view.placing && this.rows?.has(key)) rows.set(key, this.rows.get(key)!)
    for (const [name, at] of this.movedAt) {
      const mine = this.rows?.get(name)
      if (at >= began && mine) rows.set(name, mine)
    }
    this.rows = rows
    if (first) {
      if (this.state && this.viewport) this.reconcile()
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
    const becoming = new Set(this.renames.values())
    for (const [served, row] of Object.entries(cards)) {
      // A rename on its way: Calcifer's row under the old name is this box's, and one under the new name is another's. Gone from the old
      // name, it's landed or the box went, as the next read says.
      const name = this.renames.get(served) ?? served
      if (becoming.has(served) && !this.renames.has(served)) continue
      if (row) {
        this.rows.set(name, row)
        this.applyRow(name, row)
        if (!this.masks.has(name)) this.unsettle(name, "placed")
      } else if (name === served) {
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
    const owner = this.owners.get(run.number) ?? [...this.views].find(([, view]) => watched(view)?.run === run.number)?.[0]
    if (!owner) return
    const viewed = this.get(owner)
    // A take looked at that failed or is canceled is gone: the frame goes back to what it showed (cl-1070).
    if (viewed && watched(viewed)?.run === run.number && (run.status === "failed" || run.status === "canceled")) this.patch(owner, { watch: undefined })
    // The run its frame is on: the take looked at (cl-945), else the first going. Another's progress is its own tile's.
    const making = this.get(owner)?.asset?.making ?? []
    const view = this.get(owner)
    if (run.number !== ((view && watched(view)?.run) ?? Math.min(...making))) return
    // Only what's here: an output of a run on another machine, or a revision that hasn't landed, has nothing yet to show. A preview published
    // as it goes is a revision that landed while its output is still in progress.
    const shown = [...(run.output ?? [])].reverse().find((item) => Boolean(item.url) && /^(image|video)\//.test(item.media_type))
    const glimpse = shown && { run: run.id, number: run.number, output: shown.name, index: shown.index, rev: shown.rev, media: shown.media_type.split("/")[0], label: shown.label, sha256: shown.sha256 ?? undefined }
    const held = this.get(owner)?.glimpse
    this.patch(owner, glimpse && JSON.stringify(glimpse) !== JSON.stringify(held) ? { run, glimpse } : { run })
  }

  private reconcile() {
    if (!this.drawn.get()) requestAnimationFrame(() => setTimeout(() => this.drawn.set(true)))
    const state = this.state!
    const rows = this.rows!
    const entities = state.entities ?? []
    const present = new Set([...state.assets, ...entities].map((element) => element.name))
    const before = [...this.views.keys()]
    // A box there in a read begun before its delete was answered may be another writer's under its name (macOS e2e 43722005881: the
    // stream's "gone" then "changed" came before the delete's answer): read again once that's answered, whole, which never 404s.
    let again = false
    for (const [name, until] of this.deleting) {
      if (!present.has(name) || until < this.began) {
        this.deleting.delete(name)
        this.deletingFrom.delete(name)
        this.heldBack.delete(name)
      } else if (this.began > (this.deletingFrom.get(name) ?? NOT_YET)) {
        if (until === NOT_YET) this.heldBack.add(name)
        else again = true
      }
    }
    for (const name of this.deleting.keys()) present.delete(name)
    if (again) this.readWhole()
    // A rename on its way has landed once a read of the whole project began after Calcifer answered it, or the project has the new name and
    // not the old. With both, another writer took the name meanwhile: that box isn't this one, and the answer says what becomes of this one.
    // A read of some boxes alone, begun after the answer, says nothing of this one: it stays where it is, Focus, typing and what's on its way
    // to it included (offline, e2e 44082601717: read without it, it went, and a picture dropped on it found no box).
    for (const [from, to] of this.renames) {
      if ((this.renamedAt.get(from) ?? Infinity) < this.wholeBegan || (present.has(to) && !present.has(from))) {
        this.renames.delete(from)
        this.renamedAt.delete(from)
      }
    }
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
    // A render that made nothing new: its tool takes no seed, or its box names one, and a take already holds its inputs, so that take
    // stands and no run went (Calcifer's `Project::stands`). Said once in a note as it ends, as a failed try is: never silence (cl-1271).
    for (const job of state.board?.jobs ?? []) {
      if (!stood(job) || this.stoodSaid.has(job.id)) continue
      this.stoodSaid.add(job.id)
      if (this.seen) say(`${job.asset}: nothing new to render: ${job.why}`)
    }
    // Queued: its turn not come, as the walk's queue says, or a take of it not sent yet, as its row lists it (cl-945).
    const queued = new Set([...(state.queue ?? []).filter((item) => !item.running).map((item) => item.name), ...state.assets.filter((asset) => asset.takes?.some((take) => take.queued)).map((asset) => asset.name)])
    const noRoom = new Map((state.queue ?? []).flatMap((item) => (item.no_room ? [[item.name, item.no_room] as const] : [])))
    const noGpu = new Map((state.queue ?? []).flatMap((item) => (item.no_gpu ? [[item.name, item.no_gpu] as const] : [])))
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
      if (this.renames.has(asset.name) || becoming.has(asset.name) || this.deleting.has(asset.name)) continue
      const view = this.views.get(asset.name)
      if (view) {
        const change: Partial<ElementView> = { ...said(view, asset.name) }
        // What a take's run warned of, once it made the take, is the box's to show (its Warnings), never a note: a handled fallback
        // is no error (Paul, 11-12: a burst of "CUDA graph" notes over a picture that came out fine).
        // A render that fails says so as it fails, wherever the person is looking, the same failure again too once this page asked for it,
        // in a note that goes by itself (Paul, 11-16). One that failed before this page opened is the box's to show.
        const answered = view.asked !== undefined && view.asked < this.began
        if (asset.failure && this.hasRead && (answered || asset.failure.message !== view.asset?.failure?.message) && this.told.get(asset.name) !== asset.failure.message) {
          this.told.set(asset.name, asset.failure.message)
          this.failed(asset.name, asset.failure)
        }
        if (Boolean(view.queued) !== queued.has(asset.name)) change.queued = queued.has(asset.name)
        if (view.noRoom?.line !== noRoom.get(asset.name)?.line) change.noRoom = noRoom.get(asset.name)
        if (view.noGpu?.line !== noGpu.get(asset.name)?.line) change.noGpu = noGpu.get(asset.name)
        if (view.canceling && !asset.making?.length && !asset.takes?.length && !queued.has(asset.name)) change.canceling = undefined
        if (view.pending !== undefined && view.pending < this.began) change.pending = undefined
        if (answered) change.asked = undefined
        // Its own takes (cl-945): each listed from the click until Calcifer lists it, as a take or as its version, or a read begun after
        // its render's answer still doesn't (nothing was made, and the answer said why). No read from before then takes one off.
        const listed = new Set([...(asset.takes ?? []).map((take) => take.id), ...(asset.prefixes ?? []).map((held) => held.id).filter(Boolean)])
        const taking = view.taking?.filter((take) => !listed.has(take.id) && take.until >= this.began)
        if (view.taking && taking?.length !== view.taking.length) change.taking = taking?.length ? taking : undefined
        const gone = view.gone?.filter((id) => asset.takes?.some((take) => take.id === id))
        if (view.gone && gone?.length !== view.gone.length) change.gone = gone?.length ? gone : undefined
        const over = view.over?.filter((ask) => ask.until >= this.began)
        if (view.over && over?.length !== view.over.length) change.over = over?.length ? over : undefined
        let landing: VersionPrefix | undefined
        if (view.asset !== asset || view.landed) {
          // What its tool's rules chose moved by a write of this page's (cl-1281): the triggers go with it.
          const mine = Boolean(view.writing) || view.pending === NOT_YET || (asset.etag != null && (asset.etag === view.known || Boolean(this.own.get(asset.name)?.has(asset.etag))))
          chose(this, asset.name, view.asset?.chosen, asset.chosen, mine)
          Object.assign(change, { asset: asked(asset, over), landed: undefined })
          if (!asset.making?.length) change.run = undefined
          const newest = asset.prefixes?.at(-1)?.number
          if (newest && newest !== view.asset?.prefixes?.at(-1)?.number && view.asset) Object.assign(change, { slid: newest, glimpse: undefined })
          // A take seen being made landed: it's said, with View (cl-937), unless it's looked at as it lands. A file dropped on it or one undo brings back isn't.
          const made = asset.prefixes?.at(-1)
          const making = (held?: { id?: string; run?: number | null }) => Boolean(held && ((held.id && takingOf(view).some((take) => take.id === held.id)) || (held.run != null && view.asset?.making?.includes(held.run))))
          if (this.hasRead && made && newest !== view.asset?.prefixes?.at(-1)?.number && making(made)) landing = made
          // Its first take, landing while it's open on its words with nothing typed: it shows the take. An image tool's words are its next take's, and stay.
          if (newest && !view.asset?.prefixes?.length && view.here && view.hereWords && !view.drafts && !asset.timeline && !paints(view, this.tools.get())) change.hereWords = undefined
          // The take it was rendered to look at landed, with nothing newer being made: open, it's the one looked at. At rest a box goes on
          // showing what it showed (cl-936), what's approved staying and what lands beside it unseen (cl-928): its watch goes once nothing is on its way.
          // What Calcifer makes itself (a frame, a cut) has no run to watch: its take landing is the one it was rendered to look at.
          const watching = watched({ ...view, asset })
          const seen = watching && madeHere(asset.tool) && (newest ?? 0) > (view.asset?.prefixes?.at(-1)?.number ?? 0) ? { ...watching, number: newest } : watching
          const still =(id: string) => listed.has(id) || Boolean(taking?.some((take) => take.id === id))
          if (!view.here && view.watch !== undefined && !asset.takes?.length && !taking?.length && !queued.has(asset.name) && ("asked" in change ? change.asked : view.asked) === undefined) change.watch = undefined
          else if (seen?.number !== undefined) Object.assign(change, { watch: undefined, viewed: seen.number === asset.version ? undefined : seen.number, ...(asset.kind === "video" ? { hereWords: undefined, continuing: undefined, continuingFps: undefined } : {}) })
          // Ended with no version (failed, canceled): nothing of it to look at.
          else if (view.watch !== undefined && !still(view.watch)) change.watch = undefined
          // Read back: what was sent is what it holds, or someone wrote since. A read of a write of its own, older than its last, says neither.
          // What's typed and what's still on its way show over it, field by field: the page's unsent words win, and every other field is what's read.
          const between = asset.etag !== view.known && this.own.get(asset.name)?.has(asset.etag ?? "")
          if (asset.etag !== view.asset?.etag && !between) {
            Object.assign(change, { sent: view.writing ? view.sent : undefined, known: undefined, ...this.typedOver(view, asset) })
            this.own.delete(asset.name)
          }
          // Pinned where it was being looked: there's nothing left to look at.
          const previewed = "viewed" in change ? change.viewed : view.viewed
          if (previewed !== undefined && (previewed === asset.version || !asset.prefixes?.some((held) => held.number === previewed))) change.viewed = undefined
        }
        if (Object.keys(change).length) {
          this.patch(asset.name, change)
          this.reshape(asset.name)
        }
        // Its note shows a picture of an image or a video only, as the take says its file is: a text tool's words have none to ask for.
        const pictured = landing?.main && ["image", "video"].includes(landing.main.field) ? landing.main.sha256 : undefined
        if (landing && !this.lookedAt(asset.name, landing.number)) this.sayLater(asset.name, landing.id || undefined, landing.number, pictured)
      } else if (this.adding === 0 && !this.renames.has(asset.name)) {
        // While an import is being asked, a new name may be that import's: its answer says, then this runs again. A box's old name while
        // its rename is on its way is that box, under its new name here: never another box of it (e2e 44094179191: `named` was drawn again
        // beside `renamed`, which went).
        fresh.push({ name: asset.name, asset, queued: queued.has(asset.name), noRoom: noRoom.get(asset.name), noGpu: noGpu.get(asset.name), warnings: warnings.get(asset.name) })
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
      } else if (this.adding === 0 && !this.renames.has(entity.name)) {
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
      this.sendLastWords()
    } else {
      for (const { name } of fresh) this.unsettle(name, "placed")
    }
    this.names.set(this.names.get() + 1)
    for (const told of [...this.hearing]) told()
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
      if (!lost(error)) say("Couldn't load where the boxes are", error)
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
      // A node just added isn't in the viewport until it next draws; its row is.
      const node = this.viewport?.getNode(id)
      const row = this.rows?.get(name)
      const view = this.views.get(name)
      if ((!node && !row) || !view) continue
      const pose = this.poseOf(name)
      if (pose) boxes.set(name, this.laid(name, pose))
    }
    const views = new Map(left.map((element) => [element.name, viewOf(element)]))
    const sizes = new Map(left.map((element) => {
      const view = views.get(element.name)!
      const w = widthFor(view)
      return [element.name, { w: bodyWidth(view, w), h: heightFor(view, w) + underOf(view) }]
    }))
    const names = left.map((element) => element.name)
    const placed = homesFor(names, readsOf(this.state!), boxes, (name) => sizes.get(name)!, boxes.size ? this.onScreen() : undefined)
    return new Map([...placed].map(([name, row]) => {
      const view = views.get(name)!
      const box = canvasBox(view, { x: row.x, y: row.y, w: row.w ?? sizes.get(name)!.w, h: sizes.get(name)!.h })
      return [name, { x: box.x, y: box.y, w: box.w }]
    }))
  }

  /** The canvas as far as it shows, clear of the top bar and the tools. */
  private onScreen(): Box | undefined {
    const seen = this.viewport?.getViewport()
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
    this.viewport?.upsertNodes(nodes)
    this.followAll()
    // What watches a name hears it's here: one placed after its read (no row yet, `findHomes`) came after `names` last moved.
    for (const { element } of items) this.notify(element.name)
    this.names.set(this.names.get() + 1)
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
    const drawn = bodyBox(view, { x: row.x, y: row.y, w: width, h: height })
    this.shapes.set(name, this.shapeOf(name, width, height))
    return {
      id,
      type: view.entity ? "entity" : "element",
      position: { x: row.x, y: row.y },
      data: { name, width },
      bounds: drawn,
      selected,
    }
  }

  /** A model's Hub card, as Calcifer reads it. */
  modelDetail(name: string) {
    return this.client.model(name)
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
    const node = id ? this.viewport?.getNode(id) : undefined
    if (!id || !view || !node) return
    const width = this.poseOf(key)?.w ?? widthFor(view)
    const height = heightFor(view, width)
    const shape = this.shapeOf(key, width, height)
    const before = this.shapes.get(key)
    if (before === shape) return
    this.shapes.set(key, shape)
    this.paintPose(key)
    // Looking at another version of a picture only widens it about its middle, its height being v1's: that moves nothing. A new height does,
    // as any box's: a picture placed before it was measured (16:9 until then) pushes aside what it now lies on (cl-885).
    const picture = view.asset?.kind === "image" && !view.hereWords && shownOf(view)
    const resized = before?.slice(before.indexOf(":")) !== shape.slice(shape.indexOf(":"))
    if (!picture || resized) this.unsettle(key, "grown")
  }

  // Keeping clear: no box lies on another

  /** What a box takes on the canvas, as one key: the same from its first node on, so only a real change moves anything. */
  private shapeOf(name: string, width: number, height: number) {
    const view = this.get(name)
    return `${view ? bodyWidth(view, width) : width}:${height}:${view ? underOf(view) : 0}`
  }

  /** A box and the room it keeps under it, which nothing else lies on: what hangs from it, and what the name of a box below rises into. */
  private laid(name: string, box: Box): Box {
    const view = this.get(name)
    if (!view) return box
    const drawn = bodyBox(view, box)
    return { ...drawn, h: drawn.h + underOf(view) }
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
    const node = id ? this.viewport?.getNode(id) : undefined
    const row = this.rows?.get(name)
    if ((!node && !row) || !view) return null
    // A row is what the page last said of it, and a node may not have heard yet.
    const box = this.poseOf(name)
    if (!box) return null
    return bare ? bodyBox(view, box) : this.laid(name, box)
  }

  private field(besides?: Set<string>, bare = false): Field {
    const field = new Field()
    for (const name of this.ids.keys()) {
      const box = besides?.has(name) ? null : this.boxOf(name, bare)
      if (box) field.add(name, box)
    }
    return field
  }

  /** Its height at a width as drawn, stack and all, when what it shows keeps its shape as it narrows: a box that may shrink to fit. A picture
   *  showing another take is drawn at that take's width (`bodyWidth`): its height is its row's at the width that draws it that wide. */
  private tallOf(name: string): ((w: number) => number) | undefined {
    const view = this.views.get(name)
    if (!view) return undefined
    const per = bodyWidth(view, 1)
    const tall = (w: number) => this.laid(name, { x: 0, y: 0, w: w / per, h: heightFor(view, w / per) }).h
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
    this.viewport?.updateNode(id, { gliding: true })
    this.paintPose(name)
    this.gliding.set(id, setTimeout(() => this.viewport?.updateNode(id, { gliding: false }), GLIDE_MS))
  }

  /** Slides to `to`, at its width, and that's its row: a shrink sticks as a pulled corner does. */
  private slide(name: string, to: Box) {
    const id = this.ids.get(name)
    if (!id) return
    const view = this.get(name)
    const canonical = view ? canvasBox(view, to) : to
    const row = { x: Math.round(canonical.x), y: Math.round(canonical.y), w: Math.round(canonical.w) }
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

  /** It gives way: it stays and pushes aside what it lies on, or moves or shrinks, as `give` says. Where it is now. With `bare`, the boxes
   *  themselves without the room each keeps: where nowhere near has its room but the box itself lies on none of them, it stays, closer than
   *  its room, not sent beside everything, off past a crowd's edge. */
  private giveWay(name: string, field: Field, box: Box, give: Give, bare?: Field): Box {
    const { pushed, beside, ...fitted } = fit(box, field, give)
    const itself = beside && bare ? this.boxOf(name, true) : null
    if (itself && !bare!.under(itself).size) return box
    this.push(field, pushed)
    if (fitted.x === box.x && fitted.y === box.y && fitted.w === box.w) return box
    this.slide(name, fitted)
    return fitted
  }

  /** `name` to settle: `placed` by another writer (it wins its spot), or `grown` where it was. Placed and grown, it's placed. */
  private unsettle(name: string, why: "placed" | "grown") {
    if (!this.seen) return
    if (!this.unsettled.size) queueMicrotask(() => this.settle())
    if (this.unsettled.get(name) !== "placed") this.unsettled.set(name, why)
  }

  /**
   * What arrived lying on something, put there by another writer or grown there, keeps where it was put and pushes aside what it lies on
   * (Paul, 11-06: "the placement position of the new object should be primary, and then other objects should move around it"; cl-800),
   * what arrived or grew with it too, one by one: what was placed first, so it wins its spot, then what grew, each in name order, so every
   * tab does the same. Where what it lies on can't be pushed (held in a drag, open, still being made), it gives way itself: no box ever lies
   * on another. What's held in a drag, or by its corner, waits to be let go.
   */
  private settle() {
    const batch = [...this.unsettled]
      .filter(([name]) => this.ids.has(name) && !this.dragging.has(name) && name !== this.pulling?.name)
      .sort(([a, why], [b, other]) => (why === other ? (a < b ? -1 : 1) : why === "placed" ? -1 : 1))
      .map(([name]) => name)
    this.unsettled.clear()
    if (!batch.length) return
    const [field, bare] = [this.field(), this.field(undefined, true)]
    for (const name of batch) {
      const box = this.boxOf(name)
      if (!box) continue
      field.remove(name)
      bare.remove(name)
      const placed = this.giveWay(name, field, box, { pushes: this.pushable(new Set([name])) }, bare)
      field.add(name, placed)
      const itself = this.boxOf(name, true)
      if (itself) bare.add(name, itself)
    }
  }

  /**
   * Opened: what lies on something comes apart in the smallest moves, the later name moving, and that's saved. Only where things sit changes.
   * Only boxes themselves: the room each keeps around it is for placing and settling, so a project laid out closer than that stays as it is.
   */
  private separate() {
    const field = this.field(undefined, true)
    for (const name of [...this.ids.keys()].sort()) {
      const box = this.boxOf(name, true)
      if (!box) continue
      const view = this.get(name)
      if (view?.asset?.kind === "image" && !view.hereWords && shownOf(view)) {
        field.add(name, box)
        continue
      }
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

  /** By box id, the take a render asked here looks at once it goes: none once the person looks at a take first. */
  private aims = new Map<string, string>()

  /** The person looked at one of `name`'s takes: a render of it not gone yet leaves that look alone. */
  private looked(name: string) {
    const id = this.ids.get(this.resolveName(name))
    if (id) this.aims.delete(id)
  }

  /** Looks at version `number`, or with none its state again. Nothing leaves this page. */
  view(name: string, number?: number) {
    this.patch(name, { viewed: number })
    this.reshape(name)
  }

  /** Looks at take `number`, and the first `<self.N>` its words edit becomes that take. */
  pickTake(name: string, number: number) {
    const asset = this.get(name)?.asset
    if (!asset) return
    this.looked(name)
    this.patch(name, { watch: undefined, ...(["image", "video"].includes(asset.kind) ? { hereWords: undefined, continuing: undefined, continuingFps: undefined } : {}) })
    this.view(name, number === asset.version ? undefined : number)
    if (asset.kind === "image") return
    if (!asset.tool || asset.timeline) return
    const { field } = this.selfOf(asset)
    const words = this.valueOf(name, field)
    if (typeof words !== "string") return
    const next = words.replace(/<(self)\.\d+>/i, (_, self) => `<${self}.${number}>`)
    if (next !== words) this.setDraft(name, field, next)
  }

  /** Selects the node's persistent current image; ordinary references follow this version. */
  selectVersion(name: string, number: number) {
    const key = this.resolveName(name)
    const view = this.views.get(key)
    const asset = view?.asset
    if (!asset || !numbersOf(asset).includes(number)) return
    if (asset.kind === "image" && this.inPlace?.name === key) this.inPlace.moved = true
    void this.save(key)
    this.looked(key)
    this.patch(key, { viewed: undefined, watch: undefined, ...(asset.kind === "image" ? { hereWords: undefined } : {}) })
    if (asset.pinned && asset.approved === number) return
    const edit = this.step(`Selected version ${number} of ${key}`, [key])
    this.over(key, { pinned: true, approved: number, version: number }, (served, write) => this.client.approve(this.projectId!, served, String(number), write), "select that version of", edit)
    if (view?.hereWords && asset.tool) {
      const { field } = this.selfOf(asset)
      const words = this.valueOf(key, field)
      if (typeof words === "string") {
        const next = words.replace(/<(self)\.\d+>/i, (_, self) => `<${self}.${number}>`)
        if (next !== words) this.setDraft(key, field, next)
      }
    }
  }

  beginImageEdit(name: string) {
    const key = this.resolveName(name)
    const view = this.views.get(key)
    if (!view?.asset?.tool) return
    if (view.asset.kind === "image" && this.inPlace?.name === key) this.inPlace.moved = true
    if (view.here) this.writeNext(key, true)
    else {
      this.writeOnOpen = key
      this.openHere(key)
    }
  }

  finishImageEdit(name: string) {
    void this.save(name)
    this.patch(name, { hereWords: undefined })
  }

  /** Redo from a seam of the take shown (`frame`, se-130): its authoring fields restored, every segment before it kept. */
  async beginVideoPickup(name: string, frame: number) {
    const key = this.resolveName(name)
    const view = this.get(key)
    const shown = view && shownOf(view)
    const tool = this.tools.get().get(view?.asset?.tool ?? "")
    if (!view?.asset || shown?.field !== "video" || shown.number === undefined || !tool?.pickup || !this.projectId) return
    const number = shown.number
    try {
      const source = (await this.client.asset(this.projectId, key)).versions.find((version) => version.number === number)
      const now = this.get(key)
      if (!now || shownOf(now)?.number !== number) return
      const fields = source && "fields" in source ? source.fields : undefined
      if (fields && typeof fields === "object" && !Array.isArray(fields)) {
        const saved = fields as Record<string, unknown>
        for (const field of new Set([...tool.fields.map((field) => field.field), "models", "lora"])) this.setDraft(key, field, saved[field] ?? null)
      }
      const fps = shown.fps ?? FPS
      const cut = pickupAt(frame, framesIn(shown.duration_ms, fps), tool.pickup)
      this.focusFit?.()
      this.patch(key, { hereWords: true, watch: undefined, continuing: cut ? { take: `<${key}.${number}>`, out_frame: cut, beats_from: cut / fps } : undefined, continuingFps: fps })
      this.focusing = key
    } catch (error) { say("Couldn't load this video's prompts", error) }
  }

  /** Continue another box's take from its end, a seam too (cl-951): its words are this box's own, from their start. */
  continueFrom(name: string, from: string, number: number) {
    const key = this.resolveName(name)
    const file = this.fileOf(`${from}.${number}`, key)
    const snap = this.tools.get().get(this.get(key)?.asset?.tool ?? "")?.pickup
    if (!snap || file?.field !== "video") return
    const fps = file.fps ?? FPS
    const last = framesIn(file.duration_ms, fps)
    // H3's takes end on its grid: one that doesn't goes on from the grid's last frame in it.
    const end = pickupAt(last, last, snap)
    if (end === null) return say(`Take ${number} of ${from} is too short for this tool to continue from`)
    this.patch(key, { hereWords: true, watch: undefined, continuing: { take: `<${mentionFrom(key, from, number)}>`, out_frame: end, beats_from: 0 }, continuingFps: fps })
    this.openHere(key)
  }

  /** The one playback a video in Focus has: its transport and track drive it, its player plays it (cl-965). */
  playbackOf(name: string, fps: number): VideoPlayback {
    const key = this.resolveName(name)
    let held = this.playbacks.get(key)
    if (!held) this.playbacks.set(key, (held = new VideoPlayback(fps)))
    held.fps = fps
    return held
  }

  /** Back to a plain draft: nothing kept from a take. */
  stopContinuing(name: string) {
    this.patch(this.resolveName(name), { continuing: undefined, continuingFps: undefined })
  }

  /** Every other box's video takes, newest box first: what a box can continue. */
  videoTakes(besides: string): Array<{ name: string; number: number }> {
    const key = this.resolveName(besides)
    return [...this.views.values()]
      .filter((view) => view.name !== key && view.asset?.kind === "video" && !view.asset.timeline)
      .flatMap((view) => (view.asset?.prefixes ?? []).filter((held) => held.main?.field === "video").map((held) => ({ name: view.name, number: held.number })))
  }

  /** Looks at a take still being made, by its id: its tile is picked, and its own progress fills the frame, a video's too (Paul, 11-12).
   *  `words`: its words stay over it, as a video's do once it's rendered. */
  watchTake(name: string, take: string, words = false) {
    const asset = this.get(name)?.asset
    if (!asset) return
    this.looked(name)
    const run = asset.takes?.find((held) => held.id === take)?.run ?? undefined
    this.patch(name, { hereWords: words || undefined, viewed: undefined, watch: take, glimpse: undefined, ...(run !== undefined && this.runs.has(run) ? { run: this.runs.get(run) } : {}) })
  }

  /** What a render of `name` leaves out of what its box mentions, in one note (Paul, 11-16, of a character in LTX-2.5's words-only prompt:
   *  "it did not do anything with the character … complete fail"). His input ignored is no handled fallback: it's said. */
  private sayLeftOut(name: string, asset: Asset) {
    const lines = new Set((asset.unused ?? []).map((one) => one.why))
    if (lines.size) say(`${name}: ${[...lines].join("; ")}`)
  }

  seek(name: string, at: number) {
    const held = this.get(name)
    if (held) this.patch(name, { seek: { at, ask: (held.seek?.ask ?? 0) + 1 } })
  }

  /**
   * A new take or version: the box says Queued now, and anything typed goes first. What Calcifer runs itself needs no machine; with none up for the rest, it asks
   * for one and goes when one is. Its key is made here, once, so however often this is tried it asks one render.
   */
  run(name: string, also: Also = {}) {
    const key = this.resolveName(name)
    const view = this.views.get(key)
    const asset = view?.asset
    if (!asset) return
    // What its words mention that its tool can't use: said as Render is pressed, once, not only struck through (cl-1066).
    if (!also.take) this.sayLeftOut(key, asset)
    if (!also.pickup && view.continuing) also = { ...also, pickup: view.continuing }
    // Its take has its id from the click, and is listed under it at once (cl-945). However often this goes on, after a question or a
    // machine coming up, it's that take.
    const take = also.take ?? newTakeId()
    const id = this.ids.get(key)
    if (!also.take && id) this.aims.set(id, take)
    also = { ...also, take }
    if (!view.taking?.some((held) => held.id === take)) this.patch(key, { taking: [...(view.taking ?? []), { id: take, until: NOT_YET }] })
    const local = madeHere(asset.tool)
    const on = also.on ? [also.on] : local ? ["local"] : this.machines()
    const saved = this.save(key)
    if (!on.length) {
      this.patch(key, { waiting: also })
      this.askMachine()
      return
    }
    // From an image tool's words, the box looks at the take being made, and its + goes back to them. Not before it goes: what it waits
    // for or asks first is said under its words, and a take the person looks at meanwhile stays the one looked at.
    const aimed = !id || this.aims.get(id) === take
    if (aimed && id) this.aims.delete(id)
    if (aimed && (asset.kind === "image" || asset.kind === "video")) this.watchTake(key, take, asset.kind === "video")
    this.told.delete(key)
    this.patch(key, { waiting: undefined, scrub: undefined, glimpse: undefined, asked: NOT_YET })
    // Answered, its take is Calcifer's to list from a read begun after; not sent, it's no take.
    const settle = (asked?: number) => {
      this.patchId(id, { asked })
      this.untake(id, take, asked)
    }
    const once = `take-${crypto.randomUUID()}`
    this.act(key, {
      // None named but here: Calcifer places it, as a `cozy run` naming none.
      send: async (served, write) => ((await saved) ? this.client.render(this.projectId!, [served], local || also.on ? on : [], once, { pickup: also.pickup, takes: [take], ...(also.agreed ? { agreed: also.agreed } : {}) }, write.once) : null),
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

  /** What a render of `name` on `on` (`new[:sku]`) would rent, and its price, as it stands once what's typed is saved: null, it sends nothing to
   *  a machine (cl-1066). Its Render on a new rental agrees to it (`run` with `on` and `agreed`). */
  async renderQuote(name: string, on: string): Promise<RunQuote | null> {
    const key = this.resolveName(name)
    const id = this.ids.get(key)
    if (!this.projectId || !(await this.save(key))) throw new Error(`${name} isn't saved yet`)
    return this.client.renderQuote(this.projectId, [this.served.get(id ?? "") ?? key], on)
  }

  /** `name`'s render failed, said. One its machine's shutdown stopped offers Retry (Paul, 11-16, cl-1085): "you push the retry button and
   *  we'll try to place that run on a different machine". */
  private failed(name: string, failure: ErrorBody) {
    if (failure.code !== MACHINE_LOST) return void say(`${name} didn't render`, new ApiError(0, { ...failure, message: failure.message.replace(`${name}: `, "") }))
    say(failure.message, new ApiError(0, failure), { action: { label: "Retry", run: () => this.retry(name) } })
  }

  /** Its render again, as Render asks one: placed where it finishes soonest on the machines up now, which the one that shut down isn't.
   *  With none up it waits for one, and says so, with Rent. */
  retry(name: string) {
    const key = this.resolveName(name)
    if (!this.views.get(key)?.asset) return
    if (!this.machines().length) {
      this.patch(key, { waiting: {} })
      say(`No machine is up to render ${name} on: rent one, and it renders there`, undefined, { action: { label: "Rent", run: () => this.rent() } })
      return
    }
    this.run(key)
  }

  /** An Edit's film joined here, free, from its cut as it stands now, as `cozy render` would: its next version, unless that cut has one. What's
   *  looked at stays as it is (cl-957). */
  join(name: string) {
    const key = this.resolveName(name)
    const asset = this.views.get(key)?.asset
    if (!asset?.timeline || !this.projectId) return
    const id = this.ids.get(key)
    const settle = (asked?: number) => this.patchId(id, { asked })
    this.told.delete(key)
    this.patch(key, { asked: NOT_YET })
    const once = `join-${crypto.randomUUID()}`
    this.act(key, {
      send: (served, write) => this.client.render(this.projectId!, [served], ["local"], once, {}, write.once),
      done: (answer) => {
        settle(answer ? nextProjectOrder() : undefined)
        this.readAfter([id])
      },
      refused: (error) => {
        settle()
        say(`Couldn't put ${name}'s video together`, error)
      },
    })
  }

  /** How long a render of `name` runs where it would go now, and a rented GPU far faster, by Calcifer (cl-1004): into `estimates`. */
  async estimate(name: string) {
    const key = this.resolveName(name)
    const id = this.ids.get(key)
    const project = this.projectId
    if (!id || !project) return
    const asked = (this.estimating.get(key) ?? 0) + 1
    this.estimating.set(key, asked)
    const fit = await this.client.fit(project, [this.served.get(id) ?? key]).catch(() => null)
    if (this.estimating.get(key) !== asked || this.projectId !== project) return
    const found = fit?.estimates?.[0]
    const next = new Map(this.estimates.get())
    if (found) next.set(key, found)
    else next.delete(key)
    this.estimates.set(next)
  }

  /** Its take `take`, asked from this page: Calcifer's to list from a read begun after `until`; with none, it was never sent, and it goes. */
  private untake(id: string | undefined, take: string | undefined, until?: number) {
    const name = id && this.namesById.get(id)
    const taking = name ? this.views.get(name)?.taking : undefined
    if (!name || !take || !taking?.some((held) => held.id === take)) return
    const left = until === undefined ? taking.filter((held) => held.id !== take) : taking.map((held) => (held.id === take ? { ...held, until } : held))
    this.patch(name, { taking: left.length ? left : undefined, ...(until === undefined && this.views.get(name)?.watch === take ? { watch: undefined } : {}) })
  }

  /** A machine came up: each render that waited for one goes. */
  machineUp() {
    for (const [name, view] of this.views) if (view.waiting) this.run(name, view.waiting)
  }

  /**
   * What holds a box's look until a read begun after the answer has to get that read: the write's own event can reach a read that began first, and then there's no
   * other. One read of these, whatever else is said.
   */
  private readAfter(boxes: (string | undefined)[]) {
    // Not one this page has deleted: it's gone, and a read could only 404.
    const names = boxes.flatMap((id) => this.served.get(id ?? "") ?? []).filter((name) => !this.deleting.has(name))
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

  /** `row` over what its row says, now, until a read begun after the answer says for itself, as `overlay`, and asked of Calcifer under `edit`. Refused, it goes, and the rest stays.
   *  With none yet, `send` shows it as it goes (`show`), or answers nothing when there's nothing to ask, and its step goes. */
  private over<T extends object>(name: string, row: Partial<Asset> | null, send: (served: string, write: Write, show: (row: Partial<Asset>) => void) => Promise<T | null>, what: string, edit: string | undefined, then?: { done?(answer: T): void; refused?(): void; gone?(): void }) {
    const key = this.resolveName(name)
    const view = this.views.get(key)
    if (!view?.asset) return
    const id = this.ids.get(key)
    const now = () => this.views.get((id && this.namesById.get(id)) || this.resolveName(key))
    let shown = row ? this.overlay(key, row) : undefined
    const show = (later: Partial<Asset>) => {
      const held = now()
      if (!shown && held) shown = this.overlay(held.name, later)
    }
    this.act<T | null>(key, {
      send: (served, write) => send(served, write, show),
      edit,
      done: (answer) => {
        if (!answer) return void (edit && this.history.forget(edit))
        if (shown) shown.ask.until = nextProjectOrder()
        this.readAfter([id])
        // A write of its own: its read back isn't news.
        const held = now()
        const etag = "etag" in answer && typeof answer.etag === "string" ? answer.etag : null
        if (held && etag) {
          this.own.set(held.name, (this.own.get(held.name) ?? new Set()).add(etag))
          this.patch(held.name, { known: etag })
        }
        then?.done?.(answer)
      },
      refused: (error) => {
        shown?.drop()
        if (edit) this.history.forget(edit)
        say(`Couldn't ${what} ${name}`, error)
        then?.refused?.()
      },
      gone: () => {
        shown?.drop()
        if (edit) this.history.forget(edit)
        then?.gone?.()
      },
    })
  }

  /** One take or version off its box, at once, with no dialog: the strip closes up, an approved one's box stands on the newest, and a note offers Undo for a while. */
  discardTake(name: string, number: number) {
    const key = this.resolveName(name)
    const asset = this.views.get(key)?.asset
    if (!asset?.prefixes?.some((held) => held.number === number)) return
    // A picture box's only picture, with no tool to make another: its × deletes the box, as a person means, and nothing of it stays (cl-919).
    if (!asset.tool && asset.kind !== "text" && asset.prefixes.length === 1 && !asset.making?.length) {
      const open = this.inPlace?.name === key
      this.remove([key])
      // Gone first, so nothing draws its picture on the way back to the canvas.
      if (open) this.leaveHere()
      return
    }
    const edit = this.step(`Discarded ${asset.noun} ${number} of ${key}`, [key])
    let note = 0
    this.dropTake(key, number, edit, () => dismiss(note))
    const stands = asset.approved === number ? ` It was the approved one, so ${name} now shows its newest.` : ""
    const readers = (this.state?.assets ?? []).filter((other) => other.name !== key && other.reads?.some((read) => read.name === key && read.version === number)).map((other) => other.name)
    const read = readers.length ? ` ${readers.join(", ")} ${readers.length === 1 ? "uses" : "use"} it and can't render until it's back.` : ""
    note = this.offer(edit, `Discarded ${asset.noun} ${number} of ${name}.${stands}${read}`)
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
    for (const one of readers) void this.write(one.name, one.next, `update the prompt in ${one.name}`, { edit })
    const stands = asset.pinned && asset.approved === number ? ` It was the approved one, so ${from} now follows its inputs.` : ""
    const read = readers.length ? ` ${readers.map((one) => one.name).join(", ")} now ${readers.length === 1 ? "uses" : "use"} ${to}.` : ""
    note = this.offer(edit, `Moved ${asset.noun} ${number} of ${from} into its own box, ${to}.${stands}${read}`)
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
   * it by. `merged`: the edit of a box merging in, whose own note says it.
   */
  joinVersion(name: string, file: Pick<MainFile, "sha256" | "field"> & Partial<MainFile>, source: string, what: string, merged?: string): boolean {
    const key = this.resolveName(name)
    const asset = this.views.get(key)?.asset
    if (!asset) return false
    const next = nextVersion(asset, file)
    if (!next) {
      say(`${name} already has that image`)
      return false
    }
    const edit = merged ?? this.step(`Added ${what} to ${key}`, [key])
    let note = 0
    this.over<VersionMoved>(key, next.row, (served, write) => this.client.addVersion(this.projectId!, served, source, write), "add to", edit, {
      done: (answer) => {
        if (answer.already) dismiss(note)
        if (!answer.already) this.view(key, answer.number)
      },
      refused: () => dismiss(note),
    })
    this.view(key, next.number)
    if (merged === undefined) note = this.offer(edit, `Added ${what} to ${name}`)
    return true
  }

  /**
   * A file dropped on `name`, its next version. Its step and its turn are taken as it's dropped, so what's done after it lands after it, Calcifer
   * there or not; it shows, and goes, once `held` has its bytes in the project, which is what Calcifer finds it by. `ended`: it's off its way.
   */
  dropVersion(name: string, what: string, field: string, held: Promise<string>, ended: () => void) {
    const key = this.resolveName(name)
    let off = false
    const end = () => {
      if (!off) ended()
      off = true
    }
    // Its failure is said as its turn comes.
    held.catch(() => {})
    if (!this.views.get(key)?.asset) return end()
    const id = this.ids.get(key)
    const edit = this.step(`Added ${what} to ${key}`, [key])
    let note = 0
    let asked: string | undefined
    this.over<VersionMoved>(key, null, async (served, write, show) => {
      if (asked === undefined) {
        const source = await held.finally(end)
        const view = this.views.get((id && this.namesById.get(id)) || this.resolveName(key))
        const next = view?.asset && nextVersion(view.asset, { sha256: source.slice("sha256:".length), field })
        if (!view || !next) {
          say(view ? `${view.name} already has that image` : `Couldn't add ${what} to ${key}: the box is gone`)
          return null
        }
        show(next.row)
        this.view(view.name, next.number)
        note = this.offer(edit, `Added ${what} to ${view.name}`)
        asked = source
      }
      return this.client.addVersion(this.projectId!, served, asked, write)
    }, "add to", edit, {
      done: (answer) => {
        const now = (id && this.namesById.get(id)) || this.resolveName(key)
        if (answer.already) dismiss(note)
        else if (this.views.has(now)) this.view(now, answer.number)
      },
      refused: () => {
        end()
        dismiss(note)
      },
      gone: () => {
        end()
        dismiss(note)
        say(`Couldn't add ${what} to ${key}: the box is gone`)
      },
    })
  }

  /** A take let go on a box: one with no tool takes it as a version, anything else with words mentions it. Whether anything did. */
  takePicture(name: string, file: CarriedFile): boolean {
    const key = this.resolveName(name)
    const asset = this.views.get(key)?.asset
    if (!asset || asset.timeline || madeHere(asset.tool)) return false
    if (!asset.tool && asset.kind !== file.field) {
      say(`${key} is ${an(asset.kind)} ${asset.kind} box, so it can't take ${an(file.field)} ${file.field}`)
      return true
    }
    const own = file.from !== undefined && nameKey(file.from) === nameKey(key)
    // Another box's take on one that picks up: it continues that take from its end (cl-951).
    if (!own && file.from && file.number && file.field === "video" && this.tools.get().get(asset.tool ?? "")?.pickup) {
      this.continueFrom(key, file.from, file.number)
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

  /** `tool` goes on it, or takes the place of the one there. What it can't take is said, with Undo. A tool of another kind that reads it makes a
   *  new box from it (`madeFrom`); one that can't changes nothing and says why (tools §9). */
  putTool(name: string, tool: Tool) {
    const key = this.resolveName(name)
    const view = this.views.get(key)
    if (!view?.asset) return
    if (view.asset.tool === tool.id) return
    const wont = wontGoOn(tool, view.asset, key)
    if (wont) return void say(wont)
    if (readsBox(tool, view.asset)) return void this.madeFrom(key, view.asset, tool)
    void this.save(key)
    const step = goingOn(view.asset, { ...view.asset.fields, ...view.sent, ...view.drafts }, this.tools.get().get(view.asset.tool ?? ""), tool, view.viewed)
    this.retool(key, step, step.lost.length ? `${name} now uses ${toolName(tool)}, which doesn't take ${step.lost.join(" or ")}` : undefined)
    if (tool.makes === "image" && holdsPictures(view.asset)) this.patch(key, { hereWords: undefined })
  }

  /** `tool` let go on box `from` of another kind it reads (xs-204, Paul 11-16: "attach an LLM … to an image node … and ask it to produce a
   *  caption"): a new box of what it makes, right of it, reading it where Calcifer says (`reads`), opened on its words so the caret is there to
   *  ask. One step: Undo takes it away. */
  private madeFrom(from: string, asset: Asset, tool: Tool) {
    const pose = this.poseOf(from)
    const project = this.projectId
    if (!pose || !project) return
    const starting = { ...startingFields(tool), node_kind: tool.makes }
    // Its mention as Calcifer writes it (`reads`), sent too: it writes none twice.
    const fields = { ...starting, ...startsFrom(tool, asset.kind, starting, `<${from}>`, true) }
    // Made here, it holds none of what its tool's rules choose yet: they're brought in (cl-1281).
    const row: Asset = { name: "", standing: "missing", kind: tool.makes, noun: tool.makes === "video" ? "take" : "version", tool: tool.id, versions: 0, status: "", fields, chosen: [] }
    const at = { x: Math.round(pose.x + pose.w + GAP_BESIDE), y: Math.round(pose.y) }
    const once = `asset-${crypto.randomUUID()}`
    const name = this.place(asName(tool.name || tool.label), { asset: row }, at, (clear, write, pushes) =>
      this.client.newAsset(project, once, { tool: tool.id, name: asName(tool.name || tool.label), fields, kind: tool.makes, reads: from, at: clear, moves: pushes.moves, placed: pushes.placed }, write),
    (error) => say(`Couldn't make a box from ${from}`, error))
    // In Focus the canvas isn't there to open it on: it's said instead.
    if (this.focusedName.get()) say(`Made ${name} from ${from}`)
    else this.openHere(name)
  }

  /** Its tool's X leaves a typed container and its words. A note offers Undo for a while. */
  takeToolOff(name: string) {
    const key = this.resolveName(name)
    const view = this.views.get(key)
    if (!view?.asset) return
    void this.save(key)
    const known = this.tools.get().get(view.asset.tool ?? "")
    const word = known ? toolName(known) : "the tool"
    const words = holdsPictures(view.asset) ? "Its images stay." : `It's still ${an(view.asset.kind)} ${view.asset.kind} box.`
    this.retool(key, comingOff(view.asset, { ...view.asset.fields, ...view.sent, ...view.drafts }, known), `Took ${word} off ${name}. ${words}`)
  }

  private retool(key: string, step: ToolStep, said?: string) {
    const edit = this.step(said ?? `Changed ${key}'s tool`, [key])
    let note = 0
    this.over(key, step.row, (served, write) => this.client.patchAsset(this.projectId!, served, { tool: step.tool, fields: step.fields }, write), "change the tool on", edit, {
      done: (answer) => answer.warnings?.forEach((line) => say(line)),
      refused: () => dismiss(note),
    })
    if (!step.row.tool) this.patch(key, { hereWords: undefined })
    this.reshape(key)
    if (said) note = this.offer(edit, said)
  }

  /** Approves it: this version is what `name` is, until it's unapproved. It turns into that file now, and can flip back. `edit`: what it's part of. */
  pin(name: string, number: number, edit?: string) {
    const key = this.resolveName(name)
    this.patch(key, { hereWords: undefined, viewed: undefined })
    const noun = this.views.get(key)?.asset?.noun ?? "version"
    const by = edit ?? this.step(`Approved ${noun} ${number} of ${key}`, [key])
    this.over(key, { pinned: true, approved: number, version: number }, (served, write) => this.client.approve(this.projectId!, served, String(number), write), "approve", by)
  }

  /** Starred, it leads the project's Files list (cl-892). A star is no input, and it's no approval. */
  star(name: string, on: boolean) {
    const key = this.resolveName(name)
    const edit = this.step(`${on ? "Starred" : "Unstarred"} ${key}`, [key])
    this.over(key, { starred: on }, (served, write) => this.client.patchAsset(this.projectId!, served, { fields: { starred: on || null } }, write), on ? "star" : "unstar", edit)
  }

  /** A double tap on a take toggles it (Paul, 11-12): it's approved, or if it was, nothing is, and it's still the one looked at. A picture's
   *  approval selects it as its box's picture (`selectVersion`). */
  toggleApproved(name: string, number: number) {
    this.looked(name)
    const asset = this.get(name)?.asset
    if (asset?.pinned && asset.approved === number) {
      this.unpin(name)
      this.view(name, number)
      return
    }
    if (asset?.kind === "image") return this.selectVersion(name, number)
    this.pin(name, number)
  }

  unpin(name: string) {
    const key = this.resolveName(name)
    const edit = this.step(`Unapproved ${key}`, [key])
    this.over(key, { pinned: false, approved: undefined }, (served, write) => this.client.unpin(this.projectId!, served, write), "unapprove", edit)
  }

  /** Take `number` of `name` was looked at: its dot goes, in this person's own state, never the project's (cl-928). */
  markSeen(name: string, number: number) {
    const asset = this.get(name)?.asset
    if (!asset?.unseen?.includes(number)) return
    this.over(name, { unseen: asset.unseen.filter((held) => held !== number) }, (served, write) => this.client.seen(this.projectId!, served, number, write).then(() => ({})), `mark ${asset.noun} ${number} as seen on`, undefined)
  }

  /** Takes landed and not said yet: said together once none has landed for a moment (cl-937). */
  private landedNow: { name: string; id?: string; number: number; sha256?: string }[] = []
  private landedTimer = 0

  private sayLater(name: string, id: string | undefined, number: number, sha256?: string) {
    this.landedNow.push({ name, id, number, sha256 })
    clearTimeout(this.landedTimer)
    this.landedTimer = window.setTimeout(() => this.sayLanded(), LANDED_MS)
  }

  /** Take `number` of `name` is the one Focus shows. */
  private lookedAt(name: string, number: number): boolean {
    const view = this.get(name)
    return Boolean(view && this.focusedName.get() === this.resolveName(name) && shownOf(view)?.number === number)
  }

  /** What landed, said (Paul, 11-12: "so I can view the result and I know when it's done"): one take with its picture and View, which opens
   *  its box on it, and that marks it seen; several at once as one note. One looked at as it landed, or since, isn't said. */
  private sayLanded() {
    const landed = this.landedNow.splice(0).filter(({ name, number }) => this.get(name) && !this.lookedAt(name, number))
    if (!landed.length) return
    const [{ name, id, number, sha256 }] = landed
    // That take, by its id (cl-945), whatever has landed since.
    const view = { label: "View", run: () => {
      this.openHere(name)
      this.pickTake(name, this.get(name)?.asset?.prefixes?.find((held) => id && held.id === id)?.number ?? number)
    } }
    if (landed.length > 1) return void say(`${landed.length} renders are ready: ${[...new Set(landed.map((one) => one.name))].join(", ")}`, undefined, { action: view })
    const noun = this.get(name)?.asset?.noun ?? "take"
    say(`${noun.charAt(0).toUpperCase()}${noun.slice(1)} ${number} of ${name} is ready`, undefined, { action: view, picture: sha256 && this.projectId ? { project: this.projectId, sha256 } : undefined })
  }

  /** Stops one of its takes, by its id (cl-945), gone from the box at once, and its others go on: one sent, its run; one queued, that take. */
  cancelTake(name: string, take: Taking) {
    const view = this.get(name)
    const asset = view?.asset
    const project = this.projectId
    if (!view || !asset || !project) return
    if (watched(view)?.id === take.id) this.patch(name, { watch: undefined })
    const taking = view.taking?.filter((held) => held.id !== take.id)
    if (taking?.length !== view.taking?.length) this.patch(name, { taking: taking?.length ? taking : undefined })
    if (take.id) this.patch(name, { gone: [...(view.gone ?? []), take.id] })
    // One with an id is off by it (`gone`), and what's listed after stays as read; a run no take names is off its runs until a read.
    const making = take.id ? {} : { making: asset.making?.filter((number) => number !== take.run) }
    const run = take.run
    const send = run !== undefined ? (_: string, write: Write) => this.client.cancelRun({ project, run }, write.once).then(() => ({})) : (_: string, write: Write) => this.client.cancelTakes(project, [take.id], write.once).then(() => ({}))
    this.over(name, making, send, run !== undefined ? `cancel run ${this.runName(run)} of` : "cancel a take of", undefined)
  }

  /** Its render stops, wherever it is: what's queued leaves, its runs are canceled (as `cozy run cancel`), and it says Cancelling… until
   *  they end. No question is asked: Undo renders it again. */
  cancelRender(name: string) {
    const busy = Boolean(this.get(name)?.asset?.making?.length || this.get(name)?.queued)
    this.patch(name, { asked: undefined, taking: undefined, canceling: busy || undefined, ...(this.get(name)?.asset?.kind === "image" ? { watch: undefined } : {}) })
    this.act(name, {
      send: (served, write) => this.client.cancelRender(this.projectId!, [served], write.once),
      done: () => say(`Canceled ${name}'s render`, undefined, { action: { label: "Undo", run: () => this.run(name) } }),
      refused: (error) => {
        this.patch(name, { canceling: undefined })
        say(`Couldn't cancel ${name}`, error)
      },
    })
  }

  /** A fetch ahead's row as it changed (`ahead.updated`). */
  applyAhead(row: Ahead) {
    this.ahead.set(new Map([...this.ahead.get(), [row.run, row]]))
  }

  /** What's being prepared for box `name` while it waits for its turn: the newest fetch ahead that names it. */
  preparing(name: string): Ahead | undefined {
    const key = nameKey(name)
    let newest: Ahead | undefined
    for (const row of this.ahead.get().values()) if (row.names?.some((held) => nameKey(held) === key) && (!newest || row.run > newest.run)) newest = row
    return newest
  }

  /** The box run `number` makes, where this project has it. */
  boxOfRun(number: number): string | undefined {
    return this.owners.get(number)
  }

  /** The boxes ahead of `name` in its project's line, the first going first. */
  aheadOf(name: string): string[] | undefined {
    const queue = this.state?.queue ?? []
    const at = queue.findIndex((item) => item.name === name)
    return at < 0 ? undefined : queue.slice(0, at).map((item) => item.name)
  }

  private removeNode(name: string) {
    const id = this.ids.get(name)
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
    this.notify(name)
    if (id) this.viewport?.removeNodes([id])
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
    const row = this.rows?.get(from)
    if (row) {
      this.rows!.delete(from)
      this.rows!.set(to, row)
    }
    if (this.inPlace?.name === from) {
      this.inPlace.name = to
      this.hereRenamed(to)
    }
    if (this.focusedName.get() === from) this.focusedName.set(to)
    // What's kept of its words for a reload goes under the name Calcifer has for it now.
    this.keepWords(id)
    this.notify(to)
    this.viewport?.updateNode(id, (node) => ({ data: { ...node.data, name: to } }))
  }

  // A node's inputs

  /** What a control shows: what's typed, else what was sent, else what it holds. */
  valueOf(name: string, field: string): unknown {
    return valueIn(this.get(name), field)
  }

  /** The project document's heads `name` was read at. */
  private headsOf(name: string): string[] {
    return this.state?.headsOf?.[name] ?? this.state?.heads ?? []
  }

  /** Typing begins in `field` of `key`: what its words go as splices of, the field as read and its heads, unless it's kept them already. Only a
   *  read the box's row stands on: one this page made and hasn't read since (`pending`) isn't in those heads, and splices on them merge with
   *  its words as if typed on nothing, in either order (xs-204: " write a caption<photo>"); a field with a write on its way is past them. Those
   *  go whole, and the write's answer is what typing goes on from (`typedAnswered`). */
  private begin(key: string, field: string) {
    const id = this.ids.get(key)
    const view = this.views.get(key)
    if (!id || !view || this.typedFrom.get(id)?.has(field)) return
    if (view.pending !== undefined || (view.sent && field in view.sent)) return
    const heads = this.headsOf(this.served.get(id) ?? key)
    if (!heads.length) return
    this.typedFrom.set(id, (this.typedFrom.get(id) ?? new Map()).set(field, { value: view.asset?.fields?.[field] ?? null, heads }))
  }

  /** A write of `fields` of box `id`: what of its words changed as splices on what they began from, wherever they are in it; the rest as it is.
   *  A field with a write already on its way from there goes whole: the same splices twice would land twice (the last flush as the page goes, and
   *  the save its text's blur asks after it). */
  private body(id: string, fields: Record<string, unknown>, view?: ElementView, asking = true): AssetPatch {
    const from = this.typedFrom.get(id)
    const plain: Record<string, unknown> = {}
    const typed: Record<string, Splice[]> = {}
    const to: Record<string, string> = {}
    let heads: string[] | undefined
    for (const [field, value] of Object.entries(fields)) {
      const base = from?.get(field)
      const local = view?.sent && field in view.sent ? view.sent[field] : value
      const changed = base && base.sent === undefined && (!heads || heads.join() === base.heads.join()) ? wordsChanged(base.value, local) : null
      if (base && asking) base.sent = local
      if (!base || !changed) {
        plain[field] = value
        continue
      }
      heads = base.heads
      for (const words of changed) {
        const path = words.path ? `${field}.${words.path}` : field
        typed[path] = [spliceOf(words.from, words.to)]
        to[path] = words.to
      }
    }
    return { fields: plain, ...(heads ? { typed: { heads, fields: typed, to } } : {}) } as AssetPatch
  }

  /** Calcifer answered a write of box `id`: what each field typed in came to is what typing goes on from, and what was typed meanwhile is moved onto
   *  it. `whole`: what was typed and went whole, with no read to begin from (`begin`). */
  private typedAnswered(id: string, answer: AssetPatched, whole?: Record<string, unknown>) {
    if (!answer.heads?.length) return
    const from = this.typedFrom.get(id) ?? new Map<string, { value: unknown; heads: string[]; sent?: unknown }>()
    const name = this.namesById.get(id)
    for (const [field, merged] of Object.entries(answer.fields ?? {})) {
      if (!from.has(field) && whole && field in whole) from.set(field, { value: whole[field], heads: answer.heads, sent: whole[field] })
      const base = from.get(field)
      if (!base || base.sent === undefined) continue
      const view = name ? this.views.get(name) : undefined
      // What was sent as Calcifer keeps it: the space it cuts after the last word is no one's edit. Taken for one, it was merged into what
      // was typed since, and a space typed there went (e2e 44122550502: "A " sent, then "Aharbour at dusk.").
      const kept = endTrimmed(base.sent)
      if (name && view && !same(merged, kept)) {
        const onto = (local: unknown) => mergeValue(kept, local, merged)
        const sent = view.sent && field in view.sent ? { sent: { ...view.sent, [field]: onto(view.sent[field]) } } : {}
        const drafts = view.drafts && field in view.drafts ? { drafts: { ...view.drafts, [field]: onto(view.drafts[field]) } } : {}
        this.patch(name, { ...sent, ...drafts })
      }
      Object.assign(base, { value: merged, heads: answer.heads, sent: undefined })
    }
    if (from.size) this.typedFrom.set(id, from)
  }

  /** Someone's words, read: what each typed field goes on from, and what's typed and not yet on its way moved onto them. With a write on its way,
   *  its answer does that. */
  private typedOver(view: ElementView, asset: Asset): Partial<ElementView> {
    const id = this.ids.get(view.name)
    const from = id ? this.typedFrom.get(id) : undefined
    if (!from || view.writing) return {}
    const heads = this.headsOf(asset.name)
    let drafts = view.drafts
    for (const [field, base] of from) {
      const said = asset.fields?.[field] ?? null
      if (same(said, base.value) || !heads.length) continue
      if (drafts && field in drafts) drafts = { ...drafts, [field]: mergeValue(base.value, drafts[field], said) }
      Object.assign(base, { value: said, heads })
    }
    return drafts === view.drafts ? {} : { drafts }
  }

  /** What's typed shows at once and writes itself at the first pause, on leaving the text, or as its box shuts. */
  setDraft(name: string, field: string, value: unknown) {
    const view = this.get(name)
    if (!view) return
    const key = this.resolveName(name)
    this.begin(key, field)
    this.patch(name, { drafts: { ...view.drafts, [field]: value } })
    this.keepWords(this.ids.get(key))
    clearTimeout(this.pauses.get(name))
    this.pauses.set(name, setTimeout(() => void this.save(name), TYPING_PAUSE_MS))
  }

  /** Everything typed is written now, not at its pause; `going`, the page may be going, and each write goes on past it (`keepalive`). */
  flush(going = false) {
    for (const [name, view] of this.views) if (view.drafts) void this.save(name, going)
  }

  /** Box `id`'s last word, as it stands now: its words typed or on their way, as one write, with nothing marked as asked. None when nothing's owed. */
  private wordsNow(id: string): AssetPatch | null {
    const view = this.views.get(this.namesById.get(id) ?? "")
    if (!view?.asset || !(view.drafts || view.writing)) return null
    return this.body(id, { ...view.sent, ...view.drafts }, undefined, false)
  }

  /** Box `id`'s last word kept up to date, under a key of its own each time it changes: in a plain project, in this tab's storage too. */
  private keepWords(id: string | undefined) {
    const project = this.projectId
    if (!project || !id) return
    const body = this.wordsNow(id)
    const was = this.lastWords.get(id)
    const name = this.served.get(id) ?? this.namesById.get(id)
    if (body && name && was?.name === name && JSON.stringify(was.body) === JSON.stringify(body)) return
    const word = body && name ? { box: id, name, body, action: crypto.randomUUID(), at: Date.now() } : null
    if (word) this.lastWords.set(id, word)
    else this.lastWords.delete(id)
    if (word || was) keepLastWord(project, word && plainProject(project) ? word : null, was?.action)
  }

  /**
   * The page goes: each box's last word goes once more, one write a box, which goes on past the page (`keepalive`). What was on its way may not have
   * reached Calcifer, and what waited its turn never would. It goes under the key it's kept by, so the next load's send of it is the same ask.
   */
  private leaving() {
    const project = this.projectId
    if (!project || this.gone) return
    this.gone = true
    for (const id of new Set([...this.lastWords.keys(), ...[...this.views.keys()].map((name) => this.ids.get(name) ?? "")])) this.keepWords(id)
    for (const word of this.lastWords.values()) void this.client.patchAsset(project, word.name, word.body, { keepalive: lasts(word.body), action: word.action }).catch(() => {})
  }

  /** What this tab kept of what was typed before it last went, each sent again in its turn under its key: Calcifer does it once, whether or not it
   *  got out then. */
  private sendLastWords() {
    const project = this.projectId
    if (!project) return
    for (const word of lastWordsOf(project)) {
      this.outbox.add({
        send: (once) => this.client.patchAsset(project, word.name, word.body, { once, action: word.action }),
        done: () => keepLastWord(project, null, word.action),
        refused: (error) => {
          keepLastWord(project, null, word.action)
          say(`Couldn't save what you typed in ${word.name}`, error)
        },
      })
    }
  }

  /** Writes what's typed. Whether it stands: a refusal leaves it typed. Once the page has gone, its last flush said it all. */
  save(name: string, going = false): Promise<boolean> {
    clearTimeout(this.pauses.get(name))
    this.pauses.delete(name)
    if (this.gone) return Promise.resolve(true)
    const key = this.resolveName(name)
    const view = this.views.get(key)
    if (!view?.drafts) return Promise.resolve(true)
    return this.write(key, view.drafts, `save ${view.name}`, { typed: true, going })
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
   * `fields`, shown as written now, go in their turn, under `edit` (else a step of their own), over whatever those fields hold then: the other fields stay
   * as anyone left them. Whether they stand: refused, they're typed again.
   */
  private write(key: string, fields: Record<string, unknown>, what: string, opts: { edit?: string; typed?: boolean; going?: boolean } = {}): Promise<boolean> {
    const view = this.views.get(key)
    const id = this.ids.get(key)
    if (!view?.asset || !id) return Promise.resolve(true)
    const edit = opts.edit ?? this.writeStep(key, fields, opts.typed)
    const drafts = Object.fromEntries(Object.entries(view.drafts ?? {}).filter(([field]) => !(field in fields)))
    this.patch(key, { drafts: Object.keys(drafts).length ? drafts : undefined, sent: { ...view.sent, ...fields }, writing: (view.writing ?? 0) + 1 })
    this.keepWords(id)
    const now = () => this.views.get(this.namesById.get(id) ?? "")
    let asked: AssetPatch | undefined
    return new Promise((settled) => {
      this.act(key, {
        // Made once: a try again after a lost answer (a restart) is the same ask, which Calcifer answers once.
        send: (served, write) => {
          if (!asked) {
            asked = this.body(id, fields, now())
            this.keepWords(id)
          }
          return this.client.patchAsset(this.projectId!, served, asked, opts.going ? { ...write, keepalive: lasts(asked) } : write)
        },
        edit,
        done: (answer) => {
          this.typedAnswered(id, answer, opts.typed ? fields : undefined)
          const held = now()
          if (held) {
            this.wrote(held.name, answer)
            this.patch(held.name, { writing: (held.writing ?? 1) - 1 || undefined })
          }
          this.keepWords(id)
          settled(true)
        },
        refused: (error) => {
          for (const field of Object.keys(fields)) delete this.typedFrom.get(id)?.get(field)?.sent
          const held = now()
          if (held) {
            const sent = Object.fromEntries(Object.entries(held.sent ?? {}).filter(([field]) => !(field in fields)))
            this.patch(held.name, { drafts: { ...fields, ...held.drafts }, sent, writing: (held.writing ?? 1) - 1 || undefined })
            say(`Couldn't ${what}`, error)
          }
          this.keepWords(id)
          this.history.forget(edit)
          settled(false)
        },
      })
    })
  }

  /** A write of its own: its read back isn't news. */
  private wrote(key: string, answer: AssetPatched) {
    if (this.views.has(key)) this.patch(key, { known: answer.etag })
    this.own.set(key, (this.own.get(key) ?? new Set()).add(answer.etag))
    for (const warning of answer.warnings ?? []) say(warning)
  }

  /** A slider let go, a choice made: one field, whatever else is typed. */
  setField(name: string, field: string, value: unknown, edit?: string) {
    return this.setFields(name, { [field]: value }, edit)
  }

  /** A write of what keeps its own draft (an Edit's clips), in its turn like any other. */
  patchInTurn(name: string, fields: Record<string, unknown>): Promise<AssetPatched> {
    const key = this.resolveName(name)
    const edit = this.step(`Set ${Object.keys(fields).join(", ")} on ${key}`, [key])
    return new Promise((done, refused) => this.act(key, { send: (served, write) => this.client.patchAsset(this.projectId!, served, { fields }, write), edit, done, refused }))
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
    this.keepWords(this.ids.get(this.resolveName(name)))
  }

  /** What makes it, among those of its kind: its fields stay. */
  setTool(name: string, tool: string) {
    const key = this.resolveName(name)
    if (this.views.get(key)?.asset?.tool === tool) return
    const known = this.tools.get().get(tool)
    const label = known ? toolName(known) : tool.split(/[/\\]/).at(-1)
    const edit = this.step(`Switched ${key} to ${label}`, [key])
    this.over(key, { tool }, (served, write) => this.client.patchAsset(this.projectId!, served, { tool }, write), "change the tool on", edit, { done: (answer) => answer.warnings?.forEach((line) => say(line)) })
    this.reshape(key)
  }

  // Names in a text

  /** Each name a mention could be, for the `<` list: nodes only. One its tool can't use is struck through once written (cl-944). */
  namesLike(typed: string, besides: string): string[] {
    return this.mentionable(typed, besides)
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
    if (said.number === undefined) return selectedOf(view)
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
  dropTargetAt(x: number, y: number, dragged: Element[]): DropTarget | null {
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
      return this.targetFor(name)
    }
    const name = this.canvasAt(x, y)
    return name ? this.targetFor(name) : (this.setHot(null), null)
  }

  private targetFor(name: string): DropTarget | null {
    const view = this.views.get(name)
    if (view?.asset?.kind === "image" && view.asset.tool) {
      this.setHot(name)
      return { over: () => {}, drop: (names) => this.takeInto(name, names) }
    }
    if (view?.asset && holdsPictures(view.asset) && !view.asset.timeline && !madeHere(view.asset.tool)) {
      this.setHot(name)
      return { over: () => {}, drop: (names) => this.mergeBoxes(name, names) }
    }
    if (view && makes(view) && faceOf(view) === "edit") {
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

  /** Boxes let go on a box of pictures: each of its kind merges in as its newest take, which it shows, and is gone; what's approved stays. Any other a tool reads. */
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
    const given = [...new Set(names.map((name) => this.resolveName(name)))].filter((name) => name !== key)
    if (!asset || !given.length) return
    const tags = given.map((name) => `<${name}>`)
    const asText = (held: unknown) => [typeof held === "string" ? held.trimEnd() : "", ...tags].filter(Boolean).join(" ")
    const fields = fieldsOf(asset, this.tools.get())
    if (asset.timeline) {
      const clips = asset.timeline.tracks[0]?.clips ?? []
      const taken = given.flatMap((name) => clipOf(this, name, asset.timeline?.fps ?? 24) ?? [])
      if (taken.length < given.length) say("Only videos and images can go in an Edit for now")
      if (!taken.length) return
      void this.setFields(key, { timeline: wire([...clips, ...taken]) })
      // The cut changed: its film joined after the write, as the editor does (cl-957).
      this.join(key)
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
    if (wrong) return void say(`${slot.label} takes ${slot.accept === "image" ? "an image" : slot.accept === "video" ? "a video" : "audio"}, and ${wrong} isn't ${slot.accept === "audio" ? "audio" : "one"}`)
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
    const what = going.length === 1 ? going[0] : `${going.length} boxes`
    const edit = merged?.edit ?? this.step(`Deleted ${what}`, going)
    let note = 0
    for (const name of going) {
      const id = this.ids.get(name)!
      const box: Gone = { name, view: this.views.get(name)!, row: this.position(name) ?? this.rows?.get(name) ?? { x: 0, y: 0 } }
      this.deleting.set(name, NOT_YET)
      this.deletingFrom.set(name, nextProjectOrder())
      this.rows?.delete(name)
      this.removeNode(name)
      this.forget([name])
      this.outbox.add<AssetDeleted>({
        send: (once, action) => this.client.deleteAsset(this.projectId!, this.served.get(id) ?? name, { once, edit, action }),
        done: () => {
          if (this.deleting.has(name)) this.deleting.set(name, nextProjectOrder())
          this.forgotten([name])
          if (this.heldBack.delete(name)) this.readWhole()
        },
        // Its answer was lost: its first try did it.
        landed: (error) => (error.status === 404 ? { name } : undefined),
        refused: (error) => {
          this.deleting.delete(name)
          this.deletingFrom.delete(name)
          this.heldBack.delete(name)
          this.bringBack(box)
          this.readAgain([name])
          this.history.forget(edit)
          dismiss(note)
          say(`Couldn't delete ${name}`, error)
        },
      })
    }
    const said = readers.length ? ` ${readers.join(", ")} ${readers.length === 1 ? "uses" : "use"} it and ${readers.length === 1 ? "isn't" : "aren't"} approved yet.` : ""
    note = this.offer(edit, merged ? `${what} merged into ${merged.into}.${said}` : `Deleted ${what}.${said}`)
  }

  /** A box again, as it was, where it was. */
  private bringBack(box: Gone, pending?: number) {
    const { view, name, row } = box
    this.deleting.delete(name)
    this.deletingFrom.delete(name)
    this.heldBack.delete(name)
    this.rows?.set(name, row)
    this.views.set(name, { ...view, pending })
    this.viewport?.upsertNodes(this.node(name, row))
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
    const note = say(message, undefined, { action: { label: "Undo", run: () => this.undo({ edit }) } })
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
        if (answer.lost?.length) {
          dismiss(note)
          this.stepped = say(`${back ? "Undone" : "Redone"}: ${taken.length} ${taken.length === 1 ? "edit" : "edits"}. ${lostSaid(answer.lost)}`)
        }
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
    if (!id) return
    const words = this.history.wordsOf(target)
    const shown = this.showStep(target, back)
    this.history.mark(target, back)
    this.typing = null
    const verb = back ? "undo" : "redo"
    for (const edit of this.history.editsOf(target)) dismiss(this.offers.get(edit) ?? 0)
    dismiss(this.stepped)
    const note = say(`${back ? "Undone" : "Redone"}: ${words}`, undefined, { action: { label: back ? "Redo" : "Undo", run: () => (back ? this.redo(target) : this.undo(target)) } })
    this.stepped = note
    this.outbox.add<HistoryStepped>({
      send: (once, action) => this.client[verb](id, target, { once, action }),
      done: (answer) => {
        shown.settle()
        // A box whose undo window ended comes back without its files: said, with the same Redo or Undo (cl-919).
        if (answer.lost?.length) {
          dismiss(note)
          this.stepped = say(`${back ? "Undone" : "Redone"}: ${words}. ${lostSaid(answer.lost)}`, undefined, { action: { label: back ? "Redo" : "Undo", run: () => (back ? this.redo(target) : this.undo(target)) } })
        }
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
   * settles it, as any write's answer does; refused, all of it goes and what Calcifer holds is read again. A name put back is a rename, as `rename` has one.
   */
  private showAs(snaps: Snap[]): { settle(): void; release(): void } {
    const overlays: { ask: Asking; drop(): void }[] = []
    const back: string[] = []
    const hidden: string[] = []
    const moved: string[] = []
    /** Each box renamed back, by its node, and the name Calcifer has for it until it's answered. */
    const renamed: { id: string; from: string }[] = []
    for (const snap of snaps) {
      const name = (snap.node && this.namesById.get(snap.node)) || snap.name
      const now = this.deleting.has(name) ? undefined : this.views.get(name)
      if (!snap.view) {
        if (!now || now.placing) continue
        this.deleting.set(name, NOT_YET)
        this.deletingFrom.set(name, nextProjectOrder())
        this.rows?.delete(name)
        this.removeNode(name)
        this.forget([name])
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
        if (id) {
          back.push(snap.name)
          renamed.push({ id, from: name })
        }
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
        this.forgotten(hidden)
        if (hidden.filter((name) => this.heldBack.delete(name)).length) this.readWhole()
        for (const name of back) {
          const id = this.ids.get(name)
          if (id) this.served.set(id, name)
          this.patch(name, { pending: at })
        }
        for (const { from } of renamed) if (this.renames.has(from)) this.renamedAt.set(from, at)
        const there = names.filter((name) => !hidden.includes(name))
        if (there.length) this.readAgain(there)
      },
      release: () => {
        for (const { drop } of overlays) drop()
        for (const name of back) this.patch(name, { pending: nextProjectOrder() })
        for (const name of hidden) {
          this.deleting.delete(name)
          this.deletingFrom.delete(name)
          this.heldBack.delete(name)
        }
        for (const name of moved) this.masks.delete(name)
        // Each name it put back goes back to what Calcifer has, and what took that name meanwhile, another writer's, shows as Calcifer has it.
        for (const { id, from } of renamed) {
          this.renames.delete(from)
          this.renamedAt.delete(from)
          const now = this.namesById.get(id)
          if (now && now !== from) this.moveName(now, from, true)
        }
        if (renamed.length && this.state && this.rows && this.viewport) this.reconcile()
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
  addPlacing(key: string, name: string, placing: Placing, at: Card, opts: { naming: boolean; preview?: string; aspect?: number }, edit: string) {
    this.views.set(key, { name, placing, draft: opts.naming ? name : undefined, preview: opts.preview ? { url: opts.preview } : undefined, aspect: opts.aspect })
    const row = this.clearOf(key, at, edit)
    this.rows?.set(key, row)
    this.viewport?.upsertNodes(this.node(key, row))
    this.joinStep(edit, [key])
  }

  /** `at` for a box not on the canvas yet: there, a little smaller at most, with what it lies on pushed aside under `edit`, unless something that can't be is in the way. */
  private clearOf(key: string, at: Card, edit: string): Card {
    const view = this.views.get(key)
    if (!view) return at
    const w = at.w ?? widthFor(view)
    const field = this.field()
    const { pushed, ...fitted } = fit(this.laid(key, { x: at.x, y: at.y, w, h: heightFor(view, w) }), field, { tall: this.tallOf(key), pushes: this.pushable(new Set([key])) })
    this.gesture = edit
    this.push(field, pushed)
    this.gesture = null
    const canonical = canvasBox(view, fitted)
    return { x: canonical.x, y: canonical.y, w: canonical.w }
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
    if (this.adding === 0 && this.state && this.rows && this.viewport) this.reconcile()
  }

  /** The project holds it now. If it moved while being asked, its row moves too. */
  land(key: string, answer: Imported, at: Card, edit?: string) {
    const preview = this.views.get(key)?.preview
    this.patch(key, { placing: undefined, draft: undefined, landed: { sha256: answer.sha256, field: answer.field }, ...(preview ? { preview: { ...preview, sha256: hexOf(answer.sha256) } } : {}) })
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
    if (view?.preview) URL.revokeObjectURL(view.preview.url)
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

  /** It shows a file other than the one dropped, and that's up: the dropped one's bytes go. */
  showThumb(name: string) {
    const view = this.get(name)
    if (!view?.preview) return
    URL.revokeObjectURL(view.preview.url)
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
    const node = id ? this.viewport?.getNode(id) : undefined
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
    this.unsettle(key, "grown")
  }

  /** A pull let go of with Escape: its width and shape as they were, what it pushed aside back where it is, and nothing kept. */
  cancelPull(name: string, width: number) {
    const key = this.resolveName(name)
    this.patch(key, { shape: undefined })
    const before = this.position(key)
    if (before) { this.rememberPose(key, { ...before, w: width }); this.paintPose(key) }
    this.letBack(new Map())
    if (this.pulling?.name === key) this.pulling = null
    this.reshape(key)
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

  /** A dragged output shape is explicit; only resetting it returns control to automatic sizing. */
  setShape(name: string, control: BoxField, width: number | undefined, shape: string) {
    const held = this.valueOf(name, control.field)
    const edit = this.step(`Reshaped ${this.resolveName(name)}`, [name])
    if (held !== shape) void this.setField(name, control.field, shape, edit)
    this.patch(name, { shape: undefined })
    if (width !== undefined) this.setWidth(name, width, true, edit)
    else this.reshape(name)
  }

  /** Its shape unset: it takes its auto shape again, at the width it has. */
  unshape(name: string, field: string) {
    void this.setField(name, field, null)
    this.reshape(name)
  }

  /** Whatever's done to `name`, in its turn, under what it's called by then. Gone by then, there's nothing to do. */
  private act<T>(name: string, action: Omit<Action<T>, "send"> & { send(name: string, write: Write): Promise<T>; edit?: string; gone?(): void }): Action<T | typeof NOT_THERE> {
    let id = this.ids.get(this.resolveName(name))
    const inTurn = {
      ...action,
      send: async (once: AbortSignal, outboxed: string) => {
        let refusedAs: string | undefined
        for (;;) {
          // Its node made again since (a read can draw a box anew), it's found by its name, as the page's renames carried it (offline, e2e
          // 44027468586: a picture dropped on a box renamed offline was never sent, and nothing said so).
          if (!id || !this.namesById.has(id)) id = this.ids.get(this.resolveName(name))
          const served = id && this.namesById.has(id) ? this.served.get(id) : undefined
          if (!id || !served) return NOT_THERE
          try {
            return await action.send(served, { once, edit: action.edit, action: outboxed })
          } catch (error) {
            // Calcifer has nothing by the name the page knows it by: another writer renamed or deleted it since the page last heard. It
            // goes again, still in its turn, under the name the stream says it has now, and gone, there's nothing to do. Its own rename on
            // its way, or refused twice under one name, the refusal stands.
            const unknown = error instanceof ApiError && error.status === 404 && error.body.code === "asset_unknown"
            if (!unknown) throw error
            // The stream told of it while this try was on its way: it goes again at once, under the name it has now.
            if (!this.namesById.has(id) || this.served.get(id) !== served) continue
            if (refusedAs === served || this.namesById.get(id) !== served) throw error
            refusedAs = served
            await this.heardOf(id, served, once, error)
          }
        }
      },
      done: (answer: T | typeof NOT_THERE) => {
        if (answer !== NOT_THERE) action.done?.(answer as T)
        else action.gone?.()
      },
    } as Action<T | typeof NOT_THERE>
    this.outbox.add(inTurn)
    return inTurn
  }

  /**
   * Box `id`, which Calcifer had nothing by `served` for, until the page hears what became of it: renamed or gone, as the stream says, or
   * found as `served` again by a read begun after. Put away, the canvas hears nothing: `refusal` stands.
   */
  private heardOf(id: string, served: string, signal: AbortSignal, refusal: unknown): Promise<void> {
    if (!this.namesById.has(id) || this.served.get(id) !== served) return Promise.resolve()
    const after = nextProjectOrder()
    const was = this.state?.assets.find((asset) => asset.name === served)
    this.readAgain([served])
    return new Promise((done, failed) => {
      const look = () => {
        if (signal.aborted) return end(() => failed(signal.reason))
        const read = this.began > after ? this.state?.assets.find((asset) => asset.name === served) : undefined
        if (!this.namesById.has(id) || this.served.get(id) !== served || (read && read !== was)) return end(done)
        if (this.retired || !this.viewport) end(() => failed(refusal))
      }
      const end = (then: () => void) => {
        this.hearing.delete(look)
        signal.removeEventListener("abort", look)
        then()
      }
      this.hearing.add(look)
      signal.addEventListener("abort", look, { once: true })
      look()
    })
  }

  /** The name a new box takes now, as Calcifer would give it: what's on the canvas and what's being placed is taken. A deleted box's name is free (cl-919). */
  private nameFor(wanted: string): string {
    return freeName(wanted, [...this.views].map(([key, view]) => (view.placing ? view.name : key)))
  }

  /** Whether `mention` names a box deleted from the project that Undo can still bring back: it resolves to nothing, and says so (cl-919). */
  deletedName(mention: string): boolean {
    const key = nameKey(baseOf(mention))
    return (this.state?.orphans ?? []).some((orphan) => nameKey(orphan.name) === key)
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
    done?: (name: string, answer: Partial<T>, edit: string) => void,
    edit?: string,
  ): string {
    const one = async ([at]: Card[], write: Write, pushes: Pushes) => {
      const answer = await ask(at, write, pushes)
      return { made: [answer], kept: answer.kept }
    }
    return this.placeAll([{ wanted, view, dropped }], one, refused, done, edit)[0]
  }

  /** Boxes, now, each as `place` has one, made in the project by one ask: one request, one transaction, one entry, and refused together. */
  placeAll<T extends { name: string; etag?: string }>(
    boxes: { wanted: string; view: Omit<ElementView, "name">; dropped: Card }[],
    ask: (at: Card[], write: Write, pushes: Pushes) => Promise<{ made: T[]; kept?: Record<string, Card | null> }>,
    refused: (error: unknown) => void,
    done?: (name: string, answer: Partial<T>, edit: string) => void,
    edit?: string,
  ): string[] {
    const by = edit ?? this.step(`Added ${this.nameFor(boxes[0].wanted)}`, [])
    this.adding += boxes.length
    const placed = boxes.map(({ wanted, view, dropped }) => {
      const name = this.nameFor(wanted)
      const asset = view.asset && { ...view.asset, name }
      this.views.set(name, { ...view, name, asset, pending: NOT_YET, landing: true })
      this.reauto(name, this.views.get(name)!)
      const at = this.clearOf(name, dropped, by)
      this.rows?.set(name, at)
      const node = this.node(name, at)
      this.viewport?.upsertNodes(node)
      this.history.stepOf({ edit: by })?.before.push({ name, node: node.id })
      return { name, at, node }
    })
    const left = () => {
      this.adding -= boxes.length
      if (this.adding === 0 && this.state && this.rows && this.viewport) this.reconcile()
    }
    // What their drop pushed aside goes with them, the same on every try: one request, one transaction, one entry.
    let pushes: Pushes | undefined
    this.outbox.add<{ made: T[]; kept?: Record<string, Card | null> }>({
      send: (once, action) => {
        pushes ??= this.pushesFor(by)
        return ask(placed.map(({ at }) => at), { once, edit: by, action }, pushes)
      },
      done: (answer) => {
        if (pushes) this.pushesSent(pushes, answer.kept)
        placed.forEach(({ name, at, node }, i) => {
          // Sent again after Calcifer stopped between doing it and answering, it's answered only that it was done (`{}`): the box keeps
          // the name it asked for, and the read after it says the rest.
          const made: Partial<T> | undefined = answer.made ? answer.made[i] : {}
          if (!made) return
          if (made.name) {
            this.served.set(node.id, made.name)
            // Another writer took the name, unless a rename has moved it on since.
            if (made.name !== name && this.namesById.get(node.id) === name) this.moveName(name, made.name)
          }
          const now = this.namesById.get(node.id)
          if (!now) return
          const row = this.views.get(now)?.asset
          this.patch(now, { ...(made.etag ? { known: made.etag, asset: row && { ...row, etag: made.etag } } : {}), pending: nextProjectOrder() })
          // What it pushed aside goes with its own row, in one write: one entry of the history.
          if (this.pushedFor(by)) this.move(now, this.rows?.get(now) ?? at, undefined, by)
          done?.(now, made, by)
        })
        this.readAfter(placed.map(({ node }) => node.id))
        left()
        this.sendMoves()
      },
      refused: (error) => {
        for (const { node } of placed) {
          const now = this.namesById.get(node.id)
          if (!now) continue
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
    return placed.map(({ name }) => name)
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
        say("Couldn't save that move", error)
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
      done: () => {
        this.served.set(id, free)
        this.renamedAt.set(from, nextProjectOrder())
        this.keepWords(id)
      },
      // A try whose answer was lost: its first did it.
      landed: (error) => (error.status === 404 ? { from, to: free, versions: 0 } : undefined),
      refused: (error) => {
        this.history.forget(edit)
        say(`Couldn't rename ${from}`, error)
        this.renames.delete(from)
        const now = this.namesById.get(id)
        if (now) this.moveName(now, from, true)
        // What took the name meanwhile, another writer's, shows now as Calcifer has it.
        if (this.state && this.rows && this.viewport) this.reconcile()
      },
    })
  }
}

/** `a` or `an`, as `kind` is said. */
function an(kind: string): string {
  return /^[aeiou]/i.test(kind) ? "an" : "a"
}
