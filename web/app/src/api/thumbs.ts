import type { StreamEvent } from "./calcifer"

/**
 * Thumbnails Calcifer is still making (cl-982). Each is made as its file lands and kept in its project; one asked for before that answers
 * `202 thumb_pending` at once, and the stream's `thumb` says when to ask again. A file still on its way here has no thumbnail and no event,
 * so a wait also ends after `ms`.
 */
const waiting = new Map<string, Set<() => void>>()
const keyOf = (project: string, sha256: string) => `${project} ${sha256.replace(/^sha256:/, "").toLowerCase()}`

/** How many times a thumbnail is asked for before it's said to have none. */
export const THUMB_TRIES = 6

/** `then`, once, when `project`'s thumbnail of `sha256` is made (or can't be), the stream starts over, or `ms` passes. Returns how to stop. */
export function whenThumb(project: string, sha256: string, ms: number, then: () => void): () => void {
  const key = keyOf(project, sha256)
  let set = waiting.get(key)
  if (!set) waiting.set(key, (set = new Set()))
  const held = set
  let done = false
  const once = () => {
    if (done) return
    done = true
    stop()
    then()
  }
  const late = setTimeout(once, ms)
  const stop = () => {
    clearTimeout(late)
    held.delete(once)
    if (!held.size && waiting.get(key) === held) waiting.delete(key)
  }
  held.add(once)
  return () => {
    done = true
    stop()
  }
}

/** The wait before try `n` (from 1) of a thumbnail that wasn't there: the event ends it sooner. */
export const thumbWait = (n: number) => 1000 * 2 ** Math.min(n, 5)

/** What the stream says: a thumbnail made, or, starting over, that anything may have been. */
export function heardThumb(event: StreamEvent) {
  if (event.type === "thumb") release(keyOf(event.project, event.sha256))
  else if (event.type === "hello" || event.type === "resync") for (const key of [...waiting.keys()]) release(key)
}

function release(key: string) {
  const held = waiting.get(key)
  if (!held) return
  waiting.delete(key)
  for (const then of [...held]) then()
}
