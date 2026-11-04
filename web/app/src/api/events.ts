import type { ServerEvent } from "../types"
import { signal } from "../ui/signal"
import type { Client } from "./client"

export type Health =
  | { state: "connecting" }
  | { state: "live" }
  | { state: "reconnecting"; attempt: number }
  | { state: "signed_out" }

type Shared = { event: ServerEvent } | { health: Health } | { ask: true }

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
 * holds it and shares it by `BroadcastChannel`; when that tab goes, the next waiting takes the lock.
 */
export class EventStream {
  readonly health = signal<Health>({ state: "connecting" })
  private listeners = new Set<(event: ServerEvent) => void>()
  private channel: BroadcastChannel
  private holding = false
  private client: Client
  private scope: string

  constructor(client: Client, scope: string) {
    this.client = client
    this.scope = scope
    this.channel = new BroadcastChannel(`cozy-events:${scope}`)
    this.channel.onmessage = ({ data }: MessageEvent<Shared>) => {
      if ("event" in data) this.dispatch(data.event)
      else if ("health" in data) this.health.set(data.health)
      else if (this.holding) this.channel.postMessage({ health: this.health.get() } satisfies Shared)
    }
  }

  subscribe(listener: (event: ServerEvent) => void): () => void {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }

  start() {
    this.channel.postMessage({ ask: true } satisfies Shared)
    void navigator.locks.request(`cozy-events:${this.scope}`, () => this.hold())
  }

  private dispatch(event: ServerEvent) {
    for (const listener of this.listeners) listener(event)
  }

  private say(health: Health) {
    this.health.set(health)
    this.channel.postMessage({ health } satisfies Shared)
  }

  private publish(event: ServerEvent) {
    this.dispatch(event)
    this.channel.postMessage({ event } satisfies Shared)
  }

  /** Holds the lock for as long as this tab lives, unless Calcifer says it's signed out. */
  private async hold() {
    this.holding = true
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
          }
          this.publish(event)
        },
        () => {
          last = Date.now()
        },
      ).catch(() => {})
      return live
    } finally {
      clearInterval(watch)
    }
  }
}

function parse(data: string): ServerEvent | null {
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

/** One of the two live `<video>`s or `<audio>`s, across tabs. Resolves when it is ours; abort to stop waiting. */
export const holdMedia = (signal: AbortSignal) => ration(["cozy-media-0", "cozy-media-1"], signal)

/** The one run's log stream, across tabs. */
export const holdLog = (signal: AbortSignal) => ration(["cozy-log"], signal)
