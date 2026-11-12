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
  /** For `<img>`: a URL, or a blob URL. */
  src(path: string): Promise<string>
  /** A URL with Range, or the MSE player. */
  play(path: string, el: HTMLMediaElement): Playback
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
  const storage = async (signal?: AbortSignal | null) => {
    const token = session()?.token
    if (!token || !ownedBirth) throw new Error("An owned private browser session is required before opening project data")
    const response = await fetch(resolve("/v1/project-storage"), { signal, cache: "no-store", headers: { Authorization: `Bearer ${token}` } })
    const capability = response.ok ? await response.json() : null
    const hash = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(token))
    const principal = [...new Uint8Array(hash)].map((byte) => byte.toString(16).padStart(2, "0")).join("")
    if (session()?.token !== token || capability?.version !== 3 || capability?.daemon_birth !== ownedBirth || capability?.principal !== principal) {
      throw new Error("Calcifer cannot prove project storage protocol 3 for this private session")
    }
  }
  const needsStorage = (path: string) => ["/v1/projects", "/v1/project-", "/v1/runs", "/v1/run-", "/v1/plans", "/v1/tools", "/v1/events"].some((prefix) => path.startsWith(prefix))
  return {
    channel: false,
    async fetch(path, init = {}) {
      if (needsStorage(path)) await storage(init.signal)
      const headers = new Headers(init.headers)
      const token = session()?.token
      if (token) headers.set("Authorization", `Bearer ${token}`)
      headers.set("Cozy-Client", `page ${TAB}`)
      headers.set(PROJECT_STORAGE_HEADER, PROJECT_STORAGE_VERSION)
      return fetch(resolve(path), { ...init, headers })
    },
    src: async (path) => { await storage(); return media(path) },
    play(path, el) {
      let live = true
      void storage().then(() => { if (live) el.src = media(path) }, () => { if (live) el.dispatchEvent(new Event("error")) })
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
