import { ApiError } from "../api/client"
import { signal } from "./signal"

/** A line the page says: a file it couldn't take, a move that didn't land. `next` holds commands, shown as text to copy. */
export interface Note {
  id: number
  message: string
  remedy?: string
  next?: string[]
  /** What it offers, for as long as it's up. */
  action?: { label: string; run: () => void }
}

export const notes = signal<Note[]>([])
let last = 0

export function say(message: string, error?: unknown, offer?: { action: NonNullable<Note["action"]>; ms: number }) {
  const body = error instanceof ApiError ? error.body : undefined
  const reason = error instanceof Error ? error.message : undefined
  const text = reason && reason !== message ? `${message}: ${reason}` : message
  const id = ++last
  notes.set([...notes.get().slice(-7), { id, message: text, remedy: body?.remedy, next: body?.next, action: offer?.action }])
  if (offer) setTimeout(() => dismiss(id), offer.ms)
}

export function dismiss(id: number) {
  notes.set(notes.get().filter((note) => note.id !== id))
}
