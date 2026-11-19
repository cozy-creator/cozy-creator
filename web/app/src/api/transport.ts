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
  /** A video's or a sound's bytes, whole, in this page's memory, for an element's `src` (cl-1215): fetched by one request that completes, so
   *  playing, looping and seeking it hold none of the six connections a browser opens to Calcifer. Shared while held; `release` when no
   *  element shows it. One past MEDIA_IN_MEMORY plays from its URL with Range instead. A `202` (not made yet) rejects with `Pending`. */
  media(path: string): Promise<{ url: string; release(): void }>
  /** `path` into `el` from memory (`media`): resolved once it's `el`'s source, to seek and play; it holds the bytes until `stop`. */
  play(path: string, el: HTMLMediaElement): Promise<Playback>
  /** An ordinary browser download of what `path` answers, into the Downloads folder. */
  save(path: string): void
}

/** The most a video or a sound is held in a page's memory: past it, it plays from Calcifer over Range, holding a connection while it loads. */
const MEDIA_IN_MEMORY = 1 << 30

/** This tab, as a project's history says who made an edit. */
export const TAB = crypto.randomUUID().slice(0, 4)

/** `media`'s refusal for a `202 thumb_pending`: Calcifer is still making what `path` names; the file's `thumb` event says when. */
export class Pending extends Error {
  constructor(readonly path: string) {
    super(`Cozy is still making ${path}`)
    this.name = "Pending"
  }
}

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
  const direct = (path: string) => {
    const url = resolve(path)
    return url.pathname + url.search
  }
  // Which play each element shows now.
  const playing = new WeakMap<HTMLMediaElement, object>()
  // Each held file once, while anything shows it: by its path, its blob's URL and how many hold it.
  const held = new Map<string, { url: Promise<string>; holders: number }>()
  const media = async (path: string): Promise<{ url: string; release(): void }> => {
    const project = guard(path)
    let one = held.get(path)
    if (!one) {
      const url = (async () => {
        const response = await fetch(direct(path), { signal: project?.signal })
        // Not made yet: no bytes to hold, and nothing kept under `path`, so the next ask reads it again.
        if (response.status === 202) { void response.body?.cancel(); throw new Pending(path) }
        if (!response.ok) { void response.body?.cancel(); return direct(path) }
        if (Number(response.headers.get("Content-Length")) > MEDIA_IN_MEMORY) { void response.body?.cancel(); return direct(path) }
        return URL.createObjectURL(await response.blob())
      })()
      held.set(path, one = { url, holders: 0 })
      url.catch(() => { if (held.get(path) === one) held.delete(path) })
    }
    const mine = one
    mine.holders++
    let released = false
    const release = () => {
      if (released) return
      released = true
      if (--mine.holders > 0) return
      if (held.get(path) === mine) held.delete(path)
      void mine.url.then((url) => { if (url.startsWith("blob:")) URL.revokeObjectURL(url) }, () => {})
    }
    try {
      const url = await mine.url
      project?.check()
      return { url, release }
    } catch (error) {
      release()
      throw error
    }
  }
  const projects = new Map<string, { epoch: number; blocked: boolean; active: AbortController }>()
  const scope = (path: string, method = "GET") => {
    const url = resolve(path), segments = url.pathname.split("/")
    // Vault control, reference lookup, a rename and a delete (cl-1097: neither reads its content) remain available while the project's
    // content is closed.
    const closed = ["vault", "address", "name"].includes(segments[4]) || (method === "DELETE" && segments.length === 4)
    if (segments[2] === "projects" && !closed) return decodeURIComponent(segments[3] ?? "") || null
    return url.searchParams.get("project")
  }
  const guard = (path: string, method?: string) => {
    const id = scope(path, method)
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
      const project = guard(path, init.method)
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
    src: async (path) => { guard(path); return direct(path) },
    media,
    save(path) {
      void (async () => {
        guard(path)
        const link = document.createElement("a")
        link.href = direct(path)
        link.download = ""
        document.body.append(link)
        link.click()
        link.remove()
      })().catch(() => {})
    },
    async play(path, el) {
      // Stopped, the element lets go of what it shows, unless another play has been put in it since: by this play, not its URL, which
      // two plays of one file share (React's development build mounts an orbit twice).
      const showing = (url: string, release?: () => void) => {
        const mine = {}
        playing.set(el, mine)
        el.src = url
        return {
          stop() {
            release?.()
            if (playing.get(el) !== mine) return
            playing.delete(el)
            el.pause()
            el.removeAttribute("src")
            el.load()
          },
        }
      }
      try {
        const one = await media(path)
        return showing(one.url, one.release)
      } catch (error) {
        if (error instanceof DOMException && error.name === "AbortError") throw error
        // What can't be read so plays from its URL, where the element says why (not found, can't decode, Calcifer gone).
        return showing(direct(path))
      }
    },
  }
}
