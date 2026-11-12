import { ownedBirth } from "../app/session"
import type { BrowserSession } from "./calcifer"
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
export function sameOrigin(session: () => BrowserSession | null): Transport {
  const resolve = (path: string) => {
    const url = new URL(path, location.origin)
    if (url.origin !== location.origin || !url.pathname.startsWith("/v1/")) throw new Error(`Not an API path: ${path}`)
    return url
  }
  const media = (path: string) => {
    const url = resolve(path)
    const held = session()?.media
    if (held) url.searchParams.set("media", held)
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
  // The token whose storage proof stands: proven once, not before each request, so what's written as the page goes leaves at once
  // (`keepalive`). A refused session or an unreachable Calcifer proves it again.
  let proven: string | null = null
  const unproven = () => proven !== session()?.token
  const storage = async (signal?: AbortSignal | null) => {
    const token = session()?.token
    if (!token) throw new Error("A browser session is required before opening project data")
    if (proven === token) return
    const response = await fetch(resolve("/v1/project-storage"), { signal, cache: "no-store", headers: { Authorization: `Bearer ${token}`, [PROJECT_STORAGE_HEADER]: PROJECT_STORAGE_VERSION } })
    const capability = response.ok ? await response.json() : null
    const hash = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(token))
    const principal = [...new Uint8Array(hash)].map((byte) => byte.toString(16).padStart(2, "0")).join("")
    // The owned browser's authority is one Calcifer's; a signed-in tab's outlives a restart, so it takes the birth Calcifer proves for it.
    const birth = ownedBirth ?? capability?.daemon_birth
    if (session()?.token !== token || capability?.version !== 3 || typeof birth !== "string" || !birth || capability?.daemon_birth !== birth || capability?.principal !== principal) {
      throw new Error("Calcifer cannot prove project storage protocol 3 for this private session")
    }
    proven = token
  }
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
      if (needsStorage(path) && unproven()) await storage(signal)
      project?.check()
      const headers = new Headers(init.headers)
      const token = session()?.token
      if (token) headers.set("Authorization", `Bearer ${token}`)
      headers.set("Cozy-Client", `page ${TAB}`)
      headers.set(PROJECT_STORAGE_HEADER, PROJECT_STORAGE_VERSION)
      let response: Response
      try { response = await fetch(resolve(path), { ...init, signal, headers }) } catch (error) { proven = null; throw error }
      if (response.status === 401) proven = null
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
