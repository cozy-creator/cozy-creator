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

/** `#code=…&to=/projects/<id>`, taken out of the URL before anything else happens. */
export function takeCode(): string | null {
  const params = new URLSearchParams(location.hash.slice(1))
  const code = params.get("code")
  if (!code) return null
  const to = params.get("to")
  const route = to?.startsWith("/") ? to : "/"
  history.replaceState(null, "", `${location.pathname}${location.search}#${route}`)
  return code
}
