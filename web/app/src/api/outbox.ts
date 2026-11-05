import { dismiss, say } from "../ui/notes"
import { ApiError } from "./client"

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

/** What the queue took a try's failure for: Calcifer is the same, the way there isn't. */
const LOST = new Set([0, 401, 408, 429, 502, 503, 504])
const FIRST_PAUSE_MS = 250
const LONGEST_PAUSE_MS = 5000
/** Nothing heard for this long is worth one line. */
const SAYS_AFTER_MS = 8000

function lost(error: unknown): boolean {
  return (error instanceof ApiError && LOST.has(error.status)) || (error instanceof DOMException && error.name === "AbortError")
}

/** Ordered, one at a time: frontend first, then this, then the answer. What can't reach Calcifer waits here, and goes when it can. */
export class Outbox {
  private queue: Action<any>[] = []
  private going = false
  private trying?: AbortController
  private wake?: () => void
  private note?: number
  private silence?: ReturnType<typeof setTimeout>

  /** Whether something isn't known to be saved. */
  get owed() {
    return this.queue.length > 0
  }

  add<T>(action: Action<T>) {
    this.queue.push(action)
    void this.drain()
  }

  /** Takes back what hasn't been tried. Whether `action` was still waiting. */
  takeBack(action: Action<any>): boolean {
    const at = this.queue.indexOf(action)
    if (at < 1) return false
    this.queue.splice(at, 1)
    return true
  }

  /** Calcifer answers again: what's waiting goes now, and a try that may never be answered goes again. */
  back() {
    this.trying?.abort()
    this.wake?.()
  }

  private async drain() {
    if (this.going) return
    this.going = true
    while (this.queue.length) {
      const action = this.queue[0]
      this.listen()
      if (action.long) {
        this.queue.shift()
        void this.attempt(action)
      } else {
        await this.attempt(action)
        this.queue.shift()
      }
    }
    this.going = false
    clearTimeout(this.silence)
    this.silence = undefined
    if (this.note !== undefined) dismiss(this.note)
    this.note = undefined
  }

  /** Each action has this long before the wait is said, once, however long it goes on. */
  private listen() {
    clearTimeout(this.silence)
    this.silence = setTimeout(() => {
      this.note ??= say("Not saved yet: Calcifer isn't answering, and this goes as soon as it does")
    }, SAYS_AFTER_MS)
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
          if (!trying.signal.aborted) await this.pause(tries)
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

  private pause(tries: number): Promise<void> {
    return new Promise((done) => {
      const timer = setTimeout(done, Math.min(LONGEST_PAUSE_MS, FIRST_PAUSE_MS * 2 ** (tries - 1)))
      this.wake = () => {
        clearTimeout(timer)
        done()
      }
    })
  }
}
