import type { BrowserSession } from "../api/calcifer"

/** Injected over the native process's private debugging pipe before application boot. */
const owned = (globalThis as typeof globalThis & { __cozyOwned?: { session: BrowserSession; birth: string } }).__cozyOwned
let held: BrowserSession | null = owned?.session ?? null
export const ownedBirth = owned?.birth ?? null

export function readSession(): BrowserSession | null { return held }

/** Browser authority is memory-only and never recovered from an older normal browser profile. */
export function saveSession(session: BrowserSession | null) { held = session }

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
