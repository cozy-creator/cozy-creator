import type { Edge, Node, ReactFlowInstance } from "@xyflow/react"
import type { Client } from "../api/client"
import type { Entity, Imported, Asset, ProjectState, Row, Run, Standing } from "../types"
import { say } from "../ui/notes"
import { signal } from "../ui/signal"
import { type Box, type EdgeKind, type EdgeSpec, edgesOf, homesFor, warningsOf } from "./graph"
import { asName, freeName } from "./names"
import { anchorsFor, heightFor, widthFor } from "./shown"

export type ElementData = { name: string }
export type ElementNode = Node<ElementData, "element" | "entity">
export type GraphEdge = Edge<{ kind: EdgeKind }, "graph">

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
}

export const CARD_WIDTH = 240

type Flow = ReactFlowInstance<ElementNode, GraphEdge>

const samePlace = (a: { x: number; y: number }, b: { x: number; y: number }) => a.x === b.x && a.y === b.y

export function toRow(node: ElementNode): Row {
  return { x: Math.round(node.position.x), y: Math.round(node.position.y), w: node.width ?? CARD_WIDTH }
}

/** What the state says of one element: an asset, or an entity. */
type Element = Pick<ElementView, "name" | "asset" | "entity" | "linked" | "queued" | "warnings">

const viewOf = (element: Element): ElementView => ({ ...element })

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
  private edgeSubscribers = new Map<string, () => void>()
  private edges = new Map<string, EdgeSpec>()
  private lit = new Set<string>()
  private shapes = new Map<string, string>()
  private asked = new Set<number>()
  private runs = new Map<number, Run>()
  /** The text open beside the canvas, and the place to show. */
  readonly open = signal<{ name: string; at?: string } | null>(null)
  /** The edge under the pointer, and where. */
  readonly tip = signal<{ id: string; x: number; y: number } | null>(null)
  /** A push or pull is going. */
  readonly transferring = signal(false)
  /** Whatever machine a new take should go to, or none up. */
  machine: () => string | null = () => null
  /** The first thing a take asked for after this needs no reload to show. */
  refresh: () => void = () => {}
  private ids = new Map<string, string>()
  private nextId = 0
  private state: ProjectState | null = null
  private rows: Map<string, Row> | null = null
  private owners = new Map<number, string>()
  private dragging = new Set<string>()
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
    for (const [from, to] of this.renames) if (present.has(to)) this.renames.delete(from)
    const becoming = new Set(this.renames.values())
    const fresh: Element[] = []
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
      if (this.renames.has(asset.name)) continue
      const view = this.views.get(asset.name)
      if (view) {
        const change: Partial<ElementView> = { ...said(view, asset.name) }
        if (Boolean(view.queued) !== queued.has(asset.name)) change.queued = queued.has(asset.name)
        if (view.asset !== asset || view.landed) {
          Object.assign(change, { asset, landed: undefined })
          if (!asset.making?.length) change.run = undefined
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
    this.syncEdges()
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
  private async findHomes(elements: Element[]) {
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
    if (!left.length) return this.syncEdges()
    const homes = this.homes(left)
    this.addNodes(left.map((element) => ({ element, row: homes.get(element.name)! })))
    this.syncEdges()
    for (const [name, row] of homes) this.move(name, row)
    // Opened here for the first time, with nothing placed yet: its whole new layout is what to look at.
    this.onPlaced?.()
  }

  private homes(left: Element[]): Map<string, Row> {
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
    return homesFor(names, edgesOf(this.state!), boxes, (name) => sizes.get(name)!)
  }

  private addNodes(items: { element: Element; row: Row }[]) {
    if (!items.length) return
    const nodes = items.map(({ element, row }) => {
      const making = element.asset?.making ?? []
      // Its card may be placed after what its run said came back.
      this.views.set(element.name, { ...viewOf(element), run: making.length ? this.runs.get(Math.min(...making)) : undefined })
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
    const handles = anchorsFor(view, width)
    this.shapes.set(name, JSON.stringify(handles))
    return {
      id,
      type: view.entity ? "entity" : "element",
      position: { x: row.x, y: row.y },
      data: { name },
      width,
      // Its size from the row and the lock: a culled node never gets measured, and React Flow's fit leaves out what isn't.
      measured: { width, height: heightFor(view, width) },
      handles,
      selected,
    }
  }

  /** Where edges land on a card moves with its shape: the node hears only when it did, so a change of state costs no edge a thing. */
  private reshape(name: string) {
    const key = this.resolveName(name)
    const id = this.ids.get(key)
    const view = this.views.get(key)
    const node = id ? this.flow?.getNode(id) : undefined
    if (!id || !view || !node) return
    const width = node.width ?? widthFor(view)
    const handles = anchorsFor(view, width)
    const shape = JSON.stringify(handles)
    if (this.shapes.get(key) === shape) return
    this.shapes.set(key, shape)
    this.flow?.updateNode(id, { handles, measured: { width, height: heightFor(view, width) } })
  }

  // Edges

  /** The edges the state says, with both ends on the canvas. React Flow hears only what came or went. */
  private syncEdges() {
    if (!this.state || !this.flow) return
    const next = new Map<string, EdgeSpec>()
    for (const spec of edgesOf(this.state)) if (this.ids.has(spec.source) && this.ids.has(spec.target)) next.set(spec.id, spec)
    const added = [...next.values()].filter((spec) => !this.edges.has(spec.id))
    const gone = [...this.edges.keys()].some((id) => !next.has(id))
    this.edges = next
    if (added.length || gone) {
      const made = added.map((spec): GraphEdge => ({
        id: spec.id,
        type: "graph",
        source: this.ids.get(spec.source)!,
        target: this.ids.get(spec.target)!,
        sourceHandle: spec.sourceHandle ?? null,
        targetHandle: spec.targetHandle ?? null,
        data: { kind: spec.kind },
        selectable: false,
        focusable: false,
      }))
      this.flow.setEdges((edges) => [...edges.filter((edge) => next.has(edge.id)), ...made])
    }
    this.light()
  }

  edge = (id: string): EdgeSpec | undefined => this.edges.get(id)

  subscribeEdge(id: string, listener: () => void): () => void {
    this.edgeSubscribers.set(id, listener)
    return () => this.edgeSubscribers.delete(id)
  }

  isLit = (id: string) => this.lit.has(id)

  /** Opens the words that made an edge. */
  openEdge(id: string) {
    const spec = this.edges.get(id)
    if (!spec) return
    this.tip.set(null)
    // A link is on its entity's card, whichever of its rows says it.
    this.open.set({ name: spec.kind === "link" ? spec.target : spec.reader, at: spec.at[0] })
  }

  /** What's selected now. It lights its edges; the rest dim by one attribute on the stage, so only these edges hear. */
  select(names: string[]) {
    this.selected = new Set(names)
    this.light()
  }

  private light() {
    const lit = new Set<string>()
    if (this.selected.size) {
      for (const spec of this.edges.values()) if (this.selected.has(spec.source) || this.selected.has(spec.target)) lit.add(spec.id)
    }
    const was = this.lit
    this.lit = lit
    for (const id of lit) if (!was.has(id)) this.edgeSubscribers.get(id)?.()
    for (const id of was) if (!lit.has(id)) this.edgeSubscribers.get(id)?.()
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

  /** A new take, on the machine that's up; what Calcifer runs itself needs none. */
  async newTake(name: string) {
    const asset = this.get(name)?.asset
    if (!asset || !this.projectId) return
    const on = asset.tool === "cozy" ? "local" : this.machine()
    if (!on) return say(`No machine is up to make ${name}: rent one under Machines`)
    try {
      await this.client.render(this.projectId, [asset.name], [on], `take-${crypto.randomUUID()}`)
      this.refresh()
    } catch (error) {
      say(`Couldn't ask for a new take of ${name}`, error)
    }
  }

  /** Writes the one hash line: this version is what `name` is, until unpinned. */
  async pin(name: string, number: number) {
    if (!this.projectId) return
    try {
      const pinned = await this.client.approve(this.projectId, this.get(name)?.asset?.name ?? name, String(number))
      if (pinned.stale.length) say(`That puts ${pinned.stale.join(", ")} out of date`)
      this.refresh()
    } catch (error) {
      say(`Couldn't pin ${name}`, error)
    }
  }

  async unpin(name: string) {
    if (!this.projectId) return
    try {
      const unpinned = await this.client.unpin(this.projectId, this.get(name)?.asset?.name ?? name)
      if (unpinned.stale.length) say(`That puts ${unpinned.stale.join(", ")} out of date`)
      this.refresh()
    } catch (error) {
      say(`Couldn't unpin ${name}`, error)
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

  // An import

  /** This project has that file, as far as the page has read. */
  holdsFile(sha256: string): boolean {
    for (const view of this.views.values()) if (view.landed?.sha256 === sha256) return true
    return Boolean(
      this.state?.assets.some((asset) => asset.sha256 === sha256 || Object.values(asset.files ?? {}).some((file) => file.sha256 === sha256)),
    )
  }

  addPlacing(key: string, name: string, placing: Placing, at: Row, opts: { naming: boolean; preview?: string }) {
    this.views.set(key, { name, placing, draft: opts.naming ? name : undefined, preview: opts.preview })
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

  dragStarted(names: string[]) {
    for (const name of names) this.dragging.add(name)
  }

  dragStopped(nodes: ElementNode[]) {
    for (const node of nodes) {
      const name = node.data.name
      this.dragging.delete(name)
      if (!this.views.get(name)?.placing) this.move(name, toRow(node))
    }
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
