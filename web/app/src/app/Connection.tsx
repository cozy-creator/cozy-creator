import { QueryClient } from "@tanstack/react-query"
import { createContext, useContext } from "react"
import { ApiError, Client, type RunFilter } from "../api/client"
import { EventStream } from "../api/events"
import { Outbox } from "../api/outbox"
import { nextProjectOrder } from "../api/projectOrder"
import { runsAfter, withAssets } from "../api/queries"
import { sameOrigin } from "../api/transport"
import type { Machine, MachineList, Run, RunList, StreamEvent, CalciferStatus, ProjectEventCursor } from "../api/calcifer"
import type { Project } from "../types"
import { revokeProjectMedia } from "../canvas/Media"
import type { CanvasStore } from "../canvas/CanvasStore"
import { signal } from "../ui/signal"
import { readSession } from "./session"
import { forgetProjectView, protectProjectView } from "./view"
import { notes } from "../ui/notes"

const READ_GAP_MS = 250
/** More than this changing at once is read whole. */
const MOST_ASSETS = 64

/** What's to read of a project, and when it last was. */
interface Owed {
  timer?: ReturnType<typeof setTimeout>
  read: number
  flying: boolean
  names: Set<string>
  gone: Set<string>
  head: boolean
  whole: boolean
  wholeCursor: ProjectEventCursor | null
}

/** Stream events into the query cache. The stream invalidates, so nothing goes stale by itself. */
class Bridge {
  /** Calcifer came back as another version: the page offers Reload. */
  readonly newVersion = signal<string | null>(null)
  private queries: QueryClient
  private client: Client
  private version: string | null = null
  private revs = new Map<string, string>()
  private owed = new Map<string, Owed>()
  private runs = new Map<string, Run>()
  private runListeners = new Set<(run: Run) => void>()
  private flushing = false
  private privateBlocked = new Set<string>()
  private privacyEpoch = new Map<string, number>()

  allowProject(id: string) { this.privateBlocked.delete(id); this.client.allowProject(id) }

  forgetProject(id: string) {
    this.client.retireProject(id)
    this.privateBlocked.add(id)
    this.privacyEpoch.set(id, (this.privacyEpoch.get(id) ?? 0) + 1)
    const held = this.owed.get(id)
    if (held?.timer) clearTimeout(held.timer)
    this.owed.delete(id)
    this.revs.delete(id)
    for (const [number, run] of this.runs) if (run.project_id === id) this.runs.delete(number)
  }

  constructor(queries: QueryClient, client: Client, private vaultChanged: (id: string, state: string) => void) {
    this.queries = queries
    this.client = client
  }

  projectEpoch(id: string) { return this.privacyEpoch.get(id) ?? 0 }

  setVersion(version: string) {
    this.version = version
  }

  /**
   * What the stream said changed of a project: at once if none was read lately, else one read per `READ_GAP_MS`, so a burst shows as it goes,
   * in few reads. One at a time, and one more if it changed during that read. Only what changed is read, whatever else the project holds.
   */
  changed(id: string, change: { assets?: string[] | null; gone?: string[]; head?: boolean; cursor?: ProjectEventCursor | null }) {
    if (this.privateBlocked.has(id)) return
    let slot = this.owed.get(id)
    if (!slot) this.owed.set(id, (slot = { read: 0, flying: false, names: new Set(), gone: new Set(), head: false, whole: false, wholeCursor: null }))
    for (const name of change.assets ?? []) slot.names.add(name)
    for (const name of change.gone ?? []) slot.gone.add(name)
    slot.head ||= Boolean(change.head)
    if (!change.assets) {
      const cursor = change.cursor ?? null
      if (!slot.whole) slot.wholeCursor = cursor
      else if (!cursor || !slot.wholeCursor || cursor.generation !== slot.wholeCursor.generation) slot.wholeCursor = null
      else if (cursor.sequence > slot.wholeCursor.sequence) slot.wholeCursor = cursor
      slot.whole = true
    }
    this.go(id, slot)
  }

  private go(id: string, held: Owed) {
    if (this.privateBlocked.has(id)) return
    if (held.flying || held.timer !== undefined) return
    held.timer = setTimeout(async () => {
      held.timer = undefined
      held.flying = true
      held.read = performance.now()
      await this.read(id, held)
      held.flying = false
      if (held.whole || held.head || held.names.size || held.gone.size) this.go(id, held)
    }, Math.max(0, held.read + READ_GAP_MS - performance.now()))
  }

  private async read(id: string, held: Owed) {
    if (this.privateBlocked.has(id)) return
    const epoch = this.privacyEpoch.get(id) ?? 0
    const key = ["project", id]
    const tooMany = held.names.size > MOST_ASSETS
    const whole = held.whole || tooMany
    const cursor = held.wholeCursor
    const [names, gone, head] = [[...held.names], [...held.gone], held.head]
    held.names.clear()
    held.gone.clear()
    Object.assign(held, { head: false, whole: false, wholeCursor: null })
    const query = this.queries.getQueryCache().find({ queryKey: key })
    // No one's looking: what's held is read when someone is.
    if (!query?.getObserversCount()) return void query?.invalidate()
    // A first read still going may be older than this: this waits for it, and then what changed is read over it.
    await query.promise?.catch(() => {})
    const state = this.queries.getQueryData<Project>(key)
    const covered = cursor && state?.covered && cursor.generation === state.covered.generation && cursor.sequence <= state.covered.sequence
    if (tooMany || (whole && !covered) || state === undefined) {
      await this.queries.refetchQueries({ queryKey: key })
      return
    }
    const began = nextProjectOrder()
    try {
      const [read, said] = await Promise.all([
        Promise.all(names.map((name) => this.client.asset(id, name).then(({ asset, heads }) => ({ asset, heads }), (error) => {
          if (error instanceof ApiError && error.status === 404) return null
          throw error
        }))),
        head ? this.client.projectHead(id) : undefined,
      ])
      const rows = read.map((row) => row?.asset ?? null)
      const heads = Object.fromEntries(read.flatMap((row) => (row?.heads?.length ? [[row.asset.name, row.heads]] : [])))
      if (this.privateBlocked.has(id) || (this.privacyEpoch.get(id) ?? 0) !== epoch) return
      const asked = rows.flatMap((row) => row ?? [])
      const left = [...gone, ...names.filter((_, at) => !rows[at])]
      if (asked.length || left.length || said) this.queries.setQueryData<Project>(key, (state) => state && { ...withAssets(state, asked, left, said, heads), began })
    } catch {
      // Whatever kept it from reading those, the next says why, of all of it.
      held.whole = true
      held.wholeCursor = null
    }
  }

  /** Everything held is read again, a project's whole, or when it's next looked at. With `behind`, what the stream lost may be newer than any read going, so that goes too. */
  private readAll(behind: boolean) {
    for (const query of this.queries.getQueryCache().findAll({ queryKey: ["project"] })) {
      const id = query.queryKey[1]
      if (typeof id !== "string" || !id) continue
      if (!query.getObserversCount()) query.invalidate()
      else if (behind || query.state.fetchStatus !== "fetching") this.changed(id, {})
    }
    void this.queries.invalidateQueries({ predicate: ({ queryKey }) => queryKey[0] !== "project" }, { cancelRefetch: behind })
  }

  onRun(listener: (run: Run) => void): () => void {
    this.runListeners.add(listener)
    return () => this.runListeners.delete(listener)
  }

  private flushRuns = () => {
    this.flushing = false
    const batch = [...this.runs.values()].filter((run) => !run.project_id || !this.privateBlocked.has(run.project_id))
    this.runs.clear()
    for (const run of batch) {
      this.queries.setQueryData(run.project_id ? ["project-run", run.project_id, run.number] : ["maintenance-run", run.number], run)
      for (const listener of this.runListeners) listener(run)
    }
    const ended = batch.some((run) => run.status !== "queued" && run.status !== "in_progress")
    // An install, a removal or a machine's update that ends can change what a tool reads as, and so does local code's call: the machine's reading
    // of an install is kept as its call is asked, after the install's own end. The list is read again.
    if (batch.some((run) => run.status === "completed" && (run.kind !== "call" || run.target.startsWith("local/")))) void this.queries.invalidateQueries({ queryKey: ["tools"] })
    // A list shows a run it holds as it changes; one it doesn't hold may be new to it, so it's read again.
    for (const [key, list] of this.queries.getQueriesData<RunList>({ predicate: ({ queryKey }) => queryKey[0] === "project-runs" || queryKey[0] === "maintenance-runs" })) {
      if (!list) continue
      const scoped = key[0] === "project-runs" ? batch.filter((run) => run.project_id === key[1]) : batch.filter((run) => !run.project_id)
      if (!scoped.length) continue
      const after = runsAfter(list, key[key[0] === "project-runs" ? 2 : 1] as RunFilter, scoped)
      this.queries.setQueryData<RunList>(key, after.list)
      // A read begun before a run ended answers with it still going, and nothing after it says otherwise.
      const stale = ended && this.queries.isFetching({ queryKey: key, exact: true }) > 0
      if (after.again || stale) void this.queries.invalidateQueries({ queryKey: key, exact: true }, { cancelRefetch: stale })
    }
  }

  private machines(change: (machines: Machine[]) => Machine[]) {
    this.queries.setQueryData<MachineList>(["machines"], (list) => list && { ...list, machines: change(list.machines) })
  }

  handle = (event: StreamEvent) => {
    switch (event.type) {
      case "hello":
        this.runs.clear()
        for (const [id, epoch] of this.privacyEpoch) this.privacyEpoch.set(id, epoch + 1)
        this.vaultChanged("", "recheck")
        if (this.version && event.version !== this.version) this.newVersion.set(event.version)
        this.readAll(false)
        break
      case "resync":
        this.runs.clear()
        for (const [id, epoch] of this.privacyEpoch) this.privacyEpoch.set(id, epoch + 1)
        this.vaultChanged("", "recheck")
        this.readAll(true)
        break
      case "run.updated":
        if (event.run.project_id && this.privateBlocked.has(event.run.project_id)) break
        this.runs.set(`${event.run.project_id ?? "maintenance"}\0${event.run.id}`, event.run)
        if (!this.flushing) {
          this.flushing = true
          setTimeout(this.flushRuns, 16)
        }
        break
      case "machine.updated":
        this.machines((machines) => {
          const known = machines.some((machine) => machine.name === event.machine.name)
          return known ? machines.map((machine) => (machine.name === event.machine.name ? event.machine : machine)) : [...machines, event.machine]
        })
        break
      case "machine.removed":
        this.machines((machines) => machines.filter((machine) => machine.name !== event.name))
        break
      case "project.vault": {
        if (event.state === "plain") {
          this.forgetProject(event.id)
          // Discard old status work before the purge rerenders Shell. Only its
          // fresh status reply can release private read and persistence guards.
          this.queries.removeQueries({ queryKey: ["project-vault", event.id], exact: true })
          this.vaultChanged(event.id, event.state)
          break
        }
        if (event.state === "unlocked") this.allowProject(event.id)
        else this.forgetProject(event.id)
        this.queries.setQueryData(["project-vault", event.id], (held: import("../api/calcifer").VaultStatus | undefined) => ({
          ...(held ?? { path: "", idle_timeout_s: 1800 }), encrypted: true, state: event.state,
          ...(event.state === "unlocked" ? {} : { view: null, expires_at_ms: null }),
        }))
        this.vaultChanged(event.id, event.state)
        void this.queries.invalidateQueries({ queryKey: ["project-vault", event.id] })
        break
      }
      case "project.changed":
        const revision = event.cursor ? JSON.stringify([event.cursor.generation, event.cursor.sequence]) : event.rev
        if (this.revs.get(event.id) !== revision) {
          this.revs.set(event.id, revision)
          this.changed(event.id, event)
        }
        break
    }
  }
}

export interface Connection {
  client: Client
  queries: QueryClient
  stream: EventStream
  bridge: Bridge
  /** What a person did, in order, on its way to this Calcifer, whichever project it's in. */
  outbox: Outbox
  /** Each project's canvas as it was last shown, by its id. */
  canvases: Map<string, CanvasStore>
  /** Remount mixed-project drawers after a lock, dropping their local logs and snapshots. */
  closeBlockers: Set<() => boolean>
  retireProject(id: string): void
  privateViewEpoch: ReturnType<typeof signal<number>>
  /** Which try a call is on, when it isn't the first; 0 otherwise. */
  attempt: ReturnType<typeof signal<number>>
  signedOut: ReturnType<typeof signal<boolean>>
  status: CalciferStatus
}

export type Boot = { connection: Connection } | { failed: "signed_out" | "unreachable" }

export async function connect(): Promise<Boot> {
  const attempt = signal(0)
  const signedOut = signal(false)
  const client = new Client(sameOrigin(readSession), { signedOut: () => signedOut.set(true), attempt: (n: number) => attempt.set(n) })
  let status: CalciferStatus
  try {
    status = await client.status()
  } catch {
    if (signedOut.get()) return { failed: "signed_out" }
    return { failed: "unreachable" }
  }
  const queries = new QueryClient({
    defaultOptions: {
      queries: { staleTime: Infinity, retry: false, refetchOnWindowFocus: false, refetchOnReconnect: false },
    },
  })
  const stream = new EventStream(client)
  const outbox = new Outbox()
  const canvases = new Map<string, CanvasStore>()
  const privateViewEpoch = signal(0)
  const retireProject = (id: string) => {
    bridge.forgetProject(id)
    outbox.discardProject(id)
    canvases.get(id)?.retire()
    canvases.delete(id)
    forgetProjectView(id)
    revokeProjectMedia(id)
    const privateData = ({ queryKey }: { queryKey: readonly unknown[] }) =>
      ((["canvas", "history"].includes(String(queryKey[0])) || String(queryKey[0]).startsWith("project")) && queryKey[1] === id)
      || queryKey[0] === "tools"
    void queries.cancelQueries({ predicate: privateData })
    void queries.resetQueries({ predicate: privateData })
    queries.removeQueries({ predicate: privateData })
    notes.set([])
    privateViewEpoch.set(privateViewEpoch.get() + 1)
  }
  const bridge = new Bridge(queries, client, (id, state) => {
    if (id) protectProjectView(id, true)
    if (state === "unlocked") { outbox.allowProject(id); return }
    if (id) retireProject(id)
    else privateViewEpoch.set(privateViewEpoch.get() + 1)
  })
  const closeBlockers = new Set<() => boolean>()
  const stopEdit = (event: Event) => { event.preventDefault(); event.stopImmediatePropagation() }
  const editEvents = ["pointerdown", "keydown", "beforeinput", "input", "drop", "paste", "submit"]
  const hooks = globalThis as typeof globalThis & { __cozyFreeze?: () => boolean; __cozyPrepareClose?: () => Promise<void>; __cozyResume?: () => void }
  hooks.__cozyFreeze = () => {
    if ([...closeBlockers].some((blocked) => blocked())) return false
    if (document.activeElement instanceof HTMLElement) document.activeElement.blur()
    document.documentElement.inert = true
    for (const name of editEvents) document.addEventListener(name, stopEdit, true)
    return true
  }
  hooks.__cozyPrepareClose = async () => {
    await Promise.all([...canvases.values()].map((canvas) => canvas.prepareClose()))
    await outbox.saved()
    await client.savedWrites()
    await outbox.saved()
    outbox.freeze(); client.freezeWrites()
  }
  hooks.__cozyResume = () => {
    for (const name of editEvents) document.removeEventListener(name, stopEdit, true)
    document.documentElement.inert = false
    outbox.resume(); client.resumeWrites()
  }
  bridge.setVersion(status.version)
  stream.subscribe(bridge.handle)
  stream.subscribe((event) => {
    if (event.type === "hello") outbox.back()
  })
  stream.start()
  return { connection: { client, queries, stream, bridge, outbox, canvases, closeBlockers, retireProject, privateViewEpoch, attempt, signedOut, status } }
}

const Context = createContext<Connection | null>(null)
export const ConnectionProvider = Context.Provider

export function useConnection(): Connection {
  const connection = useContext(Context)
  if (!connection) throw new Error("no connection")
  return connection
}
