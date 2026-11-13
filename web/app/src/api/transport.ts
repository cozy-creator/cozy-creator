import { PROJECT_STORAGE_HEADER, PROJECT_STORAGE_VERSION } from "./calcifer"

export interface Playback {
  stop(): void
}

/** How the page reaches a Calcifer. `sameOrigin` now; `channel` when a page runs with none here. */
export interface Transport {
  /** Reached over a link that names any address, so it may be a fake pod: no secret is entered here (web-ui.md §6). */
  readonly channel: boolean
  fetch(path: string, init?: RequestInit): Promise<Response>
  retireProject(project: string): void
  allowProject(project: string): void
  /** For `<img>`: a URL, or a blob URL. */
  src(path: string): Promise<string>
  /** A URL with Range, or the MSE player. */
  play(path: string, el: HTMLMediaElement): Playback
  /** An ordinary browser download of what `path` answers, into the Downloads folder. */
  save(path: string): void
}

/** This tab, as a project's history says who made an edit. */
export const TAB = crypto.randomUUID().slice(0, 4)

/** This origin's Calcifer. Only a relative API path is ever followed. */
/** The storage proof's refusal: Calcifer won't answer this page, said as it said it. */
class Refused extends Error {
  readonly response: Response
  constructor(response: Response) {
    super("Calcifer won't answer this page")
    this.response = response
  }
}

/** An answer as Calcifer gives a refusal, made here. */
const refusal = (status: number, kind: string, code: string, message: string) =>
  new Response(JSON.stringify({ error: { kind, code, message, next: ["reload"] } }), { status, headers: { "Content-Type": "application/json" } })

export function sameOrigin(): Transport {
  const resolve = (path: string) => {
    const url = new URL(path, location.origin)
    if (url.origin !== location.origin || !url.pathname.startsWith("/v1/")) throw new Error(`Not an API path: ${path}`)
    return url
  }
  // What can't send a header loads as any of this page's own requests: Calcifer answers its own page (cl-980).
  const media = (path: string) => {
    const url = resolve(path)
    return url.pathname + url.search
  }
  const projects = new Map<string, { epoch: number; blocked: boolean; active: AbortController }>()
  const scope = (path: string) => {
    const url = resolve(path), segments = url.pathname.split("/")
    // Vault control and reference lookup remain available while the project's content is closed.
    if (segments[2] === "projects" && !["vault", "address"].includes(segments[4])) return decodeURIComponent(segments[3] ?? "") || null
    return url.searchParams.get("project")
  }
  const guard = (path: string) => {
    const id = scope(path)
    if (!id) return null
    let state = projects.get(id)
    if (!state) projects.set(id, state = { epoch: 0, blocked: false, active: new AbortController() })
    if (state.blocked) throw new DOMException("Project view retired", "AbortError")
    const epoch = state.epoch
    return { signal: state.active.signal, check: () => { if (state!.blocked || state!.epoch !== epoch) throw new DOMException("Project view retired", "AbortError") } }
  }
  // Whether Calcifer has said it speaks project storage protocol 3 to this page's user: said once, not before each request, so what's
  // written as the page goes leaves at once (`keepalive`). An unreachable Calcifer is asked again.
  let proven = false
  const unproven = () => !proven
  // The Calcifer this page first proved: its account and its process. Another account answering is refused; another process (a
  // restart, or a program that took the port while Calcifer was down) keeps the tab working, but gets no passphrase until the page
  // is reloaded (design/calcifer-access.md).
  let pinned: { principal: string; birth: string } | null = null
  let restarted = false
  const storage = async (signal?: AbortSignal | null) => {
    if (proven) return
    const response = await fetch(resolve("/v1/project-storage"), { signal, cache: "no-store", headers: { [PROJECT_STORAGE_HEADER]: PROJECT_STORAGE_VERSION } })
    if (response.status === 403) throw new Refused(response)
    const capability = response.ok ? await response.json() : null
    if (capability?.version !== 3 || typeof capability?.daemon_birth !== "string" || !String(capability?.principal ?? "").startsWith("user:")) {
      throw new Error("Calcifer cannot prove project storage protocol 3 for this page")
    }
    pinned ??= { principal: capability.principal, birth: capability.daemon_birth }
    if (capability.principal !== pinned.principal) throw new Refused(refusal(403, "forbidden", "other_user", "Another account answers at this page's address"))
    if (capability.daemon_birth !== pinned.birth) restarted = true
    proven = true
  }
  // What carries a passphrase or a recovery secret: an encrypted project's creation, a vault unlock or recovery.
  const secret = (body: RequestInit["body"]) => typeof body === "string" && /"(passphrase|recovery_secret)"\s*:\s*"/.test(body)
  const needsStorage = (path: string) => ["/v1/projects", "/v1/project-", "/v1/runs", "/v1/run-", "/v1/plans", "/v1/tools", "/v1/events"].some((prefix) => path.startsWith(prefix))
  return {
    channel: false,
    retireProject(project) {
      let state = projects.get(project)
      if (!state) projects.set(project, state = { epoch: 0, blocked: false, active: new AbortController() })
      state.blocked = true; state.epoch++; state.active.abort()
    },
    allowProject(project) {
      const state = projects.get(project)
      if (state?.blocked) { state.blocked = false; state.active = new AbortController() }
    },
    async fetch(path, init = {}) {
      const project = guard(path)
      const signal = project ? (init.signal ? AbortSignal.any([init.signal, project.signal]) : project.signal) : init.signal
      if (needsStorage(path) && unproven()) {
        try { await storage(signal) } catch (error) { if (error instanceof Refused) return error.response; throw error }
      }
      if (restarted && secret(init.body)) {
        return refusal(409, "conflict", "calcifer_restarted", "Calcifer restarted since this page opened: reload the page before entering a passphrase")
      }
      project?.check()
      const headers = new Headers(init.headers)
      headers.set("Cozy-Client", `page ${TAB}`)
      headers.set(PROJECT_STORAGE_HEADER, PROJECT_STORAGE_VERSION)
      let response: Response
      try { response = await fetch(resolve(path), { ...init, signal, headers }) } catch (error) { proven = false; throw error }
      try { project?.check() } catch (error) { void response.body?.cancel(); throw error }
      return response
    },
    src: async (path) => { const project = guard(path); await storage(project?.signal); project?.check(); return media(path) },
    save(path) {
      void (async () => {
        const project = guard(path)
        await storage(project?.signal)
        project?.check()
        const link = document.createElement("a")
        link.href = media(path)
        link.download = ""
        document.body.append(link)
        link.click()
        link.remove()
      })().catch(() => {})
    },
    play(path, el) {
      let live = true
      const project = guard(path)
      const load = () => { project?.check(); if (live) el.src = media(path) }
      const failed = () => { if (live) el.dispatchEvent(new Event("error")) }
      // Its storage proven, the file is asked for now: the caller seeks and plays it at once, as it did before the proof.
      if (!unproven()) {
        try { load() } catch { failed() }
      } else void storage(project?.signal).then(load).catch(failed)
      return {
        stop() {
          live = false
          el.pause()
          el.removeAttribute("src")
          el.load()
        },
      }
    },
  }
}
