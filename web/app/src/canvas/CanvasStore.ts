import type { Node, ReactFlowInstance } from "@xyflow/react"
import type { Client } from "../api/client"
import type { Asset, Imported, ProjectState, Row, Run } from "../types"
import { say } from "../ui/notes"
import { asName, freeName } from "./names"

export type ElementData = { name: string }
export type ElementNode = Node<ElementData, "element">

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
const GAP = 32

type Flow = ReactFlowInstance<ElementNode>

const samePlace = (a: { x: number; y: number }, b: { x: number; y: number }) => a.x === b.x && a.y === b.y

export function toRow(node: ElementNode): Row {
  return { x: Math.round(node.position.x), y: Math.round(node.position.y), w: node.width ?? CARD_WIDTH }
}

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
  }

  /** Fell behind: what the stream would have told us is about to be read again. */
  clearMasks() {
    this.masks.clear()
  }

  /** What a run says of the asset it's making. */
  applyRun(run: Run) {
    const owner = this.owners.get(run.number)
    if (owner) this.patch(owner, { run })
  }

  private reconcile() {
    const state = this.state!
    const rows = this.rows!
    const present = new Set(state.assets.map((asset) => asset.name))
    for (const [from, to] of this.renames) if (present.has(to)) this.renames.delete(from)
    const becoming = new Set(this.renames.values())
    const fresh: Asset[] = []
    this.owners.clear()
    for (const asset of state.assets) {
      for (const number of asset.making ?? []) this.owners.set(number, asset.name)
      if (this.renames.has(asset.name)) continue
      const view = this.views.get(asset.name)
      if (view) {
        if (view.asset !== asset || view.landed) {
          this.patch(asset.name, { asset, landed: undefined, ...(asset.making?.length ? {} : { run: undefined }) })
        }
      } else if (this.adding === 0) {
        // While an import is being asked, a new name may be that import's: its answer says, then this runs again.
        fresh.push(asset)
      }
    }
    for (const [name, view] of this.views) {
      if (!present.has(name) && !view.placing && !view.landed && !becoming.has(name)) this.removeNode(name)
    }
    this.addNodes(fresh.filter((asset) => rows.has(asset.name)).map((asset) => ({ asset, row: rows.get(asset.name)! })))
    const homeless = fresh.filter((asset) => !rows.has(asset.name)).map((asset) => asset.name)
    if (homeless.length) void this.findHomes(homeless)
    if (!this.seen) {
      this.seen = true
      this.onFirstRead?.(state.assets.length > 0)
    }
  }

  /** A name with no row may just be ahead of its row on the stream, so ask once. Still none: place it, once, and write the row. */
  private async findHomes(names: string[]) {
    if (!this.projectId) return
    let cards: Record<string, Row> = {}
    try {
      cards = (await this.client.canvas(this.projectId)).cards
    } catch (error) {
      say("Couldn't read where things sit", error)
    }
    const rows = this.rows!
    const state = this.state!
    const wanted = names.filter((name) => !this.views.has(name) && state.assets.some((asset) => asset.name === name))
    const assets = new Map(state.assets.map((asset) => [asset.name, asset]))
    const found = wanted.filter((name) => cards[name])
    for (const name of found) rows.set(name, cards[name])
    this.addNodes(found.map((name) => ({ asset: assets.get(name)!, row: cards[name] })))
    const left = wanted.filter((name) => !cards[name])
    if (!left.length) return
    const homes = this.nextHomes(left.length)
    this.addNodes(left.map((name, i) => ({ asset: assets.get(name)!, row: homes[i] })))
    left.forEach((name, i) => this.move(name, homes[i]))
  }

  /** Right of what's there, top down. Reading order stands in for graph depth until `reads` is in the state. */
  private nextHomes(count: number): Row[] {
    const nodes = this.flow?.getNodes() ?? []
    const right = nodes.length ? Math.max(...nodes.map((node) => node.position.x + (node.width ?? CARD_WIDTH))) + GAP : 0
    const top = nodes.length ? Math.min(...nodes.map((node) => node.position.y)) : 0
    const perColumn = 6
    return Array.from({ length: count }, (_, i) => ({
      x: right + Math.floor(i / perColumn) * (CARD_WIDTH + GAP),
      y: top + (i % perColumn) * (CARD_WIDTH * 0.75 + GAP),
      w: CARD_WIDTH,
    }))
  }

  private addNodes(items: { asset: Asset; row: Row }[]) {
    if (!items.length) return
    const nodes = items.map(({ asset, row }) => {
      this.views.set(asset.name, { name: asset.name, asset })
      for (const number of asset.making ?? []) this.owners.set(number, asset.name)
      return this.node(asset.name, row, this.selected.has(asset.name))
    })
    this.flow?.addNodes(nodes)
  }

  private node(name: string, row: Row, selected = false): ElementNode {
    const id = `e${++this.nextId}`
    this.ids.set(name, id)
    const width = row.w ?? CARD_WIDTH
    return {
      id,
      type: "element",
      position: { x: row.x, y: row.y },
      data: { name },
      width,
      initialHeight: Math.round(width * 0.75) + 30,
      selected,
    }
  }

  private removeNode(name: string) {
    const id = this.ids.get(name)
    this.views.delete(name)
    this.ids.delete(name)
    this.masks.delete(name)
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
    this.views.set(to, { ...view, name: to })
    this.ids.set(to, id)
    this.aliases.set(from, to)
    this.aliases.delete(to)
    const row = this.rows?.get(from)
    if (row) {
      this.rows!.delete(from)
      this.rows!.set(to, row)
    }
    this.notify(to)
    this.flow?.updateNodeData(id, { name: to })
  }

  // An import

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
      const name = freeName(other === key ? (view.draft ?? view.name) : view.name, taken)
      if (other === key) return name
      taken.push(name)
    }
    return key
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
