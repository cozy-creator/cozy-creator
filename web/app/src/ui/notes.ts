import { ApiError } from "../api/client"
import { signal } from "./signal"
import { bodyText, errorText } from "./CopyError"

/** A line the page says: a file it couldn't take, a move that didn't land. `next` holds commands, shown as text to copy. */
export interface Note {
  id: number
  message: string
  remedy?: string
  next?: string[]
  /** What it offers, for as long as it's up. */
  action?: { label: string; run: () => void }
  /** An error's whole text, to copy and paste (code, commands): every error the page says has it. */
  copy?: string
}

/** Notes held up: hovered, or with text selected in them. Their time runs out only once they're let go. */
export const held = new Set<number>()

export const notes = signal<Note[]>([])
let last = 0

const SAID_MS = 5_000
const FAILED_MS = 12_000

export function say(message: string, error?: unknown, offer?: { action: NonNullable<Note["action"]>; ms: number }): number {
  const body = error instanceof ApiError ? error.body : undefined
  const reason = error instanceof Error ? error.message : undefined
  const text = reason && reason !== message ? `${message}: ${reason}` : message
  const id = ++last
  const copy = error === undefined ? undefined : body ? bodyText({ ...body, message: text }) : errorText({ message: text })
  notes.set([...notes.get().slice(-7), { id, message: text, remedy: body?.remedy ?? undefined, next: body?.next, action: offer?.action, copy }])
  expire(id, offer?.ms ?? (error ? FAILED_MS : SAID_MS))
  return id
}

/** Its time runs out after `ms`, unless it's held then: hovered, or its text selected. Then it waits until it's let go. */
function expire(id: number, ms: number) {
  setTimeout(() => {
    const selected = window.getSelection()
    const inIt = Boolean(selected && !selected.isCollapsed && selected.anchorNode?.parentElement?.closest(`[data-note="${id}"]`))
    if (held.has(id) || inIt) expire(id, 1_000)
    else dismiss(id)
  }, ms)
}

export function dismiss(id: number) {
  notes.set(notes.get().filter((note) => note.id !== id))
}
