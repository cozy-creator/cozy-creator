import { ApiError, pause } from "../api/client"
import { holdLog, readEvents } from "../api/events"

export type Following =
  | { state: "waiting" }
  | { state: "reading" }
  | { state: "reconnecting"; attempt: number }
  | { state: "ended" }
  | { state: "refused"; error: unknown }

/**
 * Reads one log stream, holding the one slot for it across tabs: `open` opens it at the place it has reached. `finished` says a stream
 * that ended has nothing more; one that ended otherwise (Calcifer restarting) is opened again. Ends when aborted.
 */
export async function follow(
  signal: AbortSignal,
  open: () => Promise<Response>,
  onData: (data: string) => void,
  finished: () => boolean,
  onState: (state: Following) => void,
) {
  onState({ state: "waiting" })
  let release: () => void
  try {
    release = await holdLog(signal)
  } catch {
    return
  }
  try {
    let attempt = 0
    while (!signal.aborted) {
      try {
        const response = await open()
        onState({ state: "reading" })
        attempt = 0
        if (response.body) await readEvents(response.body, onData)
        if (finished()) return onState({ state: "ended" })
      } catch (error) {
        if (signal.aborted) return
        if (error instanceof ApiError && error.status !== 503) return onState({ state: "refused", error })
      }
      attempt += 1
      onState({ state: "reconnecting", attempt })
      await pause(Math.min(5000, 250 * 2 ** attempt), signal).catch(() => {})
    }
  } finally {
    release()
  }
}

/** What's waited for, as a line under a log. */
export function followingLine(following: Following, waitingFor: string): string | null {
  switch (following.state) {
    case "waiting":
      return `Another tab is following ${waitingFor}. This one starts when that's closed.`
    case "reconnecting":
      return `Reconnecting, attempt ${following.attempt}`
    default:
      return null
  }
}
