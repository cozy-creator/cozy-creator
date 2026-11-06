import { signal } from "../ui/signal"
import { ApiError, sessionLost } from "./client"

/** One thing a person did. The page already shows it; this is its way to Calcifer. */
export interface Action<T = unknown> {
  /** One try. `once` ends it. An `Idempotency-Key` is made with the action, so every try is the same ask. */
  send(once: AbortSignal): Promise<T>
  /** Calcifer has it. */
  done?(answer: T): void
  /** Calcifer said no: what the page showed goes, and a line says why. */
  refused?(error: unknown): void
  /** A try whose answer never came may have landed, and this try's refusal may only say so: what to take for its answer, or nothing. */
  landed?(error: ApiError): T | undefined
  /** As long as a run: its turn comes, and what's behind it goes on. */
  long?: boolean
}

/** What's on its way to Calcifer, as the top bar says it. */
export interface Owed {
  /** Changes not known to be saved. */
  count: number
  /** The one going has waited past a moment, or couldn't reach Calcifer: said until all are saved. */
  slow: boolean
  /** What Calcifer said kept it from saving, where that's more than not answering. */
  held?: string
}

/** What the queue took a try's failure for: Calcifer is the same, the way there isn't. */
const LOST = new Set([0, 408, 429, 502, 503, 504])
const FIRST_PAUSE_MS = 250
const LONGEST_PAUSE_MS = 5000
/** Waiting this long on one is worth a word. */
const SLOW_MS = 2000
const HELD = "file_held"

/** A try that didn't reach Calcifer, or whose answer never came back: the same ask goes again. */
export function lost(error: unknown): boolean {
  return (error instanceof ApiError && (LOST.has(error.status) || sessionLost(error))) || (error instanceof DOMException && error.name === "AbortError")
}

/**
 * Ordered, one at a time, for every project this tab has open: frontend first, then this, then the answer. What can't reach Calcifer waits here,
 * however long, and goes when it can. It lives as long as the tab: leaving with something in it asks the browser's own question.
 */
export class Outbox {
  readonly owed = signal<Owed>({ count: 0, slow: false })
  private queue: Action<any>[] = []
  /** What's on its way outside the queue: a file's bytes. */
  private outside = 0
  private going = false
  private trying?: AbortController
  private wakes = new Set<() => void>()
  private slowly?: ReturnType<typeof setTimeout>

  constructor() {
    addEventListener("beforeunload", (event) => {
      if (this.queue.length + this.outside) event.preventDefault()
    })
  }

  add<T>(action: Action<T>) {
    this.queue.push(action)
    this.tell()
    void this.drain()
  }

  /** Something on its way to Calcifer by itself, counted as waiting until it's ended. */
  owing(): () => void {
    this.outside += 1
    this.tell()
    let ended = false
    return () => {
      if (ended) return
      ended = true
      this.outside -= 1
      this.tell(this.queue.length + this.outside ? {} : { slow: false, held: undefined })
    }
  }

  /** Takes back what hasn't been tried. Whether `action` was still waiting. */
  takeBack(action: Action<any>): boolean {
    const at = this.queue.indexOf(action)
    if (at < 1) return false
    this.queue.splice(at, 1)
    this.tell()
    return true
  }

  /** Calcifer answers again: what's waiting goes now, and a try that may never be answered goes again. */
  back() {
    this.trying?.abort()
    for (const wake of [...this.wakes]) wake()
  }

  /** Waits out a try that couldn't reach Calcifer: longer each time, and no longer than until it answers again or `signal` ends it. */
  wait(tries: number, signal?: AbortSignal): Promise<void> {
    return new Promise((done) => {
      const end = () => {
        clearTimeout(timer)
        this.wakes.delete(end)
        signal?.removeEventListener("abort", end)
        done()
      }
      const timer = setTimeout(end, Math.min(LONGEST_PAUSE_MS, FIRST_PAUSE_MS * 2 ** (tries - 1)))
      this.wakes.add(end)
      signal?.addEventListener("abort", end, { once: true })
    })
  }

  private tell(change: Partial<Owed> = {}) {
    const now = { ...this.owed.get(), ...change, count: this.queue.length + this.outside }
    const was = this.owed.get()
    if (now.count !== was.count || now.slow !== was.slow || now.held !== was.held) this.owed.set(now)
  }

  private async drain() {
    if (this.going) return
    this.going = true
    while (this.queue.length) {
      const action = this.queue[0]
      clearTimeout(this.slowly)
      this.slowly = setTimeout(() => this.tell({ slow: true }), SLOW_MS)
      if (action.long) {
        this.queue.shift()
        this.tell()
        void this.attempt(action)
      } else {
        await this.attempt(action)
        this.queue.shift()
        this.tell()
      }
    }
    this.going = false
    clearTimeout(this.slowly)
    if (!this.outside) this.tell({ slow: false, held: undefined })
  }

  private async attempt<T>(action: Action<T>) {
    for (let tries = 1; ; tries++) {
      const trying = new AbortController()
      if (!action.long) this.trying = trying
      let answer: T
      try {
        answer = await action.send(trying.signal)
      } catch (error) {
        if (lost(error)) {
          this.tell({ slow: true, held: error instanceof ApiError && error.body.code === HELD ? error.message : undefined })
          if (!trying.signal.aborted) await this.wait(tries)
          continue
        }
        const was = tries > 1 && error instanceof ApiError ? action.landed?.(error) : undefined
        if (was === undefined) action.refused?.(error)
        else action.done?.(was)
        return
      }
      action.done?.(answer)
      return
    }
  }
}
