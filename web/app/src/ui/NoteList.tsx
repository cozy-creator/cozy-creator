import { useEffect, useLayoutEffect, useRef, useState } from "react"
import { useConnection } from "../app/Connection"
import { CopyError } from "./CopyError"
import { Icon } from "./Icon"
import { close, details, dismiss, hasMore, held, type Note, notes, openHistory, opened, openNote, past, unread } from "./notes"
import { pressBeganOutside } from "./press"
import { useSignal } from "./signal"
import "./NoteList.css"

/** A note's picture: a take's thumbnail, fetched as the page fetches any (with its session). */
function NotePicture({ path }: { path: string }) {
  const { client } = useConnection()
  const [src, setSrc] = useState<string>()
  useEffect(() => {
    let live = true
    void client.transport.src(path).then((url) => live && setSrc(url), () => {})
    return () => {
      live = false
    }
  }, [client, path])
  return src ? <img className="note-picture" src={src} alt="" draggable={false} /> : <span className="note-picture" />
}

/** Drawn as Icon draws: 20 by 20, in the text's colour. */
const Glyph = ({ d }: { d: string }) => (
  <svg className="icon" viewBox="0 0 20 20" width="14" height="14" aria-hidden="true" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round">
    <path d={d} />
  </svg>
)
const BELL = "M10 3.2a4.8 4.8 0 00-4.8 4.8v3.6L3.7 14h12.6l-1.5-2.4V8A4.8 4.8 0 0010 3.2zM8.2 16.4a1.9 1.9 0 003.6 0"
const MORE = "M5.5 12.5L10 8l4.5 4.5"

/** One note on one line (Paul, 11-16): its words cut short with an ellipsis, its buttons on the line. One with more to see than its line
 *  shows (words cut short, a remedy, an error's details, a picture) opens in full on a click or on its ⌃; any other doesn't. */
function Line({ note, live, open }: { note: Note; live: boolean; open: () => void }) {
  const words = useRef<HTMLSpanElement>(null)
  const [cut, setCut] = useState(false)
  useLayoutEffect(() => {
    const line = words.current
    if (!line) return
    const measure = () => setCut(line.scrollWidth > line.clientWidth + 1)
    measure()
    const sized = new ResizeObserver(measure)
    sized.observe(line)
    return () => sized.disconnect()
  }, [note.message])
  const opens = cut || hasMore(note)
  return (
    <div
      className="note"
      data-note={note.id}
      data-opens={opens || undefined}
      onPointerEnter={live ? () => held.add(note.id) : undefined}
      onPointerLeave={live ? () => held.delete(note.id) : undefined}
      onClick={(event) => {
        if (!opens || (event.target instanceof Element && event.target.closest("button"))) return
        // Words selected to copy, not a click.
        const selected = window.getSelection()
        if (selected && !selected.isCollapsed && event.currentTarget.contains(selected.anchorNode)) return
        open()
      }}
    >
      {note.picture && <NotePicture path={note.picture} />}
      <span ref={words} className="line">
        {note.message}
      </span>
      {opens && (
        <button type="button" className="iconb note-more" aria-label="Show all" title="Show all" onClick={open}>
          <Glyph d={MORE} />
        </button>
      )}
      {note.copy && <CopyError text={note.copy} />}
      {live && note.action && (
        <button
          type="button"
          className="tb note-act"
          onClick={() => {
            dismiss(note.id)
            note.action?.run()
          }}
        >
          {note.action.label}
        </button>
      )}
      {live && (
        <button type="button" className="iconb" aria-label="Dismiss" onClick={() => dismiss(note.id)}>
          <Icon name="close" />
        </button>
      )}
    </div>
  )
}

/** A note in full: its whole message, remedy and picture, and an error's details and whole text to copy. While it's open no note goes. */
function Opened({ note, live, back }: { note: Note; live: boolean; back: () => void }) {
  const shell = useRef<HTMLDivElement>(null)
  useLayoutEffect(() => shell.current?.focus({ preventScroll: true }), [])
  const more = details(note)
  return (
    <div ref={shell} className="note-open" role="dialog" aria-label="Notification" tabIndex={-1} onKeyDown={(event) => event.stopPropagation()}>
      <div className="note-open-head">
        <p className="note-open-message">{note.message}</p>
        {note.copy && <CopyError text={note.copy} />}
        <button type="button" className="iconb" aria-label="Close" title="Close" onClick={back}>
          <Icon name="close" />
        </button>
      </div>
      {note.picture && (
        <div className="note-open-picture">
          <NotePicture path={note.picture} />
        </div>
      )}
      {note.remedy && <p className="note-open-remedy">{note.remedy}</p>}
      {more && <pre className="note-open-details">{more}</pre>}
      {live && note.action && (
        <div className="note-open-acts">
          <button
            type="button"
            className="tb"
            onClick={() => {
              close()
              dismiss(note.id)
              note.action?.run()
            }}
          >
            {note.action.label}
          </button>
        </div>
      )}
    </div>
  )
}

/** Every note this page has said, newest first, each on its line as it was said; one opens in full as a note does. */
function History({ said }: { said: Note[] }) {
  const shell = useRef<HTMLDivElement>(null)
  useLayoutEffect(() => shell.current?.focus({ preventScroll: true }), [])
  return (
    <div ref={shell} className="note-open notes-log" role="dialog" aria-label="Notifications" tabIndex={-1} onKeyDown={(event) => event.stopPropagation()}>
      {said.length ? (
        <ul>
          {[...said].reverse().map((note) => (
            <li key={note.id}>
              <Line note={note} live={false} open={() => openNote(note.id, true)} />
            </li>
          ))}
        </ul>
      ) : (
        <p className="notes-none">No notifications yet.</p>
      )}
    </div>
  )
}

/** The notes against the window's bottom left (Paul, 11-16), over the Notifications button. A note or the history opens in the notes' place;
 *  Esc or its ✕ closes it (a note opened from the history goes back to it), as does a click whose press began outside it (Paul, 11-12). */
export function Notes() {
  const lines = useSignal(notes)
  const said = useSignal(past)
  const open = useSignal(opened)
  const count = useSignal(unread)
  const root = useRef<HTMLDivElement>(null)
  // Where the keys go once what's open closes: back where they came from.
  const refocus = useRef<string | null>(null)
  useLayoutEffect(() => {
    if (!refocus.current) return
    const to = root.current?.querySelector<HTMLElement>(refocus.current) ?? root.current?.querySelector<HTMLElement>(".notes-bell")
    refocus.current = null
    to?.focus({ preventScroll: true })
  })
  const listing = Boolean(open && "history" in open)
  const shown = open && "note" in open ? (said.find((note) => note.id === open.note) ?? lines.find((note) => note.id === open.note)) : undefined
  const back = () => {
    const now = opened.get()
    if (!now) return
    if ("history" in now) {
      refocus.current = ".notes-bell"
      return close()
    }
    refocus.current = `[data-note="${now.note}"] .note-more`
    if (now.back) openHistory()
    else close()
  }
  useEffect(() => {
    if (!open) return
    const inside = (element: Element) => Boolean(root.current?.contains(element))
    const click = (event: MouseEvent) => {
      if (event.button === 0 && event.target instanceof Element && !inside(event.target) && pressBeganOutside(inside)) close()
    }
    const key = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return
      event.preventDefault()
      event.stopPropagation()
      back()
    }
    document.addEventListener("click", click, true)
    window.addEventListener("keydown", key, true)
    return () => {
      document.removeEventListener("click", click, true)
      window.removeEventListener("keydown", key, true)
    }
  }, [open])
  useEffect(() => {
    if (open && "note" in open && !shown) close()
  }, [open, shown])
  return (
    <div className="notes" ref={root}>
      {listing ? (
        <History said={said} />
      ) : shown ? (
        <Opened note={shown} live={lines.some((note) => note.id === shown.id)} back={back} />
      ) : (
        lines.length > 0 && (
          <div className="note-stack" role="status">
            {lines.map((note) => (
              <Line key={note.id} note={note} live open={() => openNote(note.id, false)} />
            ))}
          </div>
        )
      )}
      <button
        type="button"
        className="notes-bell"
        aria-label="Notifications"
        title="Notifications"
        aria-haspopup="dialog"
        aria-expanded={listing}
        onClick={() => (listing ? back() : openHistory())}
      >
        <Glyph d={BELL} />
        {count > 0 && <span className="notes-count">{count > 99 ? "99+" : count}</span>}
      </button>
    </div>
  )
}
