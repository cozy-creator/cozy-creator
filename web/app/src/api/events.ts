import type { ProjectShowRefusal, ProjectShowReply, StreamEvent } from "./calcifer"
import { signal } from "../ui/signal"
import { ApiError, type Client } from "./client"

export type Health =
  | { state: "connecting" }
  | { state: "live" }
  | { state: "reconnecting"; attempt: number }
  | { state: "signed_out" }

type Presence = { tab: string; project: string | null; revision: number }
type Acknowledged = Presence & { stream: string; refusal?: ProjectShowRefusal }
type Waiter = { project: string; done(): void; fail(error: unknown): void }
type Confirmed = { stream: string; key: string; readers: number; reply: ProjectShowReply | null }

type Shared =
  | { event: StreamEvent }
  | { health: Health; stream: string | null }
  | { ask: true }
  | { shows: Presence }
  | { shown: Acknowledged }
  | { who: true }

/** Three missed comments: the connection is dead, whatever the socket says. */
const SILENT_MS = 45_000
// Vault-aware coordination ABI: older pages cannot invalidate cached Reader grants on lock.
// Keep tab liveness, media/log locks and ordinary storage shared across page generations.
const COORDINATION = "cozy-events-presence-v2"

/** Server-sent events off a `fetch` body, so one parser serves every transport. */
export async function readEvents(body: ReadableStream<Uint8Array<ArrayBuffer>>, onData: (data: string) => void, onBytes?: () => void) {
  const reader = body.pipeThrough(new TextDecoderStream()).getReader()
  let buffer = ""
  let data: string[] = []
  for (;;) {
    const { value, done } = await reader.read()
    if (done) return
    onBytes?.()
    buffer += value
    for (let end = buffer.indexOf("\n"); end >= 0; end = buffer.indexOf("\n")) {
      const line = buffer.slice(0, end).replace(/\r$/, "")
      buffer = buffer.slice(end + 1)
      if (line === "") {
        if (data.length) onData(data.join("\n"))
        data = []
      } else if (line.startsWith("data:")) {
        data.push(line.slice(5).replace(/^ /, ""))
      }
    }
  }
}

/**
 * `GET /v1/events`, shared by compatible page protocols: a browser allows six connections to a host. The tab that wins the Web Lock
 * holds it and shares it by `BroadcastChannel`; when that tab goes, the next waiting takes the lock. It tells Calcifer which project
 * each tab shows (`PUT /v1/events/{stream}`), which stays open while one does: each tab says what it shows, and holds a lock of its own
 * for its life, which the holding tab waits on to hear it's gone.
 */
export class EventStream {
  readonly health = signal<Health>({ state: "connecting" })
  private listeners = new Set<(event: StreamEvent) => void>()
  private channel: BroadcastChannel
  private holding = false
  private client: Client
  private tab = crypto.randomUUID()
  /** Once this tab holds its own lock: what it says it shows is said after. */
  private alive: Promise<void>
  /** The project this tab shows. */
  private showing: string | null = null
  private revision = 0
  private acknowledged: Acknowledged | null = null
  private waiters = new Set<Waiter>()
  private epoch: string | null = null
  /** The holding tab's: what each tab shows, by tab, the stream's id, and what Calcifer was last told it shows. */
  private shown = new Map<string, Presence>()
  private stream: string | null = null
  private following: AbortController | null = null
  private told: Confirmed | null = null
  private telling = false
  /** Lock drops server Readers even when the displayed project set later returns to the same IDs. */
  private readers = 0

  constructor(client: Client) {
    this.client = client
    this.channel = new BroadcastChannel(COORDINATION)
    this.channel.onmessage = ({ data }: MessageEvent<Shared>) => {
      if ("event" in data) this.dispatch(data.event)
      else if ("health" in data) this.heardHealth(data.health, data.stream)
      else if ("shows" in data) this.heard(data.shows)
      else if ("shown" in data) this.heardAcknowledgement(data.shown)
      else if ("who" in data) this.announce()
      else if (this.holding) this.channel.postMessage({ health: this.health.get(), stream: this.stream } satisfies Shared)
    }
    this.alive = new Promise((held) => {
      void navigator.locks.request(`cozy-tab:${this.tab}`, () => {
        held()
        return new Promise<void>(() => {})
      })
    })
  }

  /** This tab shows `project`, or none. */
  show(project: string | null) {
    if (project !== this.showing) {
      const before = this.showing
      this.showing = project
      this.revision++
      this.acknowledged = null
      for (const waiter of this.waiters) {
        if (waiter.project === before) waiter.fail(new DOMException("Project is no longer shown", "AbortError"))
      }
    }
    if (this.holding) this.heard(this.presence())
    else this.announce()
  }

  /** Await this tab's held acknowledgement (legacy services confirm completion only). Owns no Reader. */
  whenShown(project: string, signal: AbortSignal): Promise<void> {
    signal.throwIfAborted()
    return new Promise((done, fail) => {
      const remove = () => {
        this.waiters.delete(waiter)
        signal.removeEventListener("abort", aborted)
      }
      const waiter: Waiter = {
        project,
        done: () => { remove(); done() },
        fail: (error) => { remove(); fail(error) },
      }
      const aborted = () => waiter.fail(signal.reason)
      this.waiters.add(waiter)
      signal.addEventListener("abort", aborted, { once: true })
      this.releaseWaiters()
    })
  }

  private presence(): Presence {
    return { tab: this.tab, project: this.showing, revision: this.revision }
  }

  private announce() {
    void this.alive.then(() => this.channel.postMessage({ shows: this.presence() } satisfies Shared))
  }

  /** The holding tab hears what `tab` shows, and waits on its lock to hear when it goes. */
  private heard(presence: Presence) {
    if (!this.holding) return
    const { tab } = presence
    if (tab !== this.tab && !this.shown.has(tab)) {
      void navigator.locks.request(`cozy-tab:${tab}`, () => {
        this.shown.delete(tab)
        void this.tell()
      })
    }
    const before = this.shown.get(tab)
    if (before && before.revision > presence.revision) return
    this.shown.set(tab, presence)
    void this.tell()
  }

  private heardAcknowledgement(acknowledged: Acknowledged) {
    if (acknowledged.tab !== this.tab || acknowledged.revision !== this.revision || acknowledged.project !== this.showing || acknowledged.stream !== this.epoch) return
    this.acknowledged = acknowledged
    this.releaseWaiters()
  }

  private releaseWaiters() {
    const ack = this.acknowledged
    if (!ack || ack.stream !== this.epoch || this.health.get().state !== "live") return
    for (const waiter of this.waiters) {
      if (waiter.project !== ack.project) continue
      if (ack.refusal) waiter.fail(new ApiError(ack.refusal.status, ack.refusal.error))
      else waiter.done()
    }
  }

  private confirm(snapshot: Presence[], confirmed: Confirmed) {
    for (const presence of snapshot) {
      const now = this.shown.get(presence.tab)
      if (!now || now.revision !== presence.revision || now.project !== presence.project) continue
      const project = presence.project
      // A legacy204 confirms completion only; a current reply proves which IDs were held.
      const refusal = project && confirmed.reply && !confirmed.reply.held.includes(project)
        ? confirmed.reply.refused[project] ?? { status: 503, error: { kind: "unavailable" as const, code: "project_not_shown", message: "Calcifer did not hold this project open", next: [] } }
        : undefined
      const shown: Acknowledged = { ...presence, stream: confirmed.stream, refusal: refusal || undefined }
      this.heardAcknowledgement(shown)
      this.channel.postMessage({ shown } satisfies Shared)
    }
  }

  /** Calcifer told what the tabs show now, one call at a time. */
  private async tell() {
    if (this.telling || !this.stream || !this.following || this.following.signal.aborted) return
    const snapshot = [...this.shown.values()]
    const projects = [...new Set(snapshot.flatMap(({ project }) => project === null ? [] : [project]))].sort()
    const key = projects.join()
    const stream = this.stream
    const readers = this.readers
    if (this.told?.stream === stream && this.told.key === key && this.told.readers === readers) {
      this.confirm(snapshot, this.told)
      return
    }
    this.telling = true
    try {
      const reply = await this.client.show(stream, projects, this.following.signal)
      if (this.stream === stream && this.readers === readers) {
        this.told = { stream, key, readers, reply }
        this.confirm(snapshot, this.told)
      }
    } catch (error) {
      // The stream's existing reconnect loop owns recovery; a failed show is never an ack.
      if (this.stream === stream && this.readers === readers) this.following?.abort(error)
      return
    } finally {
      this.telling = false
      // A newer hello may have arrived while an older show was pending, even if that old
      // request failed. Reconcile the current stream; its aborted-follow guard prevents retry.
      void this.tell()
    }
  }

  subscribe(listener: (event: StreamEvent) => void): () => void {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }

  start() {
    this.channel.postMessage({ ask: true } satisfies Shared)
    void navigator.locks.request(COORDINATION, () => this.hold())
  }

  private dispatch(event: StreamEvent) {
    if (event.type === "hello" && this.epoch !== event.stream) {
      this.epoch = event.stream
      this.acknowledged = null
    }
    if (event.type === "project.vault") {
      this.readers++
      this.told = null
      if (this.showing === event.id) {
        this.revision++
        this.acknowledged = null
        if (event.state !== "unlocked") {
          for (const waiter of this.waiters) {
            if (waiter.project === event.id) waiter.fail(new DOMException("Project access changed", "AbortError"))
          }
        }
        this.show(this.showing)
      } else if (this.holding) void this.tell()
    }
    for (const listener of this.listeners) listener(event)
  }

  private heardHealth(health: Health, stream: string | null) {
    if (health.state !== "live" || this.epoch !== stream) this.acknowledged = null
    this.epoch = health.state === "live" ? stream : null
    this.health.set(health)
    if (health.state === "signed_out") {
      for (const waiter of this.waiters) waiter.fail(new ApiError(401, { kind: "credential", code: "unauthenticated", message: "Sign in again", next: ["cozy open"] }))
    }
  }

  private say(health: Health) {
    this.heardHealth(health, this.stream)
    this.channel.postMessage({ health, stream: this.stream } satisfies Shared)
  }

  private publish(event: StreamEvent) {
    this.dispatch(event)
    this.channel.postMessage({ event } satisfies Shared)
  }

  /** Holds the lock for as long as this tab lives, unless this computer's Calcifer says it's signed out. */
  private async hold() {
    this.holding = true
    this.shown.set(this.tab, this.presence())
    this.channel.postMessage({ who: true } satisfies Shared)
    for (let attempt = 1; ; attempt++) {
      try {
        if (await this.follow()) attempt = 1
      } catch {
        this.say({ state: "signed_out" })
        return
      }
      this.say({ state: "reconnecting", attempt })
      await new Promise((done) => setTimeout(done, Math.min(5000, 250 * 2 ** (attempt - 1))))
    }
  }

  /** One connection, until it ends. Whether it ever said `hello`; throws if signed out. */
  private async follow(): Promise<boolean> {
    const connection = new AbortController()
    this.following = connection
    let last = Date.now()
    const watch = setInterval(() => {
      if (Date.now() - last > SILENT_MS) connection.abort()
    }, SILENT_MS / 3)
    let live = false
    try {
      const response = await this.client.events(connection.signal).catch(() => null)
      if (response?.status === 401) throw new Error("signed out")
      if (!response?.ok || !response.body) return false
      await readEvents(
        response.body,
        (data) => {
          const event = parse(data)
          if (!event) return
          if (event.type === "hello") {
            live = true
            // A new stream holds nothing open yet.
            this.stream = event.stream ?? null
            this.told = null
            this.say({ state: "live" })
            void this.tell()
          }
          this.publish(event)
        },
        () => {
          last = Date.now()
        },
      ).catch(() => {})
      return live
    } finally {
      this.stream = null
      this.following = null
      this.epoch = null
      this.acknowledged = null
      clearInterval(watch)
    }
  }
}

function parse(data: string): StreamEvent | null {
  try {
    const event = JSON.parse(data)
    return typeof event?.type === "string" ? event : null
  } catch {
    return null
  }
}

function lock(name: string, options: LockOptions): Promise<(() => void) | null> {
  return new Promise((resolve, reject) => {
    navigator.locks
      .request(name, options, (held) => {
        if (!held) {
          resolve(null)
          return
        }
        return new Promise<void>((release) => resolve(release))
      })
      .catch(reject)
  })
}

async function ration(names: string[], signal: AbortSignal): Promise<() => void> {
  for (const name of names) {
    const release = await lock(name, { ifAvailable: true })
    if (release) return release
  }
  // All taken: in line for each, keeping whichever comes first.
  const settled = new AbortController()
  const waits = names.map((name) => lock(name, { signal: AbortSignal.any([signal, settled.signal]) }))
  const first = (await Promise.any(waits))!
  settled.abort()
  for (const wait of waits) {
    wait.then((release) => {
      if (release !== first) release?.()
    }, () => {})
  }
  return first
}

/** What's playing in this tab, oldest first: a third stops the first. */
export const playingHere: (() => void)[] = []

/** One of the two live `<video>`s or `<audio>`s, across tabs. Resolves when it is ours; abort to stop waiting. */
export const holdMedia = (signal: AbortSignal) => ration(["cozy-media-0", "cozy-media-1"], signal)

/** The one run's log stream, across tabs. */
export const holdLog = (signal: AbortSignal) => ration(["cozy-log"], signal)
