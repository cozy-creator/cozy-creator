import type { Session } from "../types"

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
export function sameOrigin(session: () => Session | null): Transport {
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
  return {
    channel: false,
    fetch(path, init = {}) {
      const headers = new Headers(init.headers)
      const token = session()?.token
      if (token) headers.set("Authorization", `Bearer ${token}`)
      headers.set("Cozy-Client", `page ${TAB}`)
      return fetch(resolve(path), { ...init, headers })
    },
    src: async (path) => media(path),
    play(path, el) {
      el.src = media(path)
      return {
        stop() {
          el.pause()
          el.removeAttribute("src")
          el.load()
        },
      }
    },
  }
}
