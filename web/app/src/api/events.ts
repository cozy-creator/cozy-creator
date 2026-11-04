import type { ServerEvent } from "../types"
import { signal } from "../ui/signal"
import { ApiError, type Client } from "./client"

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
  private stopping = new AbortController()
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
    navigator.locks
      .request(`cozy-events:${this.scope}`, { signal: this.stopping.signal }, () => this.hold())
      .catch(() => {})
  }

  stop() {
    this.stopping.abort()
    this.channel.close()
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

  private async hold() {
    this.holding = true
    const { signal: stopping } = this.stopping
    let attempt = 0
    while (!stopping.aborted) {
      try {
        const live = await this.follow()
        if (live) attempt = 0
      } catch (error) {
        if (error instanceof ApiError && error.status === 401) {
          this.say({ state: "signed_out" })
          return
        }
      }
      if (stopping.aborted) return
      attempt += 1
      this.say({ state: "reconnecting", attempt })
      await new Promise((done) => setTimeout(done, Math.min(5000, 250 * 2 ** (attempt - 1))))
    }
  }

  /** One connection, until it ends. Whether it ever said `hello`. */
  private async follow(): Promise<boolean> {
    const connection = new AbortController()
    const stop = () => connection.abort()
    this.stopping.signal.addEventListener("abort", stop)
    let last = Date.now()
    const watch = setInterval(() => {
      if (Date.now() - last > SILENT_MS) connection.abort()
    }, SILENT_MS / 3)
    let live = false
    try {
      const response = await this.client.events(connection.signal)
      if (response.status === 401) throw new ApiError(401, { kind: "credential", code: "unauthenticated", message: "Signed out" })
      if (!response.ok || !response.body) return false
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
      )
      return live
    } catch (error) {
      if (error instanceof ApiError) throw error
      return live
    } finally {
      clearInterval(watch)
      this.stopping.signal.removeEventListener("abort", stop)
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
  return (await lock(names[0], { signal }))!
}

/** One of the two live `<video>`s or `<audio>`s, across tabs. Resolves when it is ours; abort to stop waiting. */
export const holdMedia = (signal: AbortSignal) => ration(["cozy-media-0", "cozy-media-1"], signal)

/** The one run's log stream, across tabs. */
export const holdLog = (signal: AbortSignal) => ration(["cozy-log"], signal)
