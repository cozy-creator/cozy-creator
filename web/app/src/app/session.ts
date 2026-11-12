import type { BrowserSession } from "../api/calcifer"

/** Injected over the native process's private debugging pipe before application boot. */
const owned = (globalThis as typeof globalThis & { __cozyOwned?: { session: BrowserSession; birth: string } }).__cozyOwned
export const ownedBirth = owned?.birth ?? null

// Any other browser's session is this tab's: kept through a reload, gone with the tab, never in localStorage.
const KEY = "cozy.session"

function stored(): BrowserSession | null {
  try {
    localStorage.removeItem(KEY)
    const held = JSON.parse(sessionStorage.getItem(KEY) ?? "null")
    return typeof held?.token === "string" && typeof held?.media === "string" ? held : null
  } catch {
    return null
  }
}

let held: BrowserSession | null = owned ? owned.session : stored()

export function readSession(): BrowserSession | null { return held }

/** The owned browser's authority stays in memory only. */
export function saveSession(session: BrowserSession | null) {
  held = session
  if (owned) return
  try {
    if (session) sessionStorage.setItem(KEY, JSON.stringify(session))
    else sessionStorage.removeItem(KEY)
  } catch {
    // Storage refused: this load still has the session in memory.
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
