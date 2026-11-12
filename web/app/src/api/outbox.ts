import { signal } from "../ui/signal"
import { ApiError, sessionLost } from "./client"

/** One thing a person did. The page already shows it; this is its way to Calcifer. */
export interface Action<T = unknown> {
  /** Local project privacy scope; never serialized or sent as authority. */
  project?: string | null
  /** One try. `once` ends it. `action` is made with the action and goes with every try (`Write.action`), so every try is the same ask. */
  send(once: AbortSignal, action: string): Promise<T>
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
export type OutboxPort = Pick<Outbox, "add" | "owing" | "back" | "wait" | "owed">

export class Outbox {
  readonly owed = signal<Owed>({ count: 0, slow: false })
  private queue: Action<any>[] = []
  private frozen = false
  private closeRefused = false
  /** What's on its way outside the queue: a file's bytes. */
  private outside = 0
  private going = false
  private trying?: AbortController
  private wakes = new Set<() => void>()
  private slowly?: ReturnType<typeof setTimeout>
  private blocked = new Set<string>()
  private cancelled = new WeakSet<Action<any>>()
  private active = new Map<Action<any>, AbortController>()

  /** The existing shared queue, with project ownership attached by its canvas. */
  scoped(project: () => string | null): OutboxPort {
    return {
      owed: this.owed,
      add: (action) => this.add({ ...action, project: project() }),
      owing: () => this.owing(),
      back: () => this.back(),
      wait: (tries, signal) => this.wait(tries, signal),
    }
  }

  allowProject(project: string) { this.blocked.delete(project) }

  /** Cancel only this project's queued bodies and suppress late private callbacks. */
  discardProject(project: string) {
    this.blocked.add(project)
    const error = new ApiError(423, { kind: "conflict", code: "project_locked", message: "The project was locked before this change was saved" })
    const discarded = new Set([...this.queue, ...this.active.keys()].filter((action) => action.project === project))
    for (const action of discarded) {
      this.cancelled.add(action)
      this.active.get(action)?.abort()
      this.active.delete(action)
      action.refused?.(error)
    }
    this.queue = this.queue.filter((action) => !discarded.has(action))
    for (const wake of [...this.wakes]) wake()
    this.tell({ slow: false, held: undefined })
  }

  constructor() {
    addEventListener("beforeunload", (event) => {
      if (this.queue.length + this.outside) event.preventDefault()
    })
  }

  freeze() { this.frozen = true; this.closeRefused = false }
  resume() { this.frozen = false }
  async saved(): Promise<void> {
    while (this.owed.get().count) {
      if (this.closeRefused) throw new Error("A pending edit could not be saved")
      await new Promise<void>((done) => { const stop = this.owed.subscribe(() => { stop(); done() }) })
    }
    if (this.closeRefused) throw new Error("A pending edit could not be saved")
  }

  add<T>(action: Action<T>) {
    if (this.frozen) {
      action.refused?.(new ApiError(409, { kind: "conflict", code: "browser_closing", message: "The Cozy browser is saving its edits before closing" }))
      return
    }
    if (action.project && this.blocked.has(action.project)) {
      action.refused?.(new ApiError(423, { kind: "conflict", code: "project_locked", message: "Unlock the project before saving this change" }))
      return
    }
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
    const now = { ...this.owed.get(), ...change, count: this.queue.length + this.outside + [...this.active.keys()].filter((action) => !this.queue.includes(action)).length }
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
        if (this.queue[0] === action) this.queue.shift()
        this.tell()
      }
    }
    this.going = false
    clearTimeout(this.slowly)
    if (!this.outside) this.tell({ slow: false, held: undefined })
  }

  private async attempt<T>(action: Action<T>) {
    const id = crypto.randomUUID()
    for (let tries = 1; ; tries++) {
      if (this.cancelled.has(action)) return
      const trying = new AbortController()
      this.active.set(action, trying)
      if (!action.long) this.trying = trying
      let answer: T
      try {
        answer = await action.send(trying.signal, id)
      } catch (error) {
        if (this.cancelled.has(action)) return
        if (lost(error)) {
          this.tell({ slow: true, held: error instanceof ApiError && error.body.code === HELD ? error.message : undefined })
          if (!trying.signal.aborted) await this.wait(tries)
          continue
        }
        const was = tries > 1 && error instanceof ApiError ? action.landed?.(error) : undefined
        this.active.delete(action)
        if (this.frozen && was === undefined) this.closeRefused = true
        this.tell()
        if (was === undefined) action.refused?.(error)
        else action.done?.(was)
        return
      }
      this.active.delete(action)
      this.tell()
      if (!this.cancelled.has(action)) action.done?.(answer)
      return
    }
  }
}
