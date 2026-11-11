import type { BrowserSession } from "../api/calcifer"

const KEY = "cozy.session"

export function readSession(): BrowserSession | null {
  try {
    const held = JSON.parse(localStorage.getItem(KEY) ?? "null")
    return typeof held?.token === "string" && typeof held?.media === "string" ? held : null
  } catch {
    return null
  }
}

export function saveSession(session: BrowserSession | null) {
  try {
    if (session) localStorage.setItem(KEY, JSON.stringify(session))
    else localStorage.removeItem(KEY)
  } catch {
    // Private windows may refuse storage: this tab still has the session in memory.
  }
}

/** Consume auth fragments and old hash routes before the history router is created. */
export function takeCode(): string | null {
  if (location.hash.startsWith("#/")) {
    history.replaceState(null, "", location.hash.slice(1))
    return null
  }
  const params = new URLSearchParams(location.hash.slice(1))
  const code = params.get("code")
  if (!code) return null
  const to = params.get("to")
  let route = `${location.pathname}${location.search}`
  if (to?.startsWith("/")) {
    try {
      const target = new URL(to, location.origin)
      if (target.origin === location.origin) route = `${target.pathname}${target.search}`
    } catch {
      // Keep the current path when an old link's target cannot be read.
    }
  }
  history.replaceState(null, "", route)
  return code
}
