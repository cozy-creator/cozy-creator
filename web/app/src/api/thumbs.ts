import type { StreamEvent } from "./calcifer"
import { Pending, type Transport } from "./transport"

/**
 * Thumbnails Calcifer is still making (cl-982). Each is made as its file lands and kept in its project; one asked for before that answers
 * `202 thumb_pending` at once, and the stream's `thumb` says when to ask again. A file still on its way here has no thumbnail and no event,
 * so a wait also ends after `ms`.
 */
const waiting = new Map<string, Set<() => void>>()
const keyOf = (project: string, sha256: string) => `${project} ${sha256.replace(/^sha256:/, "").toLowerCase()}`

/** Consecutive real failures before a thumbnail is unavailable; accepted pending work does not spend this budget. */
export const THUMB_TRIES = 6

/** An img/decode failure has no HTTP status. Probe only that failed thumbnail, without downloading its bytes again. */
export async function thumbPending(transport: Pick<Transport, "fetch">, path: string, signal: AbortSignal): Promise<boolean> {
  const response = await transport.fetch(path, { method: "HEAD", signal })
  const pending = response.status === 202
  await response.body?.cancel()
  return pending
}

/** `then`, once, when `project`'s thumbnail of `sha256` is made (or can't be), the stream starts over, or `ms` passes, if given. Returns how
 *  to stop. */
export function whenThumb(project: string, sha256: string, ms: number | undefined, then: () => void): () => void {
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
  const late = ms === undefined ? undefined : setTimeout(once, ms)
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

/** Video previews that failed `THUMB_TRIES` times: none is asked for again until Calcifer next draws that file (`thumb`), its project is
 *  locked or unlocked, or the stream starts over. */
const unplayable = new Set<string>()

export interface HeldPreview {
  /** What it was handed couldn't play: Calcifer couldn't make it or answer, its file isn't here yet, or the browser can't decode it. It's
   *  asked for again as a thumbnail is (`THUMB_TRIES`, each after `thumbWait` or the file's `thumb` event), then once that event comes. An
   *  ask nothing answered (Calcifer down) is asked again so too. */
  failed(): void
  /** `then` isn't called after. */
  stop(): void
}

/**
 * `project`'s preview of `sha256`, Calcifer's `path`, in this page's memory (`Transport.media`) and handed to `then` once it's there. Asked for
 * before Calcifer made it (`202 thumb_pending`, cl-982), it waits for that file's `thumb` event, or `thumbWait`, and is asked again, spending no
 * tries. One that failed, or that nothing answered, is asked again as a thumbnail is (`failed`). One cut short by its project's lock waits for
 * that project's next `project.vault`, its unlock.
 */
export function heldPreview(transport: Pick<Transport, "media">, project: string, sha256: string, path: string,
  then: (held: { url: string; release(): void }) => void): HeldPreview {
  const key = keyOf(project, sha256)
  let stopped = false, waits = 0, errors = 0
  let waiting: (() => void) | undefined
  // Asked again once every wait that event ended has run, so a failure it clears is gone by then.
  const later = (ms: number | undefined) => {
    waiting?.()
    waiting = whenThumb(project, sha256, ms, () => queueMicrotask(ask))
  }
  const failed = () => {
    if (stopped) return
    if (++errors < THUMB_TRIES) return later(thumbWait(errors))
    if (!unplayable.has(key)) {
      unplayable.add(key)
      whenThumb(project, sha256, undefined, () => unplayable.delete(key))
    }
    later(undefined)
  }
  const ask = () => {
    if (stopped) return
    if (unplayable.has(key)) return later(undefined)
    transport.media(path).then((held) => {
      if (stopped) return held.release()
      waits = 0
      then(held)
    }, (error) => {
      if (stopped) return
      if (error instanceof DOMException && error.name === "AbortError") return later(undefined)
      if (!(error instanceof Pending)) return failed()
      waits = Math.min(5, waits + 1)
      later(thumbWait(waits))
    })
  }
  ask()
  return {
    failed,
    stop() {
      stopped = true
      waiting?.()
    },
  }
}

/** What the stream says: a thumbnail made; a project locked or unlocked, so anything of it may read differently; or, starting over, that
 *  anything may have been. */
export function heardThumb(event: StreamEvent) {
  if (event.type === "thumb") release(keyOf(event.project, event.sha256))
  else if (event.type === "project.vault") for (const key of [...waiting.keys()].filter((key) => key.startsWith(`${event.id} `))) release(key)
  else if (event.type === "hello" || event.type === "resync") for (const key of [...waiting.keys()]) release(key)
}

function release(key: string) {
  const held = waiting.get(key)
  if (!held) return
  waiting.delete(key)
  for (const then of [...held]) then()
}
