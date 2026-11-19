import type { StreamEvent } from "./calcifer"
import { type HeldMedia, Pending, type Transport } from "./transport"

/**
 * Thumbnails Calcifer is still making (cl-982). Each is made as its file lands and kept in its project; one asked for before that answers
 * `202 thumb_pending` at once, and the stream's `thumb` says when to ask again. A file still on its way here has no thumbnail and no event,
 * so a wait also ends after `ms`.
 */
const waiting = new Map<string, Set<() => void>>()
const keyOf = (project: string, sha256: string) => `${project} ${sha256.replace(/^sha256:/, "").toLowerCase()}`

/** Consecutive real failures before a thumbnail is unavailable; accepted pending work does not spend this budget. */
export const THUMB_TRIES = 6

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

/** Files that failed `THUMB_TRIES` times, by what was asked: none is asked for again until Calcifer next draws its file (`thumb`), its
 *  project is locked or unlocked, or the stream starts over. */
const unavailable = new Set<string>()

/** What a loader says of its last ask (`asking`). */
export interface Asking {
  /** Calcifer is still making it (`202 thumb_pending`): asked again on the file's `thumb` event, or `thumbWait`, spending no try. */
  pending(): void
  /** It wasn't had: refused, nothing answered, or what came won't decode. Asked again so, `THUMB_TRIES` times; then it's unavailable, and
   *  asked again only on the file's next event. */
  failed(): void
  /** Its project's lock cut it short: asked again on that project's next `project.vault`, its unlock. */
  held(): void
  /** It's drawn: the tries start over. */
  shown(): void
  /** The first ask: none while it's unavailable (`gone` is told), until the file's next event. */
  start(): void
  stop(): void
}

/**
 * The one way the page asks Calcifer for a file of `project`'s `sha256` until it has it, the same for a picture, a thumbnail and a video's
 * preview, on the board and in Focus: each loader asks with its own fetch and decode and says how that went, and `ask` is called again
 * when it's time, first at `start`. `path` names what's asked, so a preview and a thumbnail of one file are each unavailable alone;
 * `gone` is told when it is.
 */
export function asking(project: string, sha256: string, path: string, ask: () => void, gone?: () => void): Asking {
  const key = `${project} ${path}`
  let stopped = false, waits = 0, errors = 0
  let waiting: (() => void) | undefined
  // Asked again once every wait that event ended has run, so a failure it clears is gone by then.
  const later = (ms: number | undefined) => {
    waiting?.()
    waiting = whenThumb(project, sha256, ms, () => queueMicrotask(again))
  }
  // One already given up on, by this asker or another, is said to be so, and asked again only on its file's next event.
  const again = () => {
    waiting = undefined
    if (stopped) return
    if (unavailable.has(key)) {
      gone?.()
      return later(undefined)
    }
    ask()
  }
  return {
    pending() {
      if (stopped) return
      waits = Math.min(5, waits + 1)
      later(thumbWait(waits))
    },
    failed() {
      if (stopped) return
      waits = 0
      if (++errors < THUMB_TRIES) return later(thumbWait(errors))
      if (!unavailable.has(key)) {
        unavailable.add(key)
        whenThumb(project, sha256, undefined, () => unavailable.delete(key))
      }
      gone?.()
      later(undefined)
    },
    held() {
      if (!stopped) later(undefined)
    },
    shown() {
      waits = 0
      errors = 0
    },
    start: again,
    stop() {
      stopped = true
      waiting?.()
    },
  }
}

export interface HeldPreview {
  /** What it was handed couldn't play: Calcifer couldn't make it or answer, its file isn't here yet, or the browser can't decode it
   *  (`Asking.failed`). */
  failed(): void
  /** `then` isn't called after. */
  stop(): void
}

/**
 * `project`'s preview of `sha256`, Calcifer's `path`, in this page's memory (`Transport.media`) and handed to `then` once it's there, asked
 * for as every file is (`asking`): a `202` waits for the file's `thumb` event, and one that failed or that nothing answered is asked again.
 */
export function heldPreview(transport: Pick<Transport, "media">, project: string, sha256: string, path: string,
  then: (held: HeldMedia) => void): HeldPreview {
  let stopped = false
  const ask = () => {
    transport.media(path).then((held) => {
      if (stopped) return held.release()
      then(held)
    }, (error) => {
      if (stopped) return
      if (error instanceof DOMException && error.name === "AbortError") asked.held()
      else if (error instanceof Pending) asked.pending()
      else asked.failed()
    })
  }
  const asked = asking(project, sha256, path, ask)
  asked.start()
  return {
    failed: () => asked.failed(),
    stop() {
      stopped = true
      asked.stop()
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
