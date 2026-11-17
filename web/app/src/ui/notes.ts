import { ApiError } from "../api/client"
import { signal } from "./signal"
import { bodyText, errorText } from "./CopyError"

/** A line the page says: a file it couldn't take, a move that didn't land. What it offers is a button, never a command to run. */
export interface Note {
  id: number
  message: string
  remedy?: string
  /** What it offers, for as long as it's up. */
  action?: { label: string; run: () => void }
  /** An error's whole text, to copy and paste: every error the page says has it. */
  copy?: string
  /** A small picture of what it's about: a take's thumbnail, by its path on Calcifer (cl-937). */
  picture?: string
}

/** Notes the pointer is on. While it's on any, none goes: one gone from under it moves the rest, and the one reached for stays put. */
export const held = new Set<number>()

export const notes = signal<Note[]>([])
/** Every note this page has said, newest last (Paul, 11-16: "go back and read ones you missed"). Kept for the page's life, nowhere else. */
export const past = signal<Note[]>([])
/** Notes said since the history was last open. */
export const unread = signal(0)
/** What's open over the notes: the history, or one note in full (`back`: Esc goes back to the history). */
export const opened = signal<{ history: true } | { note: number; back: boolean } | null>(null)
let last = 0

/** Every note goes by itself (Paul, 11-16): after this long, or `OFFERED_MS` with something to press. */
const SAID_MS = 6_000
const OFFERED_MS = 10_000
const KEPT = 100

export function say(message: string, error?: unknown, offer?: { action?: NonNullable<Note["action"]>; picture?: string }): number {
  const body = error instanceof ApiError ? error.body : undefined
  const reason = error instanceof Error ? error.message : undefined
  const text = reason && reason !== message ? `${message}: ${reason}` : message
  const id = ++last
  const copy = error === undefined ? undefined : body ? bodyText({ ...body, message: text }) : errorText({ message: text })
  const note = { id, message: text, remedy: body?.remedy ?? undefined, action: offer?.action, copy, picture: offer?.picture }
  notes.set([...notes.get().slice(-7), note])
  past.set([...past.get().slice(1 - KEPT), note])
  const open = opened.get()
  if (!open || ("back" in open && !open.back)) unread.set(unread.get() + 1)
  expire(id, offer?.action ? OFFERED_MS : SAID_MS)
  return id
}

/** Its time runs out after `ms`, unless the notes are held then: the pointer or the keys on one, its text selected, or a note or the history
 *  open over them. Then it waits until they're let go. */
function expire(id: number, ms: number) {
  setTimeout(() => {
    const selected = window.getSelection()
    const inIt = Boolean(selected && !selected.isCollapsed && selected.anchorNode?.parentElement?.closest(`[data-note="${id}"]`))
    const keys = Boolean(document.activeElement?.closest(".note-stack"))
    if (opened.get() || notes.get().some((note) => held.has(note.id)) || inIt || keys) expire(id, 1_000)
    else dismiss(id)
  }, ms)
}

export function dismiss(id: number) {
  held.delete(id)
  notes.set(notes.get().filter((note) => note.id !== id))
}

/** What its one line leaves out, beyond words cut short: the error's text past its message and remedy (its code, run, what it said in full). */
export const details = (note: Note) =>
  note.copy
    ?.split("\n")
    .filter((line) => line !== note.message && line !== note.remedy)
    .join("\n") ?? ""

/** A note opens in full when its line can't show all of it: a remedy, an error's details or a picture (or its words cut short, which the line
 *  measures). */
export const hasMore = (note: Note) => Boolean(note.remedy || note.picture || details(note))

export function openNote(id: number, back: boolean) {
  held.clear()
  opened.set({ note: id, back })
}

export function openHistory() {
  held.clear()
  unread.set(0)
  opened.set({ history: true })
}

export function close() {
  opened.set(null)
}
