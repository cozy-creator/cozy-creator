import type { Edge, Node, ReactFlowInstance } from "@xyflow/react"
import { ApiError, type Client } from "../api/client"
import type { Entity, Imported, AssetDeleted, AssetPatched, Asset, ProjectState, Row, Run, Standing, Tool } from "../types"
import { say } from "../ui/notes"
import { signal } from "../ui/signal"
import { type Box, homesFor, readsOf, warningsOf } from "./graph"
import { asName, freeName, nameKey } from "./names"
import { heightFor, widthFor } from "./shown"

export type ElementData = { name: string }
export type ElementNode = Node<ElementData, "element" | "entity">

/** A box that takes elements dragged onto it, at a point: a text at its caret, a slot. A tool can register one of its own. */
export interface DropTarget {
  over(names: string[], x: number, y: number): void
  drop(names: string[], x: number, y: number): void
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
  placing?: Placing
  /** A blob URL, until the thumbnail is up. */
  preview?: string
  /** The name field's text, while it's open. */
  draft?: string
  aspect?: number
  run?: Run
  /** Which face it shows, when a person flipped it. Nothing leaves this page. */
  face?: "edit" | "media"
  /** What's typed and not saved, by field. A card culled away keeps it here. */
  drafts?: Record<string, unknown>
  /** What was written and not read back yet, by field. */
  sent?: Record<string, unknown>
  /** The `etag` the drafts began from, moved along by writes of its own. */
  base?: string
  /** The file moved under the drafts. */
  conflict?: boolean
  /** Its render waits for a machine, and what it keeps. */
  waiting?: Keep
  /** Seconds into a video, scrubbed to. */
  scrub?: number
  /** Its ratio while it's being reshaped. */
  shape?: string
  /** Its warnings are open under it. */
  warned?: boolean
  /** Something is being dragged over it. */
  hot?: boolean
  /** It just arrived, and lands. */
  landing?: boolean
  /** The take that just landed, sliding in beside the last. */
  slid?: number
}

/** What a new take keeps of an old one. */
export interface Keep {
  take?: number
  in_s?: number
}

export const CARD_WIDTH = 240

type Flow = ReactFlowInstance<ElementNode, Edge>
const UNDO_MS = 5 * 60_000

const samePlace = (a: { x: number; y: number }, b: { x: number; y: number }) => a.x === b.x && a.y === b.y

export function toRow(node: ElementNode): Row {
  return { x: Math.round(node.position.x), y: Math.round(node.position.y), w: node.width ?? CARD_WIDTH }
}

/** What the state says of one element: an asset, or an entity. */
type Placed = Pick<ElementView, "name" | "asset" | "entity" | "linked" | "queued" | "warnings">

const viewOf = (element: Placed): ElementView => ({ ...element })

function linksOf(entity: Entity): string[] {
  return [...Object.values(entity.links ?? {}), ...(entity.subs ?? []).flatMap(linksOf)]
}

const WORST = ["making", "failed", "out_of_date", "missing", "current"] as const

/** How an entity stands: the worst of what it links. */
const worst = (standings: (Standing | "failed" | undefined)[]) => WORST.find((standing) => standings.includes(standing)) ?? "current"

/**
 * What the canvas knows beyond React Flow's positions, viewport and selection: a plain store with per-name subscribers. `node.data` is
 * `{name}` and a node's id never changes, so state changes never touch React Flow, and an import or a rename stays the same node.
 */
export class CanvasStore {
  projectId: string | null
  private client: Client
  private flow: Flow | null = null
  private views = new Map<string, ElementView>()
  private aliases = new Map<string, string>()
  private subscribers = new Map<string, Set<() => void>>()
  private shapes = new Map<string, string>()
  private asked = new Set<number>()
  private runs = new Map<number, Run>()
  /** A push or pull is going. */
  readonly transferring = signal(false)
  /** The tools known so far, by id. */
  readonly tools = signal<Map<string, Tool>>(new Map())
  /** Names came or went: a text shows a name as a chip or as words by it. */
  readonly names = signal(0)
  /** The right click open, and where. */
  readonly menu = signal<{ name: string; x: number; y: number } | null>(null)
  /** What takes a drop, by its element: a box marks itself `data-drop-target`. */
  readonly dropTargets = new Map<Element, DropTarget>()
  /** Each open text's way to take a name at its caret, by `<node>:<place>`. */
  private carets = new Map<string, (name: string) => void>()
  /** The box last typed in stays open until a click elsewhere, so what's dragged next can land where its caret was. */
  readonly editing = signal<string | null>(null)
  /** Asks for a machine, for a render with none up. */
  askMachine: () => void = () => {}
  /** The name a just-dropped tool will have: its text takes the keys. */
  focusing: string | null = null
  /** The machines a render may go to, a rental before this computer: the walk places it on whichever is free. */
  machines: () => string[] = () => []
  /** The first thing a take asked for after this needs no reload to show. */
  refresh: () => void = () => {}
  private ids = new Map<string, string>()
  private nextId = 0
  private state: ProjectState | null = null
  private rows: Map<string, Row> | null = null
  private owners = new Map<number, string>()
  private dragging = new Set<string>()
  /** Where what's being dragged started, for a drop that isn't a move. */
  private dragFrom = new Map<string, { x: number; y: number }>()
  /** On its way out: a read from before that doesn't bring it back. */
  private deleting = new Set<string>()
  /** Each save of a node goes after the last. */
  private saving = new Map<string, Promise<boolean>>()
  /** Names about to land: what was dropped here, and nothing a read found. */
  private landings = new Set<string>()
  /** A position of ours that the stream hasn't echoed: it wins until then. */
  private masks = new Map<string, Row>()
  private moving = new Map<string, Row>()
  private sending = false
  private renames = new Map<string, string>()
  private adding = 0
  private seen = false
  private selected: Set<string>
  private named = new Set<() => void>()
  /** The project was read for the first time; whether it held anything. */
  onFirstRead?: (held: boolean) => void
  /** Elements with no row were just placed. */
  onPlaced?: () => void

  constructor(client: Client, projectId: string | null, selected: Iterable<string>) {
    this.client = client
    this.projectId = projectId
    this.selected = new Set(selected)
  }

  attach(flow: Flow) {
    this.flow = flow
    if (this.state && this.rows) this.reconcile()
  }

  detach() {
    this.flow = null
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
    const key = this.resolveName(name)
    const view = this.views.get(key)
    if (!view) return
    this.views.set(key, { ...view, ...change })
    this.notify(key)
  }

  patchPlacing(key: string, change: Partial<Placing>) {
    const view = this.views.get(key)
    if (view?.placing) this.patch(key, { placing: { ...view.placing, ...change } })
  }

  /** The names an element can't take. */
  takenBesides(name: string): string[] {
    return [...this.views].filter(([key, view]) => key !== name && !view.placing).map(([key]) => key)
  }

  position(name: string): Row | null {
    const id = this.ids.get(this.resolveName(name))
    const node = id ? this.flow?.getNode(id) : undefined
    return node ? toRow(node) : null
  }

  // From Calcifer

  applyState(state: ProjectState) {
    this.state = state
    this.transferring.set(Boolean(state.moving))
    if (this.rows && this.flow) this.reconcile()
  }

  /** `GET canvas`: every row. */
  applyCanvas(cards: Record<string, Row>) {
    const first = !this.rows
    this.rows = new Map(Object.entries(cards))
    if (first) {
      if (this.state && this.flow) this.reconcile()
      return
    }
    for (const [name, row] of this.rows) this.applyRow(name, row)
  }

  /** A `canvas` event: the rows that changed. Stream order is the truth. */
  applyRows(cards: Record<string, Row | null>) {
    if (!this.rows) return
    for (const [name, row] of Object.entries(cards)) {
      if (row) {
        this.rows.set(name, row)
        this.applyRow(name, row)
      } else {
        this.rows.delete(name)
      }
    }
  }

  private applyRow(name: string, row: Row) {
    if (this.dragging.has(name)) return
    const mask = this.masks.get(name)
    if (mask) {
      if (samePlace(mask, row)) this.masks.delete(name)
      return
    }
    const id = this.ids.get(name)
    if (id) this.flow?.updateNode(id, { position: { x: row.x, y: row.y }, ...(row.w ? { width: row.w } : {}) })
    if (row.w) this.reshape(name)
  }

  /** Fell behind: what the stream would have told us is about to be read again. */
  clearMasks() {
    this.masks.clear()
  }

  /** Tools the list didn't have, as they're found. */
  addTools(tools: Tool[]) {
    const known = this.tools.get()
    const fresh = tools.filter((tool) => JSON.stringify(known.get(tool.id)) !== JSON.stringify(tool))
    if (fresh.length) this.tools.set(new Map([...known, ...fresh.map((tool): [string, Tool] => [tool.id, tool])]))
  }

  /** What a run says of the asset it's making: the lowest going, when there are several. */
  applyRun(run: Run) {
    this.runs.set(run.number, run)
    const owner = this.owners.get(run.number)
    if (!owner) return
    const making = this.get(owner)?.asset?.making ?? []
    if (run.number === Math.min(...making)) this.patch(owner, { run })
  }

  private reconcile() {
    const state = this.state!
    const rows = this.rows!
    const entities = state.entities ?? []
    const present = new Set([...state.assets, ...entities].map((element) => element.name))
    for (const name of this.deleting) if (!present.has(name)) this.deleting.delete(name)
    for (const name of this.deleting) present.delete(name)
    for (const [from, to] of this.renames) if (present.has(to)) this.renames.delete(from)
    const becoming = new Set(this.renames.values())
    const fresh: Placed[] = []
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
        if (Boolean(view.queued) !== queued.has(asset.name)) change.queued = queued.has(asset.name)
        if (view.asset !== asset || view.landed) {
          Object.assign(change, { asset, landed: undefined })
          if (!asset.making?.length) change.run = undefined
          const newest = asset.prefixes?.at(-1)?.number
          if (newest && newest !== view.asset?.prefixes?.at(-1)?.number && view.asset) change.slid = newest
          // Read back: what was sent is what it holds, or someone wrote since.
          if (asset.etag !== view.asset?.etag) Object.assign(change, { sent: undefined, ...(view.drafts ? {} : { base: undefined }) })
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
      if (!present.has(name) && !view.placing && !view.landed && !becoming.has(name)) this.removeNode(name)
    }
    this.addNodes(fresh.filter((element) => rows.has(element.name)).map((element) => ({ element, row: rows.get(element.name)! })))
    const homeless = fresh.filter((element) => !rows.has(element.name))
    if (homeless.length) void this.findHomes(homeless)
    this.names.set(this.names.get() + 1)
    if (!this.seen) {
      this.seen = true
      this.onFirstRead?.(present.size > 0)
    }
  }

  /** A run this page hasn't heard of: its progress comes on the stream, but what it says now has to be read. */
  private askRun(number: number) {
    if (this.asked.has(number)) return
    this.asked.add(number)
    this.client.run(number).then((run) => this.applyRun(run), () => {})
  }

  /** A name with no row may just be ahead of its row on the stream, so ask once. Still none: place it, once, and write the row. */
  private async findHomes(elements: Placed[]) {
    if (!this.projectId) return
    let cards: Record<string, Row> = {}
    try {
      cards = (await this.client.canvas(this.projectId)).cards
    } catch (error) {
      say("Couldn't read where things sit", error)
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
    const homes = this.homes(left)
    this.addNodes(left.map((element) => ({ element, row: homes.get(element.name)! })))
    for (const [name, row] of homes) this.move(name, row)
    // Opened here for the first time, with nothing placed yet: its whole new layout is what to look at.
    this.onPlaced?.()
  }

  private homes(left: Placed[]): Map<string, Row> {
    const boxes = new Map<string, Box>()
    for (const [name, id] of this.ids) {
      const node = this.flow?.getNode(id)
      const view = this.views.get(name)
      if (!node || !view) continue
      const w = node.width ?? widthFor(view)
      boxes.set(name, { x: node.position.x, y: node.position.y, w, h: node.measured?.height ?? heightFor(view, w) })
    }
    const sizes = new Map(left.map((element) => {
      const view = viewOf(element)
      const w = widthFor(view)
      return [element.name, { w, h: heightFor(view, w) }]
    }))
    const names = left.map((element) => element.name)
    return homesFor(names, readsOf(this.state!), boxes, (name) => sizes.get(name)!)
  }

  private addNodes(items: { element: Placed; row: Row }[]) {
    if (!items.length) return
    const nodes = items.map(({ element, row }) => {
      const making = element.asset?.making ?? []
      // Its card may be placed after what its run said came back.
      this.views.set(element.name, { ...viewOf(element), landing: this.landings.delete(element.name) || undefined, run: making.length ? this.runs.get(Math.min(...making)) : undefined })
      for (const number of making) this.owners.set(number, element.name)
      return this.node(element.name, row, this.selected.has(element.name))
    })
    this.flow?.addNodes(nodes)
  }

  private node(name: string, row: Row, selected = false): ElementNode {
    const id = `e${++this.nextId}`
    this.ids.set(name, id)
    const view = this.views.get(name)!
    const width = row.w ?? widthFor(view)
    const height = heightFor(view, width)
    this.shapes.set(name, `${width}:${height}`)
    return {
      id,
      type: view.entity ? "entity" : "element",
      position: { x: row.x, y: row.y },
      data: { name },
      width,
      // Its size from the row and the lock: a culled node never gets measured, and React Flow's fit leaves out what isn't.
      measured: { width, height },
      selected,
    }
  }

  /** Its size, unmeasured, for a fit and for culling: the node hears only when it moved. */
  private reshape(name: string) {
    const key = this.resolveName(name)
    const id = this.ids.get(key)
    const view = this.views.get(key)
    const node = id ? this.flow?.getNode(id) : undefined
    if (!id || !view || !node) return
    const width = node.width ?? widthFor(view)
    const height = heightFor(view, width)
    const shape = `${width}:${height}`
    if (this.shapes.get(key) === shape) return
    this.shapes.set(key, shape)
    this.flow?.updateNode(id, { measured: { width, height } })
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

  seek(name: string, at: number) {
    const held = this.get(name)
    if (held) this.patch(name, { seek: { at, ask: (held.seek?.ask ?? 0) + 1 } })
  }

  /** A new take or version: anything typed saved first, on the machine that's up; what Calcifer runs itself needs none. With none up, it asks for one and goes when one is. */
  async run(name: string, keep?: Keep) {
    if (!this.projectId || !(await this.save(name))) return
    const asset = this.get(name)?.asset
    if (!asset) return
    const on = asset.tool === "cozy" ? ["local"] : this.machines()
    if (!on.length) {
      this.patch(name, { waiting: keep ?? {} })
      this.askMachine()
      return
    }
    this.patch(name, { waiting: undefined, scrub: undefined })
    try {
      await this.client.render(this.projectId, [asset.name], on, `take-${crypto.randomUUID()}`, keep)
      this.refresh()
    } catch (error) {
      say(`Couldn't render ${name}`, error)
    }
  }

  /** A machine came up: each render that waited for one goes. */
  machineUp() {
    for (const [name, view] of this.views) if (view.waiting) void this.run(name, view.waiting)
  }

  /** Render all. What's approved stays, and with `outdated`, what's out of date when its turn comes goes too. */
  async runAll(outdated: boolean) {
    const on = this.machines()
    if (!this.projectId) return
    if (!on.length) return this.askMachine()
    for (const name of this.views.keys()) if (!(await this.save(name))) return
    try {
      await this.client.render(this.projectId, ["all"], on, `all-${crypto.randomUUID()}`, outdated ? { outdated } : {})
      this.refresh()
    } catch (error) {
      say("Couldn't render them", error)
    }
  }

  /** Approves it: this version is what `name` is, until it's unapproved. */
  async pin(name: string, number: number) {
    if (!this.projectId) return
    try {
      const pinned = await this.client.approve(this.projectId, this.get(name)?.asset?.name ?? name, String(number))
      if (pinned.stale.length) say(`That puts ${pinned.stale.join(", ")} out of date`)
      // It turns into that file, and can flip back.
      this.patch(name, { face: undefined })
      this.refresh()
    } catch (error) {
      say(`Couldn't approve ${name}`, error)
    }
  }

  async unpin(name: string) {
    if (!this.projectId) return
    try {
      const unpinned = await this.client.unpin(this.projectId, this.get(name)?.asset?.name ?? name)
      if (unpinned.stale.length) say(`That puts ${unpinned.stale.join(", ")} out of date`)
      this.refresh()
    } catch (error) {
      say(`Couldn't unapprove ${name}`, error)
    }
  }

  async cancelRender(name: string) {
    if (!this.projectId) return
    try {
      await this.client.cancelRender(this.projectId, [name])
    } catch (error) {
      say(`Couldn't cancel ${name}`, error)
    }
  }

  async pull() {
    if (!this.projectId) return
    try {
      await this.client.pull(this.projectId)
      this.refresh()
    } catch (error) {
      say("Couldn't pull this project's files", error)
    }
  }

  private removeNode(name: string) {
    const id = this.ids.get(name)
    this.views.delete(name)
    this.ids.delete(name)
    this.masks.delete(name)
    this.shapes.delete(name)
    this.notify(name)
    if (id) this.flow?.setNodes((nodes) => nodes.filter((node) => node.id !== id))
  }

  /** The same node and view under a new name. */
  private moveName(from: string, to: string) {
    const view = this.views.get(from)
    const id = this.ids.get(from)
    if (!view || !id) return
    this.views.delete(from)
    this.ids.delete(from)
    if (this.dragging.delete(from)) this.dragging.add(to)
    this.views.set(to, { ...view, name: to })
    this.ids.set(to, id)
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
    this.notify(to)
    this.flow?.updateNodeData(id, { name: to })
  }

  // A node's inputs

  /** What a control shows: what's typed, else what was sent, else what it holds. */
  valueOf(name: string, field: string): unknown {
    const view = this.get(name)
    if (view?.drafts && field in view.drafts) return view.drafts[field]
    if (view?.sent && field in view.sent) return view.sent[field]
    return view?.asset?.fields?.[field]
  }

  /** Typed, and not saved: a walk going reads the file, so a save is explicit, or on leaving the text. */
  setDraft(name: string, field: string, value: unknown) {
    const view = this.get(name)
    if (view) this.patch(name, { drafts: { ...view.drafts, [field]: value }, base: view.base ?? view.asset?.etag })
  }

  /** Writes what's typed. Whether it stands: a refusal leaves it typed. */
  save(name: string): Promise<boolean> {
    const key = this.resolveName(name)
    const going = (this.saving.get(key) ?? Promise.resolve(true)).then(() => this.write(key))
    this.saving.set(key, going)
    return going
  }

  private async write(key: string, force = false): Promise<boolean> {
    const view = this.views.get(key)
    if (!view?.drafts || !view.asset || !this.projectId) return true
    const drafts = view.drafts
    this.patch(key, { drafts: undefined, sent: { ...view.sent, ...drafts }, conflict: undefined })
    try {
      const answer = await this.client.patchAsset(this.projectId, view.asset.name, { fields: drafts }, force ? undefined : view.base)
      this.wrote(key, answer)
      this.patch(key, { base: this.views.get(key)?.drafts ? this.views.get(key)?.base : undefined })
      this.refresh()
      return true
    } catch (error) {
      const now = this.views.get(key)
      const sent = Object.fromEntries(Object.entries(now?.sent ?? {}).filter(([field]) => !(field in drafts)))
      this.patch(key, { drafts: { ...drafts, ...now?.drafts }, sent })
      if (error instanceof ApiError && error.body.code === "asset_changed") this.patch(key, { conflict: true })
      else say(`Couldn't save ${view.name}`, error)
      return false
    }
  }

  /** A write of its own moves what the drafts began from; anyone else's doesn't, so that still conflicts. */
  private wrote(key: string, answer: AssetPatched) {
    const view = this.views.get(key)
    if (view && view.base === answer.was) this.patch(key, { base: answer.etag })
    for (const warning of answer.warnings ?? []) say(warning)
  }

  /** The file changed under what's typed: this is written over it. */
  keepMine(name: string) {
    const key = this.resolveName(name)
    return this.write(key, true)
  }

  /** The file changed under what's typed: what's typed goes. */
  takeTheFiles(name: string) {
    this.patch(name, { drafts: undefined, base: undefined, conflict: undefined })
  }

  /** A slider let go, a choice made: one field, whatever else is typed. */
  async setField(name: string, field: string, value: unknown) {
    const key = this.resolveName(name)
    const view = this.views.get(key)
    if (!view?.asset || !this.projectId) return
    const { [field]: _, ...drafts } = view.drafts ?? {}
    this.patch(key, { drafts: Object.keys(drafts).length ? drafts : undefined, sent: { ...view.sent, [field]: value } })
    try {
      this.wrote(key, await this.client.patchAsset(this.projectId, view.asset.name, { fields: { [field]: value } }))
      this.refresh()
    } catch (error) {
      say(`Couldn't set ${name}'s ${field}`, error)
    }
  }

  /** What makes it, among those of its kind. */
  async setTool(name: string, tool: string) {
    const key = this.resolveName(name)
    const view = this.views.get(key)
    if (!view?.asset || !this.projectId) return
    try {
      this.wrote(key, await this.client.patchAsset(this.projectId, view.asset.name, { tool }))
      this.refresh()
    } catch (error) {
      say(`Couldn't change what makes ${name}`, error)
    }
  }

  // Names in a text

  /** Each name a mention could be, for the `<` list: nodes only. */
  namesLike(typed: string, besides: string): string[] {
    const key = nameKey(typed)
    const names = [...this.views.entries()].filter(([name, view]) => name !== besides && !view.placing && !view.entity).map(([name]) => name)
    return names.filter((name) => nameKey(name).startsWith(key)).sort()
  }

  /** The name a mention is, in any case, with `_` and `-` alike. */
  nameOf(mention: string): string | null {
    const key = nameKey(mention)
    for (const [name, view] of this.views) if (!view.placing && nameKey(name) === key) return name
    return null
  }

  /** A text says where its caret takes a name. The first of a box is what a drop on the box means. */
  openCaret(node: string, place: string, insert: (name: string) => void): () => void {
    const key = `${node}:${place}`
    this.carets.set(key, insert)
    return () => void this.carets.delete(key)
  }

  // A drag

  /** Whatever box is under the pointer, with `dragged` taken away: it opens, and what takes a drop is the nearest of its parts, else its first. */
  dropTargetAt(x: number, y: number, dragged: Element[]): DropTarget | null {
    for (const el of document.elementsFromPoint(x, y)) {
      if (dragged.some((held) => held.contains(el))) continue
      const card = el.closest<HTMLElement>(".card[data-asset]")
      if (!card) continue
      this.setHot(this.resolveName(card.dataset.asset ?? ""))
      const own = el.closest("[data-drop-target]") ?? card.querySelector("[data-drop-target]")
      return (own && this.dropTargets.get(own)) ?? null
    }
    this.setHot(null)
    return null
  }

  private hot: string | null = null

  setHot(name: string | null) {
    if (this.hot === name) return
    if (this.hot) this.patch(this.hot, { hot: undefined })
    if (name) this.patch(name, { hot: true })
    this.hot = name
  }

  // Deleting

  /** Gone at once, with no dialog. Files stay: a note offers Undo for a while, and says if something still needs what went. */
  async remove(names: string[]) {
    const id = this.projectId
    const going = names.map((name) => this.resolveName(name)).filter((name) => this.views.get(name)?.asset && !this.views.get(name)?.placing)
    if (!id || !going.length) return
    const rows = new Map(going.map((name) => [name, this.position(name) ?? undefined]))
    const running = going.filter((name) => this.views.get(name)?.asset?.making?.length || this.views.get(name)?.queued)
    if (running.length) await this.client.cancelRender(id, running).catch(() => {})
    const readers = this.readers(going)
    for (const name of going) {
      this.deleting.add(name)
      this.rows?.delete(name)
      this.removeNode(name)
    }
    const answers = await Promise.allSettled(going.map((name) => this.client.deleteAsset(id, name)))
    const taken: AssetDeleted[] = []
    answers.forEach((answer, i) => {
      if (answer.status === "fulfilled") return void taken.push(answer.value)
      this.deleting.delete(going[i])
      say(`Couldn't delete ${going[i]}`, answer.reason)
    })
    this.refresh()
    if (!taken.length) return
    const what = taken.length === 1 ? taken[0].name : `${taken.length} assets`
    const said = readers.length ? ` ${readers.join(", ")} ${readers.length === 1 ? "reads" : "read"} this and ${readers.length === 1 ? "isn't" : "aren't"} approved yet.` : ""
    say(`Deleted ${what}.${said}`, undefined, { action: { label: "Undo", run: () => this.undo(id, taken, rows) }, ms: UNDO_MS })
  }

  /** What reads these and has nothing of its own held: it can't run again without them. */
  private readers(names: string[]): string[] {
    const gone = names.map(nameKey)
    const reads = (mention: string) => gone.some((key) => nameKey(mention) === key || nameKey(mention).startsWith(`${key}.`))
    const after = (this.state?.assets ?? []).filter((asset) => !names.includes(asset.name) && !asset.pinned && asset.reads?.some((read) => reads(read.name)))
    return after.map((asset) => asset.name)
  }

  private async undo(id: string, taken: AssetDeleted[], rows: Map<string, Row | undefined>) {
    for (const node of taken) {
      const at = node.at ?? rows.get(node.name)
      try {
        this.deleting.delete(node.name)
        if (at) this.rows?.set(node.name, at)
        await this.client.putAssetBack(id, node.name, node.text, at)
      } catch (error) {
        say(`Couldn't put ${node.name} back`, error)
      }
    }
    this.refresh()
  }

  // An import

  /** This project has that file, as far as the page has read. */
  holdsFile(sha256: string): boolean {
    for (const view of this.views.values()) if (view.landed?.sha256 === sha256) return true
    return Boolean(
      this.state?.assets.some((asset) => asset.sha256 === sha256 || Object.values(asset.files ?? {}).some((file) => file.sha256 === sha256)),
    )
  }

  addPlacing(key: string, name: string, placing: Placing, at: Row, opts: { naming: boolean; preview?: string }) {
    this.views.set(key, { name, placing, draft: opts.naming ? name : undefined, preview: opts.preview, landing: true })
    this.flow?.addNodes(this.node(key, at))
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
  land(key: string, answer: Imported, at: Row) {
    this.patch(key, { placing: undefined, draft: undefined, landed: { sha256: answer.sha256, field: answer.field } })
    this.moveName(key, answer.name)
    this.rows?.set(answer.name, at)
    const now = this.position(answer.name)
    if (now && !samePlace(now, at)) this.move(answer.name, now)
  }

  cancel(key: string) {
    const view = this.views.get(key)
    view?.placing?.abort.abort()
    if (view?.preview) URL.revokeObjectURL(view.preview)
    this.removeNode(key)
  }

  /** A picture says its shape before it's measured. */
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

  dragStopped(nodes: ElementNode[]) {
    for (const node of nodes) {
      const name = node.data.name
      this.dragging.delete(name)
      this.dragFrom.delete(name)
      if (!this.views.get(name)?.placing) this.move(name, toRow(node))
    }
  }

  /** They were dropped into a box: each goes back where it started, and no move is saved. */
  dragDropped(nodes: ElementNode[]) {
    for (const node of nodes) {
      const name = node.data.name
      const from = this.dragFrom.get(name)
      const id = this.ids.get(name)
      this.dragging.delete(name)
      this.dragFrom.delete(name)
      if (from && id) this.flow?.updateNode(id, { position: from })
    }
  }

  /** A new width, and when it's let go, the row. */
  setWidth(name: string, width: number, keep = true) {
    const key = this.resolveName(name)
    const id = this.ids.get(key)
    const node = id ? this.flow?.getNode(id) : undefined
    if (!id || !node) return
    this.flow?.updateNode(id, { width })
    if (keep) this.move(key, { x: Math.round(node.position.x), y: Math.round(node.position.y), w: Math.round(width) })
  }

  /** A row for a name the project is about to read, so it lands where it was dropped. */
  expect(name: string, at: Row) {
    this.rows?.set(name, at)
    this.focusing = name
    this.landings.add(name)
  }

  /** Coalesced and serial: what's asked while one is going goes next, newest position only. */
  private move(name: string, row: Row) {
    this.masks.set(name, row)
    this.moving.set(name, row)
    void this.sendMoves()
  }

  private async sendMoves() {
    if (this.sending || !this.moving.size || !this.projectId) return
    this.sending = true
    const batch = Object.fromEntries(this.moving)
    this.moving.clear()
    try {
      await this.client.moveCards(this.projectId, batch)
    } catch (error) {
      say("A move didn't save", error)
      for (const name of Object.keys(batch)) this.masks.delete(name)
      void this.client.canvas(this.projectId).then(({ cards }) => this.applyCanvas(cards), () => {})
    }
    this.sending = false
    void this.sendMoves()
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
  private async rename(from: string, to: string) {
    const free = to ? freeName(to, this.takenBesides(from)) : from
    if (free === from || !this.projectId) return
    this.renames.set(from, free)
    this.moveName(from, free)
    try {
      await this.client.rename(this.projectId, from, free)
    } catch (error) {
      say(`Couldn't rename ${from}`, error)
      this.renames.delete(from)
      this.moveName(free, from)
    }
  }
}
