import { ApiError, type Client } from "../api/client"
import type { HistoryEntry } from "../api/calcifer"
import { signal } from "../ui/signal"

/** What Undo or Redo takes: an entry of the project's history, or an edit of this page's that may not be one yet. */
export interface Target {
  entry?: number
  edit?: string
}

/** One thing this page did: the edit its writes share, its words until Calcifer's, and what it showed before, which Undo shows at once. */
export interface Step<S> {
  edit: string
  words: string
  before: S
  /** What it showed after, once undone: Redo shows it again. */
  after?: S
  entry?: number
}

/** What Undo or Redo would take now, as its button says it. */
export interface Next {
  target: Target
  words: string
  by?: HistoryEntry["by"]
}

const LISTED = 50
const STEPS = 200

const keyOf = (target: Target) => (target.entry !== undefined ? `#${target.entry}` : `edit:${target.edit}`)

/**
 * The project's history as this page knows it (Calcifer's `history`): every edit, whoever made it, and this page's own edits before Calcifer
 * says they're entries. Undo and Redo show at once what they'll come to, and go to Calcifer in their turn like any write, so the next press
 * goes on from there.
 */
export class PageHistory<S> {
  /** Newest first, as last read. */
  readonly entries = signal<HistoryEntry[]>([])
  /** What Undo and Redo take moved. */
  readonly moved = signal(0)
  /** Calcifer keeps a history: one older than this page doesn't, and has nothing to undo. */
  readonly kept = signal(true)
  private steps: Step<S>[] = []
  /** Undone (true) or done again (false) as this page asked, until the history says so. */
  private asked = new Map<string, boolean>()
  /** What this page undid, the last first: Redo takes these, until something new is done. */
  private redos: Target[] = []
  /** What Calcifer's Redo would take, as last read. */
  private redo?: number
  private reading = false
  private again = false
  private client: Client
  private project: () => string | null

  constructor(client: Client, project: () => string | null) {
    this.client = client
    this.project = project
  }

  private tell() {
    this.moved.set(this.moved.get() + 1)
  }

  /** Read again, once at a time: what comes meanwhile reads once more after. */
  read() {
    const id = this.project()
    if (!id) return
    if (this.reading) {
      this.again = true
      return
    }
    this.reading = true
    this.client.history(id, LISTED).then(
      (history) => {
        this.redo = history.redo ?? undefined
        for (const entry of history.entries) {
          for (const edit of entry.edits ?? []) {
            const step = this.steps.find((held) => held.edit === edit)
            if (step) step.entry = entry.id
          }
          // What this page asked, the history says now.
          for (const key of [`#${entry.id}`, ...(entry.edits ?? []).map((edit) => `edit:${edit}`)]) {
            if (this.asked.get(key) === Boolean(entry.undone)) this.asked.delete(key)
          }
        }
        // Writes go in order: one of this page's that isn't listed while a later one is made nothing.
        const listed = new Set(history.entries.flatMap((entry) => entry.edits ?? []))
        const last = this.steps.reduce((at, step, i) => (listed.has(step.edit) ? i : at), -1)
        this.steps = this.steps.filter((step, at) => at >= last || step.entry !== undefined || listed.has(step.edit))
        this.entries.set(history.entries)
        this.kept.set(true)
        this.tell()
      },
      (error) => {
        if (error instanceof ApiError && error.body.code === "unknown_route") this.kept.set(false)
      },
    ).finally(() => {
      this.reading = false
      if (this.again) {
        this.again = false
        this.read()
      }
    })
  }

  /** This page did something new: Redo no longer takes what came before it. */
  did(step: Step<S>) {
    this.steps.push(step)
    if (this.steps.length > STEPS) this.steps.shift()
    this.redos = []
    this.tell()
  }

  /** Its writes were refused: there's nothing to undo. */
  forget(edit: string) {
    this.steps = this.steps.filter((step) => step.edit !== edit)
    this.tell()
  }

  /** The newest of this page's own, if its last write is still on its way. */
  latest(): Step<S> | undefined {
    return this.steps.at(-1)
  }

  /** The page's ids for what `target` names. */
  editsOf(target: Target): string[] {
    return target.edit ? [target.edit] : (this.entries.get().find((entry) => entry.id === target.entry)?.edits ?? [])
  }

  stepOf(target: Target): Step<S> | undefined {
    const edits = target.edit ? [target.edit] : (this.entries.get().find((entry) => entry.id === target.entry)?.edits ?? [])
    return this.steps.find((step) => edits.includes(step.edit))
  }

  private undone(entry: HistoryEntry): boolean {
    for (const key of [`#${entry.id}`, ...(entry.edits ?? []).map((edit) => `edit:${edit}`)]) {
      const asked = this.asked.get(key)
      if (asked !== undefined) return asked
    }
    return Boolean(entry.undone)
  }

  /** This page's edits the history doesn't list yet, newest first. */
  private unlisted(): Step<S>[] {
    const listed = new Set(this.entries.get().flatMap((entry) => entry.edits ?? []))
    return this.steps.filter((step) => step.entry === undefined && !listed.has(step.edit)).reverse()
  }

  /** What Undo takes now: this page's newest, while it isn't an entry yet, else the newest entry that stands. */
  nextUndo(): Next | undefined {
    const own = this.unlisted().find((step) => !this.asked.get(`edit:${step.edit}`))
    if (own) return { target: { edit: own.edit }, words: own.words }
    const entry = this.entries.get().find((held) => !this.undone(held))
    return entry && { target: { entry: entry.id }, words: entry.words, by: entry.by }
  }

  /** What Redo takes now: what this page undid last, else what Calcifer's Redo would. */
  nextRedo(): Next | undefined {
    for (const target of this.redos) {
      if (!this.isUndone(target)) continue
      return { target, words: this.wordsOf(target) }
    }
    const entry = this.entries.get().find((held) => held.id === this.redo)
    return entry && this.undone(entry) ? { target: { entry: entry.id }, words: entry.words, by: entry.by } : undefined
  }

  private isUndone(target: Target): boolean {
    const asked = this.asked.get(keyOf(target))
    if (asked !== undefined) return asked
    const entry = this.entries.get().find((held) => (target.entry !== undefined ? held.id === target.entry : held.edits?.includes(target.edit ?? "")))
    return entry ? this.undone(entry) : false
  }

  wordsOf(target: Target): string {
    const entry = this.entries.get().find((held) => (target.entry !== undefined ? held.id === target.entry : held.edits?.includes(target.edit ?? "")))
    return entry?.words ?? this.stepOf(target)?.words ?? "that"
  }

  /** Undone (or done again) as this page asked, shown so until the history says. */
  mark(target: Target, undone: boolean) {
    this.asked.set(keyOf(target), undone)
    const key = keyOf(target)
    this.redos = this.redos.filter((held) => keyOf(held) !== key)
    if (undone) this.redos.unshift(target)
    this.tell()
  }

  /** What this page asked was refused: the history is as it says. */
  unmark(target: Target) {
    this.asked.delete(keyOf(target))
    this.tell()
  }

  /** What undoing back to `entry` takes, newest first: this page's not yet listed, and each entry since it that stands, it too. */
  backTo(entry: number): Target[] {
    const own = this.unlisted().filter((step) => !this.asked.get(`edit:${step.edit}`)).map((step) => ({ edit: step.edit }))
    return [...own, ...this.entries.get().filter((held) => held.id >= entry && !this.undone(held)).map((held) => ({ entry: held.id }))]
  }

  /** What redoing up to `entry` takes, in order: what was undone, the last undone first, down to it, as Calcifer takes them. */
  forthTo(entry: number): Target[] {
    const when = (held: HistoryEntry) => (this.asked.get(`#${held.id}`) ? Number.POSITIVE_INFINITY : (held.undone?.at_ms ?? 0))
    const undone = this.entries.get().filter((held) => this.undone(held)).sort((a, b) => when(b) - when(a) || a.id - b.id)
    const at = undone.findIndex((held) => held.id === entry)
    return at < 0 ? [] : undone.slice(0, at + 1).map((held) => ({ entry: held.id }))
  }

  /** Each entry, newest first, with this page's own ahead of their entries, and whether it's undone as this page shows it. */
  shown(): { target: Target; words: string; by?: HistoryEntry["by"]; at?: number; undone: boolean; saving?: boolean }[] {
    const own = this.unlisted().map((step) => ({ target: { edit: step.edit }, words: step.words, undone: Boolean(this.asked.get(`edit:${step.edit}`)), saving: true }))
    const listed = this.entries.get().map((entry) => ({ target: { entry: entry.id }, words: entry.words, by: entry.by, at: entry.at_ms, undone: this.undone(entry) }))
    return [...own, ...listed]
  }
}
