import type {
  Canvas,
  ErrorBody,
  Held,
  Import,
  Imported,
  MachineList,
  Opened,
  ProjectList,
  ProjectState,
  Renamed,
  Row,
  Session,
  Status,
} from "../types"
import type { Transport } from "./transport"

export const TRIES = 5

export class ApiError extends Error {
  readonly body: ErrorBody
  readonly status: number

  constructor(status: number, body: ErrorBody) {
    super(body.message)
    this.status = status
    this.body = body
  }
}

const unreachable = () =>
  new ApiError(0, { kind: "unavailable", code: "calcifer_unreachable", message: "Calcifer isn't answering", next: ["cozy web"] })

async function refusal(response: Response): Promise<ApiError> {
  const body = await response.json().catch(() => null)
  const error = body?.error
  if (typeof error?.message === "string") return new ApiError(response.status, error)
  return new ApiError(response.status, { kind: "internal", code: `http_${response.status}`, message: `Calcifer answered ${response.status}` })
}

const aborted = (error: unknown) => error instanceof DOMException && error.name === "AbortError"

function pause(ms: number, signal?: AbortSignal | null): Promise<void> {
  return new Promise((done, fail) => {
    signal?.throwIfAborted()
    const timer = setTimeout(done, ms)
    signal?.addEventListener("abort", () => {
      clearTimeout(timer)
      fail(signal.reason)
    })
  })
}

export interface Options {
  /** Answers other than 2xx that are still answers. */
  accept?: number[]
  /** Said for each try after the first; 0 when it is over. */
  onAttempt?: (attempt: number) => void
}

export interface Hooks {
  signedOut(): void
  attempt(attempt: number): void
}

const project = (id: string) => `/v1/projects/${encodeURIComponent(id)}`

export class Client {
  readonly transport: Transport
  private hooks: Hooks

  constructor(transport: Transport, hooks: Hooks) {
    this.transport = transport
    this.hooks = hooks
  }

  /** What can't reach Calcifer tries again where that's safe, saying each attempt, then gives up. */
  private async send(path: string, init: RequestInit = {}, options: Options = {}): Promise<Response> {
    const safe = init.method !== "POST" || new Headers(init.headers).has("Idempotency-Key")
    const say = (attempt: number) => {
      this.hooks.attempt(attempt)
      options.onAttempt?.(attempt)
    }
    for (let attempt = 1; ; attempt++) {
      let failure: ApiError
      try {
        const response = await this.transport.fetch(path, init)
        if (response.ok || options.accept?.includes(response.status)) {
          if (attempt > 1) say(0)
          return response
        }
        failure = await refusal(response)
        if (response.status === 401) this.hooks.signedOut()
        if (response.status !== 503) {
          if (attempt > 1) say(0)
          throw failure
        }
      } catch (error) {
        if (!(error instanceof TypeError)) {
          if (attempt > 1 && aborted(error)) say(0)
          throw error
        }
        failure = unreachable()
      }
      if (!safe || attempt >= TRIES) {
        if (attempt > 1) say(0)
        throw failure
      }
      say(attempt + 1)
      await pause(500 * 2 ** (attempt - 1), init.signal)
    }
  }

  private async json<T>(path: string, init?: RequestInit, options?: Options): Promise<T> {
    return (await this.send(path, init, options)).json()
  }

  private write<T>(method: string, path: string, body: unknown, key?: string, options?: Options): Promise<T> {
    const headers: Record<string, string> = { "Content-Type": "application/json" }
    if (key) headers["Idempotency-Key"] = key
    return this.json(path, { method, headers, body: JSON.stringify(body) }, options)
  }

  // Session

  redeem(code: string): Promise<Session> {
    return this.write("POST", "/v1/session", { code })
  }

  async endSession(): Promise<void> {
    await this.send("/v1/session", { method: "DELETE" })
  }

  status(): Promise<Status> {
    return this.json("/v1/status")
  }

  /** The one stream, opened once: `EventStream` does its own reconnecting. */
  events(signal: AbortSignal): Promise<Response> {
    return this.transport.fetch("/v1/events", { signal, headers: { Accept: "text/event-stream" } })
  }

  // Files

  /** `true` when held whole, else the ranges held, `[first, last]` inclusive. */
  async held(digest: string, signal: AbortSignal, onAttempt?: (attempt: number) => void): Promise<true | [number, number][]> {
    const response = await this.send(`/v1/files/${digest}`, { method: "HEAD", signal }, { accept: [404], onAttempt })
    if (response.ok) return true
    const ranges: [number, number][] = []
    for (const part of (response.headers.get("Cozy-Held") ?? "").split(",")) {
      const [first, last] = part.split("-").map(Number)
      if (Number.isFinite(first) && Number.isFinite(last)) ranges.push([first, last])
    }
    return ranges
  }

  /** One part of a file: the file's `Held` once it is whole, else null. */
  async put(
    digest: string,
    part: Blob,
    start: number,
    total: number,
    signal: AbortSignal,
    onAttempt?: (attempt: number) => void,
  ): Promise<Held | null> {
    const headers: Record<string, string> = { "Content-Type": "application/octet-stream" }
    if (total > 0) headers["Content-Range"] = `bytes ${start}-${start + part.size - 1}/${total}`
    const response = await this.send(`/v1/files/${digest}`, { method: "PUT", headers, body: part, signal }, { onAttempt })
    return response.status === 204 ? null : response.json()
  }

  // Projects

  projects(limit = 200): Promise<ProjectList> {
    return this.json(`/v1/projects?limit=${limit}`)
  }

  newProject(name: string, key: string): Promise<Opened> {
    return this.write("POST", "/v1/projects", { name }, key)
  }

  project(id: string, signal?: AbortSignal): Promise<ProjectState> {
    return this.json(project(id), { signal })
  }

  async yaml(id: string, signal?: AbortSignal): Promise<string> {
    return (await this.send(`${project(id)}/yaml`, { signal })).text()
  }

  import(id: string, key: string, body: Import): Promise<Imported> {
    return this.write("POST", `${project(id)}/imports`, body, key)
  }

  rename(id: string, from: string, to: string): Promise<Renamed> {
    return this.write("POST", `${project(id)}/rename`, { from, to })
  }

  canvas(id: string, signal?: AbortSignal): Promise<Canvas> {
    return this.json(`${project(id)}/canvas`, { signal })
  }

  moveCards(id: string, cards: Record<string, Row | null>): Promise<{ etag: string }> {
    return this.write("PATCH", `${project(id)}/canvas`, { cards })
  }

  /** A poster, a strip of `strip` frames, a waveform, or a picture, `w` wide. */
  thumb(id: string, sha256: string, w: number, strip?: number): string {
    return `${project(id)}/thumbs/${encodeURIComponent(sha256)}?w=${w}${strip ? `&strip=${strip}` : ""}`
  }

  file(id: string, file: string): string {
    return `${project(id)}/${file.split("/").map(encodeURIComponent).join("/")}`
  }

  // Machines

  machines(): Promise<MachineList> {
    return this.json("/v1/machines")
  }
}
