import type { StreamEvent } from "./calcifer"
import { signal } from "../ui/signal"
import type { Client } from "./client"

export type Health =
  | { state: "connecting" }
  | { state: "live" }
  | { state: "reconnecting"; attempt: number }
  | { state: "signed_out" }

type Shared =
  | { event: StreamEvent }
  | { health: Health }
  | { ask: true }
  | { shows: { tab: string; project: string | null } }
  | { who: true }

/** Three missed comments: the connection is dead, whatever the socket says. */
const SILENT_MS = 45_000

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
 * `GET /v1/events`, one per Calcifer across all tabs: a browser allows six connections to a host. The tab that wins the Web Lock
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
  private scope: string
  private remote: boolean
  private tab = crypto.randomUUID()
  /** Once this tab holds its own lock: what it says it shows is said after. */
  private alive: Promise<void>
  /** The project this tab shows. */
  private showing: string | null = null
  /** The holding tab's: what each tab shows, by tab, the stream's id, and what Calcifer was last told it shows. */
  private shown = new Map<string, string | null>()
  private stream: string | null = null
  private told: string | null = null
  private telling = false

  constructor(client: Client, scope: string, remote: boolean) {
    this.client = client
    this.scope = scope
    this.remote = remote
    this.channel = new BroadcastChannel(`cozy-events:${scope}`)
    this.channel.onmessage = ({ data }: MessageEvent<Shared>) => {
      if ("event" in data) this.dispatch(data.event)
      else if ("health" in data) this.health.set(data.health)
      else if ("shows" in data) this.heard(data.shows.tab, data.shows.project)
      else if ("who" in data) this.announce()
      else if (this.holding) this.channel.postMessage({ health: this.health.get() } satisfies Shared)
    }
    this.alive = new Promise((held) => {
      void navigator.locks.request(`cozy-tab:${scope}:${this.tab}`, () => {
        held()
        return new Promise<void>(() => {})
      })
    })
  }

  /** This tab shows `project`, or none. */
  show(project: string | null) {
    this.showing = project
    if (this.holding) this.heard(this.tab, project)
    else this.announce()
  }

  private announce() {
    void this.alive.then(() => this.channel.postMessage({ shows: { tab: this.tab, project: this.showing } } satisfies Shared))
  }

  /** The holding tab hears what `tab` shows, and waits on its lock to hear when it goes. */
  private heard(tab: string, project: string | null) {
    if (!this.holding) return
    if (tab !== this.tab && !this.shown.has(tab)) {
      void navigator.locks.request(`cozy-tab:${this.scope}:${tab}`, () => {
        this.shown.delete(tab)
        void this.tell()
      })
    }
    this.shown.set(tab, project)
    void this.tell()
  }

  /** Calcifer told what the tabs show now, one call at a time. */
  private async tell() {
    if (this.telling || !this.stream) return
    const projects = [...new Set([...this.shown.values()].filter((project) => project !== null))].sort()
    const key = projects.join()
    if (key === this.told) return
    this.telling = true
    const stream = this.stream
    try {
      await this.client.show(stream, projects)
      if (this.stream === stream) this.told = key
    } finally {
      this.telling = false
    }
    void this.tell()
  }

  subscribe(listener: (event: StreamEvent) => void): () => void {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }

  start() {
    this.channel.postMessage({ ask: true } satisfies Shared)
    void navigator.locks.request(`cozy-events:${this.scope}`, () => this.hold())
  }

  private dispatch(event: StreamEvent) {
    for (const listener of this.listeners) listener(event)
  }

  private say(health: Health) {
    this.health.set(health)
    this.channel.postMessage({ health } satisfies Shared)
  }

  private publish(event: StreamEvent) {
    this.dispatch(event)
    this.channel.postMessage({ event } satisfies Shared)
  }

  /** Holds the lock for as long as this tab lives, unless this computer's Calcifer says it's signed out. */
  private async hold() {
    this.holding = true
    this.shown.set(this.tab, this.showing)
    this.channel.postMessage({ who: true } satisfies Shared)
    for (let attempt = 1; ; attempt++) {
      try {
        if (await this.follow()) attempt = 1
      } catch {
        if (!this.remote) {
          this.say({ state: "signed_out" })
          return
        }
        this.client.unauthorized()
      }
      this.say({ state: "reconnecting", attempt })
      await new Promise((done) => setTimeout(done, Math.min(5000, 250 * 2 ** (attempt - 1))))
    }
  }

  /** One connection, until it ends. Whether it ever said `hello`; throws if signed out. */
  private async follow(): Promise<boolean> {
    const connection = new AbortController()
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
            this.say({ state: "live" })
            // A new stream holds nothing open yet.
            this.stream = event.stream ?? null
            this.told = null
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
