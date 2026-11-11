import type {
  Account,
  Agreed,
  Approved,
  Asked,
  AssetDeleted,
  AssetMade,
  AssetPatch,
  AssetPatched,
  AssetVersions,
  Browsed,
  BrowserSession,
  CalciferStatus,
  Canvas,
  CanvasPatched,
  Card,
  Code,
  Ended,
  ErrorBody,
  Families,
  Fit,
  HeldFile,
  History,
  HistoryStep,
  HistoryStepped,
  Import,
  Imported,
  Keepalive,
  Machine,
  MachineList,
  MachineModels,
  MachinePackages,
  MachineUpdate,
  ModelDetail,
  ModelEdit,
  ModelEdited,
  ModelReleased,
  ModelUnpublished,
  ModelSearch,
  NewAsset,
  Open,
  Opened,
  OrgClaim,
  PackageDetail,
  PackageSearch,
  Play,
  ProjectAddress,
  ProjectMachines,
  ProjectState,
  ProjectShowReply,
  Projects,
  Proven,
  Quote,
  Renamed,
  RentalSku,
  RentalSkus,
  Rented,
  Run,
  RunList,
  RunQuote,
  RunRequest,
  Runs,
  SignedOut,
  Submitted,
  Target,
  ToolList,
  ToolListChange,
  ToolUi,
  Tools,
  Unpinned,
  Updating,
  VersionMoved,
  VaultAsk,
  VaultStatus,
  VaultChanged,
  VaultEvent,
} from "./calcifer"
import { readEvents } from "./events"
import type { MachineLogName, Rent } from "../types"
import type { Transport } from "./transport"

export type { VaultAsk, VaultStatus, VaultChanged } from "./calcifer"
/** Only non-secret acknowledgement/progress events may leave the vault request reader. */
export type VaultProgress = Extract<VaultEvent, { type: "accepted" | "progress" }>

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

const failureStatus: Record<ErrorBody["kind"], number> = {
  usage: 400, validation: 400, credential: 401, forbidden: 403, not_found: 404,
  conflict: 409, capacity: 507, unavailable: 503, internal: 500,
}

const unreachable = () =>
  new ApiError(0, { kind: "unavailable", code: "calcifer_unreachable", message: "Calcifer isn't answering", next: ["cozy open"] })

/** The page's own session is gone. Any other 401 is a Hub's word on a code or a key, which belongs where it happened. */
export const sessionLost = (error: ApiError) => error.status === 401 && error.body.code === "unauthenticated"

async function refusal(response: Response): Promise<ApiError> {
  const body = await response.json().catch(() => null)
  const error = body?.error
  if (typeof error?.message === "string") return new ApiError(response.status, error)
  return new ApiError(response.status, { kind: "internal", code: `http_${response.status}`, message: `Calcifer answered ${response.status}` })
}

const aborted = (error: unknown) => error instanceof DOMException && error.name === "AbortError"

export function pause(ms: number, signal?: AbortSignal | null): Promise<void> {
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
  /** One try, which this ends: its retries are the outbox's. */
  once?: AbortSignal
  /** What a person did that this write is of (`Cozy-Edit`): the project's history keeps the writes of one as one entry. */
  edit?: string
  /** The outbox action this try is of: every try of one ask goes under one `Idempotency-Key`, so Calcifer does it once. */
  action?: string
}

/** One try of a write, the edit it's of, and its action. */
export type Write = Pick<Options, "once" | "edit" | "action">

/** The `Idempotency-Key` of a write: the one it names, else one per action and ask, so each try of the same ask is the same key and a try
 * that asks something else (a batch grown since) is another. FNV-1a: tries of one action are all it tells apart. */
function keyOf(method: string, path: string, body: string, key?: string, action?: string): string | undefined {
  if (key || !action) return key
  let hash = 0x811c9dc5
  for (const char of `${method} ${path}\n${body}`) hash = Math.imul(hash ^ char.charCodeAt(0), 0x01000193) >>> 0
  return `${action}-${hash.toString(16)}`
}

export interface Hooks {
  signedOut(): void
  attempt(attempt: number): void
}

const project = (id: string) => `/v1/projects/${encodeURIComponent(id)}`
const vaultOperation = (id: string, operation: string) => `/v1/projects/${encodeURIComponent(id)}/vault/operations/${operation}`
const run = (id: string | number) => `/v1/runs/${encodeURIComponent(id)}`
const machine = (name: string) => `/v1/machines/${encodeURIComponent(name)}`
// `org/name` on the wire; a first-party name has no owner, and its org is `_`.
const orgOf = (name: string) => encodeURIComponent(name.includes("/") ? name.slice(0, name.indexOf("/")) : "_")
const baseOf = (name: string) => encodeURIComponent(name.slice(name.indexOf("/") + 1))
const pkg = (name: string) => `/v1/packages/${orgOf(name)}/${baseOf(name)}`
const model = (name: string) => `/v1/models/${orgOf(name)}/${baseOf(name)}`

export interface RunFilter {
  project?: string
  status?: string
  machine?: string
}

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
    if (options.edit) init = { ...init, headers: { ...Object.fromEntries(new Headers(init.headers)), "Cozy-Edit": options.edit } }
    for (let attempt = 1; ; attempt++) {
      let failure: ApiError
      try {
        const response = await this.transport.fetch(path, options.once ? { ...init, signal: options.once } : init)
        if (response.ok || options.accept?.includes(response.status)) {
          if (attempt > 1) say(0)
          return response
        }
        failure = await refusal(response)
        if (sessionLost(failure)) this.hooks.signedOut()
        if (response.status !== 503 || options.once) {
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
      if (!safe || options.once || attempt >= TRIES) {
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

  /** A stream, opened once: its reader does its own reconnecting. */
  private async opened(path: string, signal: AbortSignal): Promise<Response> {
    const response = await this.transport.fetch(path, { signal, headers: { Accept: "text/event-stream" } })
    if (response.ok) return response
    const failure = await refusal(response)
    if (sessionLost(failure)) this.hooks.signedOut()
    throw failure
  }

  private write<T>(method: string, path: string, body: unknown, key?: string, options?: Options): Promise<T> {
    const headers: Record<string, string> = { "Content-Type": "application/json" }
    const text = JSON.stringify(body)
    const asked = keyOf(method, path, text, key, options?.action)
    if (asked) headers["Idempotency-Key"] = asked
    return this.json(path, { method, headers, body: text }, options)
  }

  // Session

  redeem(code: string): Promise<BrowserSession> {
    return this.write("POST", "/v1/session", { code })
  }

  async endSession(): Promise<void> {
    await this.send("/v1/session", { method: "DELETE" })
  }

  status(): Promise<CalciferStatus> {
    return this.json("/v1/status")
  }

  /** The one stream, opened once: `EventStream` does its own reconnecting. */
  events(signal: AbortSignal): Promise<Response> {
    return this.transport.fetch("/v1/events", { signal, headers: { Accept: "text/event-stream" } })
  }

  /** The projects the tabs on `stream` show, which Calcifer holds open while it lasts. One that ended is told again on the next. */
  async show(stream: string, projects: string[], signal: AbortSignal): Promise<ProjectShowReply | null> {
    const body = JSON.stringify({ projects })
    const response = await this.send(`/v1/events/${encodeURIComponent(stream)}`, { method: "PUT", headers: { "Content-Type": "application/json" }, body }, { once: signal })
    // Older services confirm completion without saying which Readers they could install.
    return response.status === 204 ? null : response.json()
  }

  // Files

  /** `true` when held whole, else the ranges held, `[first, last]` inclusive. */
  async held(digest: string, signal: AbortSignal, onAttempt?: (attempt: number) => void, projectId?: string): Promise<true | [number, number][]> {
    const response = projectId === undefined
      ? await this.send(`/v1/files/${digest}`, { method: "HEAD", signal }, { accept: [404], onAttempt })
      : await this.send(`/v1/project-files/${digest}?project=${encodeURIComponent(projectId)}`, { method: "HEAD", signal }, { accept: [404], onAttempt })
    if (response.ok) return true
    // A pod's `Cozy-*` headers are hostile too: read as numbers, and a nonsense range is no range.
    const ranges: [number, number][] = []
    for (const part of (response.headers.get("Cozy-Held") ?? "").split(",")) {
      const [first, last] = part.split("-").map(Number)
      if (Number.isSafeInteger(first) && Number.isSafeInteger(last) && first >= 0 && last >= first) ranges.push([first, last])
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
    projectId?: string,
  ): Promise<HeldFile | null> {
    const headers: Record<string, string> = { "Content-Type": "application/octet-stream" }
    if (total > 0) headers["Content-Range"] = `bytes ${start}-${start + part.size - 1}/${total}`
    const response = projectId === undefined
      ? await this.send(`/v1/files/${digest}`, { method: "PUT", headers, body: part, signal }, { onAttempt })
      : await this.send(`/v1/project-files/${digest}?project=${encodeURIComponent(projectId)}`, { method: "PUT", headers, body: part, signal }, { onAttempt })
    return response.status === 204 ? null : response.json()
  }

  // Projects

  projects(limit = 200): Promise<Projects> {
    return this.json(`/v1/projects?limit=${limit}`)
  }

  async newProject(body: Open): Promise<Opened> {
    if (body.unencrypted) return this.write("POST", "/v1/projects", body, `project-${crypto.randomUUID()}`)
    if (this.transport.channel) return Promise.reject(new ApiError(403, { kind: "forbidden", code: "vault_local_only", message: "Open Cozy on this computer to enter encryption credentials" }))
    const status = await this.json<CalciferStatus>("/v1/status", { cache: "no-store" })
    if (status.encrypted_project_creation !== true) throw new ApiError(503, { kind: "unavailable", code: "calcifer_outdated", message: "Calcifer cannot create encrypted projects directly. Update or restart it; no credential or creation request was sent." })
    return this.json("/v1/projects", { method: "POST", headers: { "Content-Type": "application/json", "Cache-Control": "no-store" }, body: JSON.stringify(body), cache: "no-store" })
  }

  vault(id: string, signal?: AbortSignal): Promise<VaultStatus> {
    return this.json(`${project(id)}/vault`, { signal, cache: "no-store" })
  }

  /** One operation ID survives connection loss; credentials and final secrets stay in this caller's memory. */
  async vaultChange(id: string, body: Partial<VaultAsk> & Pick<VaultAsk, "action">, onProgress?: (event: VaultProgress) => void, signal?: AbortSignal): Promise<VaultChanged> {
    if (this.transport.channel) throw new ApiError(403, { kind: "forbidden", code: "vault_local_only", message: "Open this project on its computer to enter encryption credentials" })
    signal?.throwIfAborted()
    const operation = crypto.randomUUID()
    const serialized = JSON.stringify(body)
    const incomplete = () => new ApiError(0, {
      kind: "unavailable", code: "vault_stream_incomplete",
      message: "The connection ended before the project operation finished. It may still be running; check project protection before trying again.",
    })
    const invalid = () => new ApiError(502, { kind: "internal", code: "vault_stream_invalid", message: "The project operation response could not be read. Check project protection before trying again." })
    const terminal: { changed?: VaultChanged; failure?: ApiError } = {}
    let legacy = false
    let attempt = 0
    let acknowledged = false
    let progress = ""
    try {
      for (;;) {
        signal?.throwIfAborted()
        const first = attempt++ === 0
        const stop = new AbortController()
        const cancel = () => stop.abort(signal?.reason)
        signal?.addEventListener("abort", cancel, { once: true })
        let heard = performance.now()
        let silent = false
        // Same 45-second silence rule as the shared event stream. Comments are health, not progress.
        const health = legacy ? undefined : setInterval(() => {
          if (performance.now() - heard >= 45_000) {
            silent = true
            stop.abort()
          }
        }, 1000)
        try {
          const init: RequestInit = {
            headers: { "Content-Type": "application/json", Accept: "text/event-stream", "Cache-Control": "no-store" },
            body: serialized, cache: "no-store", signal: stop.signal,
          }
          const response = legacy
            ? await this.send(`${project(id)}/vault`, { ...init, method: "POST" }, { once: stop.signal })
            : await this.send(vaultOperation(id, operation), { ...init, method: "PUT" }, { once: stop.signal })
          this.hooks.attempt(0)
          heard = performance.now()
          if (!response.headers.get("Content-Type")?.toLowerCase().startsWith("text/event-stream")) {
            if (!legacy) throw invalid()
            let changed: VaultChanged
            try { changed = await response.json() } catch { throw invalid() }
            if (!changed?.vault) throw invalid()
            terminal.changed = changed
          } else {
            if (!response.body) throw incomplete()
            try {
              await readEvents(response.body, (data) => {
                if (terminal.changed || terminal.failure) return
                let event: VaultEvent
                try { event = JSON.parse(data) as VaultEvent } catch { throw invalid() }
                switch (event?.type) {
                  case "accepted":
                    if (typeof event.action !== "string") throw invalid()
                    if (!acknowledged) onProgress?.({ type: "accepted", action: event.action })
                    acknowledged = true
                    break
                  case "progress": {
                    if (typeof event.phase !== "string" || !Number.isFinite(event.files_done) || event.files_done < 0 || !Number.isFinite(event.bytes_done) || event.bytes_done < 0) throw invalid()
                    const update: VaultProgress = { type: "progress", phase: event.phase, files_done: event.files_done, bytes_done: event.bytes_done }
                    const current = JSON.stringify(update)
                    if (current !== progress) onProgress?.(update)
                    progress = current
                    acknowledged = true
                    break
                  }
                  case "completed":
                    if (!event.changed?.vault) throw invalid()
                    terminal.changed = event.changed
                    stop.abort()
                    break
                  case "failed":
                    if (typeof event.error?.message !== "string" || typeof event.error.code !== "string" || typeof event.error.kind !== "string") throw invalid()
                    terminal.failure = new ApiError(failureStatus[event.error.kind] ?? 500, event.error)
                    stop.abort()
                    break
                  default: throw invalid()
                }
              }, () => { heard = performance.now() })
            } catch (error) {
              if (!terminal.changed && !terminal.failure) throw error
            }
          }
          if (terminal.failure) throw terminal.failure
          if (!terminal.changed) throw incomplete()
          return terminal.changed
        } catch (error) {
          signal?.throwIfAborted()
          if (terminal.failure || terminal.changed) throw error
          // Only an initial, unambiguous unknown route may use old POST semantics.
          // A later 404 could follow admission on the lost connection: never replay as a new operation.
          if (first && !acknowledged && error instanceof ApiError && error.status === 404 && ["unknown_route", "http_404"].includes(error.body.code)) {
            legacy = true
            continue
          }
          const lost = silent || error instanceof TypeError || (error instanceof ApiError && error.status === 0)
          if (!lost) throw error
          if (legacy) throw incomplete()
        } finally {
          if (health !== undefined) clearInterval(health)
          signal?.removeEventListener("abort", cancel)
          stop.abort()
        }
        this.hooks.attempt(attempt + 1)
        await pause(Math.min(5000, 500 * 2 ** Math.min(attempt - 1, 4)), signal)
      }
    } finally {
      this.hooks.attempt(0)
      if (!legacy && (terminal.changed || terminal.failure)) {
        // No secret in this cleanup request; losing it leaves expiry to the server.
        void this.transport.fetch(vaultOperation(id, operation), { method: "DELETE", cache: "no-store", headers: { "Cache-Control": "no-store" } })
          .then((response) => response.body?.cancel()).catch(() => {})
      }
    }
  }

  project(id: string, signal?: AbortSignal): Promise<ProjectState> {
    return this.json(project(id), { signal })
  }

  /** One asset as it is now: what a change of it costs, whatever else the project holds. */
  asset(id: string, name: string): Promise<AssetVersions> {
    return this.json(`${project(id)}/assets/${encodeURIComponent(name)}`)
  }

  projectAddress(reference: string): Promise<ProjectAddress> {
    return this.json(`${project(reference)}/address`)
  }

  /** What's said of the project besides its assets. */
  projectHead(id: string): Promise<ProjectState> {
    return this.json(`${project(id)}?assets=false`)
  }

  async yaml(id: string, signal?: AbortSignal): Promise<string> {
    return (await this.send(`${project(id)}/yaml`, { signal })).text()
  }

  import(id: string, key: string, body: Import, write: Write = {}): Promise<Imported> {
    return this.write("POST", `${project(id)}/imports`, body, key, write)
  }

  /** Pins `name` to `version`: a short run of digits is its version, anything else a hash prefix of a file of the project. */
  approve(id: string, name: string, version: string, write: Write = {}): Promise<Approved> {
    return this.write("POST", `${project(id)}/approve`, { name, version }, undefined, write)
  }

  /** Its pin's line gone, so its state follows its inputs again. */
  unpin(id: string, name: string, write: Write = {}): Promise<Unpinned> {
    return this.write("POST", `${project(id)}/unpin`, { name }, undefined, write)
  }

  /** A new version of each of `names`, on these machines. A `pickup` goes with this one render of one box; `outdated`, with `all`, re-runs pins that are. */
  render(id: string, names: string[], on: string[], key: string, also: Pick<Runs, "pickup" | "outdated"> = {}, once?: AbortSignal): Promise<Asked> {
    return this.write("POST", `${project(id)}/runs`, { names, on, ...also }, key, { once })
  }

  /** The machines the project's runs and renders go to, each to whichever finishes first; none: any. */
  setMachines(id: string, machines: string[]): Promise<ProjectMachines> {
    return this.write("PUT", `${project(id)}/machines`, { machines })
  }

  /** What a render of `names` on this computer alone would do poorly, a line a tool. */
  fit(id: string, names: string[]): Promise<Fit> {
    return this.write("POST", `${project(id)}/fit`, { names })
  }

  /** The same, each tool kept as rendered here, so nothing asks again. */
  renderHere(id: string, names: string[]): Promise<Fit> {
    return this.write("POST", `${project(id)}/fit/here`, { names })
  }

  /** A tool dropped on the canvas. */
  newAsset(id: string, key: string, body: NewAsset, write: Write = {}): Promise<AssetMade> {
    return this.write("POST", `${project(id)}/assets`, body, key, write)
  }

  /** These inputs set. With `read`, only if its own lines are still what that read. */
  patchAsset(id: string, name: string, body: AssetPatch, read?: string, write: Write = {}): Promise<AssetPatched> {
    const headers: Record<string, string> = { "Content-Type": "application/json" }
    if (read) headers["If-Match"] = read
    const path = `${project(id)}/assets/${encodeURIComponent(name)}`
    const text = JSON.stringify(body)
    const asked = keyOf("PATCH", path, `${read ?? ""}\n${text}`, undefined, write.action)
    if (asked) headers["Idempotency-Key"] = asked
    return this.json(path, { method: "PATCH", headers, body: text }, write)
  }

  deleteAsset(id: string, name: string, write: Write = {}): Promise<AssetDeleted> {
    return this.write("DELETE", `${project(id)}/assets/${encodeURIComponent(name)}`, {}, undefined, write)
  }

  /** A file joins a box as its next version. */
  addVersion(id: string, name: string, source: string, write: Write = {}): Promise<VersionMoved> {
    return this.write("POST", `${project(id)}/assets/${encodeURIComponent(name)}/versions`, { source }, undefined, write)
  }

  /** One take or version off its box. Its number is kept, and its file until `reclaim`. */
  discardVersion(id: string, name: string, number: number, write: Write = {}): Promise<VersionMoved> {
    return this.write("DELETE", `${project(id)}/assets/${encodeURIComponent(name)}/versions/${number}`, {}, undefined, write)
  }

  /** The project's history, newest first. */
  history(id: string, limit: number): Promise<History> {
    return this.json(`${project(id)}/history?limit=${limit}`)
  }

  /** An entry taken back: by its id, or the edit this page called it. */
  undo(id: string, target: HistoryStep, write: Write = {}): Promise<HistoryStepped> {
    return this.write("POST", `${project(id)}/history/undo`, target, undefined, write)
  }

  /** What undo took back, done again. */
  redo(id: string, target: HistoryStep, write: Write = {}): Promise<HistoryStepped> {
    return this.write("POST", `${project(id)}/history/redo`, target, undefined, write)
  }

  /** What can be dropped on a canvas: the Edit, the local code `project` names, what `on` has installed, then the Hub's. */
  tools(q: string, on: string | null, project: string | null, signal?: AbortSignal): Promise<Tools> {
    const asked = new URLSearchParams()
    if (q) asked.set("q", q)
    if (on) asked.set("on", on)
    if (project) asked.set("project", project)
    return this.json(`/v1/tools?${asked}`, { signal })
  }

  /** A package on your list, or tools off it. Nothing is installed or deleted. */
  changeToolList(change: ToolListChange, once?: AbortSignal): Promise<ToolList> {
    return this.write("POST", "/v1/tools/list", change, undefined, { once })
  }

  /** The Hub's packages that make media. */
  browseTools(q: string, limit: number, signal?: AbortSignal): Promise<Browsed> {
    const asked = new URLSearchParams({ limit: String(limit) })
    if (q) asked.set("q", q)
    return this.json(`/v1/tools/browse?${asked}`, { signal })
  }

  /** Where to import a tool's component from. `project`: local code it names can answer. */
  toolUi(id: string, project: string | null): Promise<ToolUi> {
    const asked = project ? `?${new URLSearchParams({ project })}` : ""
    return this.json(`/v1/tools/${encodeURIComponent(id)}/ui${asked}`)
  }

  /** These assets' queued work dropped and their runs canceled. */
  cancelRender(id: string, names: string[], once?: AbortSignal): Promise<unknown> {
    return this.write("POST", `${project(id)}/cancel`, { names }, undefined, { once })
  }

  rename(id: string, from: string, to: string, write: Write = {}): Promise<Renamed> {
    return this.write("POST", `${project(id)}/rename`, { from, to }, undefined, write)
  }

  canvas(id: string, signal?: AbortSignal): Promise<Canvas> {
    return this.json(`${project(id)}/canvas`, { signal })
  }

  /** Rows set; one in `placed`, this page's own working, only if no other client wrote it after the revision it was worked out from, else answered in `kept` as it is. */
  moveCards(id: string, cards: Record<string, Card | null>, placed: Record<string, number>, write: Write = {}): Promise<CanvasPatched> {
    return this.write("PATCH", `${project(id)}/canvas`, { cards, ...(Object.keys(placed).length ? { placed } : {}) }, undefined, write)
  }

  /** A poster, a strip of `strip` frames, a waveform, or a picture, `w` wide. */
  thumb(id: string, sha256: string, w: number, strip?: number): string {
    return `${project(id)}/thumbs/${encodeURIComponent(sha256)}?w=${w}${strip ? `&strip=${strip}` : ""}`
  }

  /** A video's preview: a few seconds of it, small and silent. */
  preview(id: string, sha256: string): string {
    return `${project(id)}/thumbs/${encodeURIComponent(sha256)}?preview=true`
  }

  file(id: string, file: string): string {
    return `${project(id)}/${file.split("/").map(encodeURIComponent).join("/")}`
  }

  // Runs

  runs(filter: RunFilter, limit: number, signal?: AbortSignal): Promise<RunList> {
    const query = new URLSearchParams({ limit: String(limit) })
    for (const [name, value] of Object.entries(filter)) if (value) query.set(name, value)
    return this.json(`/v1/runs?${query}`, { signal })
  }

  run(id: string | number, signal?: AbortSignal): Promise<Run> {
    return this.json(run(id), { signal })
  }

  /** With its measurements, calls, report and what it was asked. */
  runFull(id: string | number, signal?: AbortSignal): Promise<Run> {
    return this.json(`${run(id)}?detail=full`, { signal })
  }

  cancelRun(id: string, once?: AbortSignal): Promise<Run> {
    return this.write("POST", `${run(id)}/cancel`, {}, undefined, { once })
  }

  /** A run's log and events after a place in them, opened once: its reader resumes it. */
  runEvents(id: string, after: number, signal: AbortSignal): Promise<Response> {
    return this.opened(`${run(id)}/events${after ? `?starting_after=${after}` : ""}`, signal)
  }

  /** Where an output's bytes are served, once they landed here. */
  output(id: string, name: string, index: number): string {
    return `${run(id)}/outputs/${encodeURIComponent(name)}${index ? `?index=${index}` : ""}`
  }

  /** How to play an output straight from its machine. */
  play(id: string, name: string, index: number): Promise<Play> {
    const output = encodeURIComponent(index ? `${name}/${index}` : name)
    return this.json(`${run(id)}/play?output=${output}`)
  }

  /** What a run of `target` would call: a package's functions, and a named function's request. Runs nothing. */
  target(target: string, signal?: AbortSignal): Promise<Target> {
    return this.json(`/v1/targets?target=${encodeURIComponent(target)}`, { signal })
  }

  /** `null`: it rents nothing, as what Calcifer does itself. */
  runQuote(request: RunRequest): Promise<RunQuote | null> {
    return this.write("POST", "/v1/run-quotes", request)
  }

  /** One of Calcifer's own jobs (`cozy/…`) as the run request it is: `asked` is its fields, but for where it goes. */
  private jobRequest(target: string, { on, agreed, gpus, ...input }: Record<string, unknown>): RunRequest {
    return { target, input, ...(on ? { on } : {}), ...(agreed ? { agreed } : {}), ...(gpus ? { gpus } : {}) } as RunRequest
  }

  jobQuote(target: string, asked: object): Promise<RunQuote | null> {
    return this.runQuote(this.jobRequest(target, asked as Record<string, unknown>))
  }

  job(target: string, asked: object, key: string): Promise<Submitted> {
    return this.submit(this.jobRequest(target, asked as Record<string, unknown>), key)
  }

  submit(request: RunRequest, key: string): Promise<Submitted> {
    return this.write("POST", "/v1/runs", request, key)
  }

  // Machines

  /** `refresh` lists every Hub again first, which takes as long as they do. */
  machines(refresh = false): Promise<MachineList> {
    return this.json(`/v1/machines${refresh ? "?refresh=true" : ""}`)
  }

  machine(name: string, signal?: AbortSignal): Promise<Machine> {
    return this.json(machine(name), { signal })
  }

  addMachine(name: string, address: string, fingerprint: string): Promise<Machine> {
    return this.write("POST", "/v1/machines", { kind: "add", name, address, fingerprint })
  }

  endMachine(name: string, force: boolean): Promise<Ended> {
    return this.json(`${machine(name)}${force ? "?force=true" : ""}`, { method: "DELETE" })
  }

  keepAlive(name: string): Promise<Keepalive> {
    return this.write("POST", `${machine(name)}/keepalive`, {})
  }

  updateMachine(name: string, update: MachineUpdate, key: string): Promise<Updating> {
    return this.write("POST", `${machine(name)}/update`, update, key)
  }

  machineLogEvents(name: string, log: MachineLogName, lines: number, after: string | null, signal: AbortSignal): Promise<Response> {
    const place = after ? `&after=${encodeURIComponent(after)}` : ""
    return this.opened(`${machine(name)}/logs/${log}?lines=${lines}${place}`, signal)
  }

  machinePackages(name: string, signal?: AbortSignal): Promise<MachinePackages> {
    return this.json(`${machine(name)}/packages`, { signal })
  }

  machineModels(name: string, signal?: AbortSignal): Promise<MachineModels> {
    return this.json(`${machine(name)}/models`, { signal })
  }

  // Packages

  packages(q: string, limit: number, signal?: AbortSignal): Promise<PackageSearch> {
    return this.json(`/v1/packages?limit=${limit}${q ? `&q=${encodeURIComponent(q)}` : ""}`, { signal })
  }

  package(name: string, signal?: AbortSignal): Promise<PackageDetail> {
    return this.json(pkg(name), { signal })
  }

  yank(name: string, release: string): Promise<unknown> {
    return this.write("POST", `${pkg(name)}/releases/${encodeURIComponent(release)}/yank`, {})
  }

  unyank(name: string, release: string): Promise<unknown> {
    return this.json(`${pkg(name)}/releases/${encodeURIComponent(release)}/yank`, { method: "DELETE" })
  }

  deletePackage(name: string): Promise<unknown> {
    return this.json(pkg(name), { method: "DELETE" })
  }

  // Models

  /** The Hub's models: of `family` only when it's said, and with no `noTag` (`nsfw` leaves out models that make sexual or nude images). */
  models(q: string, limit: number, signal?: AbortSignal, only: { family?: string; noTag?: string } = {}): Promise<ModelSearch> {
    const asked = new URLSearchParams({ limit: String(limit) })
    if (q) asked.set("q", q)
    if (only.family) asked.set("family", only.family)
    if (only.noTag) asked.set("no_tag", only.noTag)
    return this.json(`/v1/models?${asked}`, { signal })
  }

  model(name: string, signal?: AbortSignal): Promise<ModelDetail> {
    return this.json(model(name), { signal })
  }

  /** The families a model's weights can verify as, for its owner to pick from. */
  families(signal?: AbortSignal): Promise<Families> {
    return this.json("/v1/families", { signal })
  }

  editModel(name: string, edit: ModelEdit): Promise<ModelEdited> {
    return this.write("PATCH", model(name), edit)
  }

  /** The Hub never reuses a version, so asking again is safe and takes no key. */
  modelPublish(name: string, version: string, checkpoints: string[]): Promise<ModelReleased> {
    return this.write("POST", `${model(name)}/releases`, { version, checkpoints })
  }

  modelUnpublish(name: string, version: string): Promise<ModelUnpublished> {
    return this.write("POST", `${model(name)}/releases/${encodeURIComponent(version)}/unpublish`, {})
  }

  deleteModel(name: string): Promise<unknown> {
    return this.json(model(name), { method: "DELETE" })
  }

  deleteModelRelease(name: string, version: string): Promise<unknown> {
    return this.json(`${model(name)}/releases/${encodeURIComponent(version)}`, { method: "DELETE" })
  }

  deleteModelCheckpoint(name: string, checkpoint: string): Promise<unknown> {
    return this.json(`${model(name)}/checkpoints/${encodeURIComponent(checkpoint)}`, { method: "DELETE" })
  }

  // Account

  account(signal?: AbortSignal): Promise<Account> {
    return this.json("/v1/account", { signal })
  }

  sendCode(purpose: "sign_in" | "sign_out_everywhere", email?: string): Promise<Code> {
    return this.write("POST", "/v1/account/codes", { purpose, ...(email ? { email } : {}) })
  }

  proveCode(id: string, code: string): Promise<Proven> {
    return this.write("POST", `/v1/account/codes/${encodeURIComponent(id)}`, { code })
  }

  /** The step the Hub's AuthKit named, refusing `id`'s code: `path` under its mount, with `body`. */
  stepCode(id: string, path: string, body: unknown): Promise<Proven> {
    return this.write("POST", `/v1/account/codes/${encodeURIComponent(id)}/step`, { path, body })
  }

  nameAccount(name: string): Promise<Account> {
    return this.write("PUT", "/v1/account/name", { name })
  }

  signOut(): Promise<SignedOut> {
    return this.json("/v1/account", { method: "DELETE" })
  }

  claimOrg(org: string): Promise<OrgClaim> {
    return this.write("POST", `/v1/orgs/${encodeURIComponent(org)}/claim`, {})
  }

  // Rentals: nothing here rents but `rent`, under a key that names one order forever.

  async rentalSkus(signal?: AbortSignal): Promise<RentalSku[]> {
    return (await this.json<RentalSkus>("/v1/rental-skus", { signal })).rental_skus
  }

  rentalQuote(order: Rent): Promise<Quote> {
    return this.write("POST", "/v1/rental-quotes", { kind: "rent", ...order })
  }

  rent(order: Rent, agreed: Agreed, key: string): Promise<Rented> {
    return this.write("POST", "/v1/machines", { kind: "rent", ...order, agreed }, key)
  }
}
