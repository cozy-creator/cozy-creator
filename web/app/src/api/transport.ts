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

/** An answer as Calcifer gives a refusal, made here. */
const refusal = (status: number, kind: string, code: string, message: string) =>
  new Response(JSON.stringify({ error: { kind, code, message } }), { status, headers: { "Content-Type": "application/json" } })

/** This origin's Calcifer. Only a relative API path is ever followed. */
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
  // The OS account Calcifer runs as, read as the page opens: a passphrase or recovery secret goes only to that account's Calcifer, read
  // again right before it leaves (cl-1066). Another account's process at this address may refuse it, but it would have it by then.
  // Nothing answering throws, as the request itself would.
  const account = async (signal?: AbortSignal | null) => {
    const answer = await fetch(resolve("/v1/status"), { signal, cache: "no-store" })
    const status = answer.ok ? await answer.json().catch(() => null) : null
    return typeof status?.os_user === "string" && status.os_user ? status.os_user : null
  }
  let opened: string | null = null
  account().then((own) => { opened ??= own }, () => {})
  const secret = (body: RequestInit["body"]) => typeof body === "string" && /"(passphrase|recovery_secret)"\s*:\s*"/.test(body)
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
      if (secret(init.body)) {
        const now = await account(signal)
        if (!now || now !== (opened ??= now)) {
          return refusal(403, "forbidden", "secret_other_account", "The Cozy answering this page runs as another account on this computer, so nothing secret was sent to it.")
        }
      }
      const headers = new Headers(init.headers)
      headers.set("Cozy-Client", `page ${TAB}`)
      const response = await fetch(resolve(path), { ...init, signal, headers })
      try { project?.check() } catch (error) { void response.body?.cancel(); throw error }
      return response
    },
    src: async (path) => { guard(path); return media(path) },
    save(path) {
      void (async () => {
        guard(path)
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
      // Asked for now: the caller seeks and plays it at once.
      try { load() } catch { failed() }
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
