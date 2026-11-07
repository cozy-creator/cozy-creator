import { useEffect, useRef, useState } from "react"
import { TAB } from "../api/transport"
import { span } from "../panels/format"
import type { By } from "../types"
import { Icon } from "../ui/Icon"
import { useSignal } from "../ui/signal"
import { useCanvasStore } from "./context"

const MAC = /Mac|iPhone|iPad/.test(navigator.platform)
const UNDO_KEY = MAC ? "⌘Z" : "Ctrl+Z"
const REDO_KEY = MAC ? "⇧⌘Z" : "Ctrl+Shift+Z"

/** Who made it, as a person says it. */
function said(by?: By): string {
  if (!by) return "this tab"
  if (by.via === "page") return by.client === TAB ? "this tab" : "another tab"
  if (by.via === "cli") return "the CLI"
  if (by.via === "file") return "project.yaml"
  return by.client ?? "the API"
}

const ago = (at?: number) => (at === undefined ? "saving" : `${span((Date.now() - at) / 1000).replace(/(\d+[smhd]).*/, "$1")} ago`)

/** Undo and Redo of the project's history, whoever made what they take, and its list a click away, to go back or forward several at once. */
export function HistoryControls() {
  const store = useCanvasStore()
  const { history } = store
  useSignal(history.moved)
  useSignal(history.entries)
  const kept = useSignal(history.kept)
  const [open, setOpen] = useState(false)
  const held = useRef<HTMLDivElement>(null)
  const undo = kept ? history.nextUndo() : undefined
  const redo = kept ? history.nextRedo() : undefined
  const none = "This Calcifer keeps no history: it's older than this page"

  useEffect(() => {
    if (!open) return
    history.read()
    const away = (event: Event) => {
      if (!held.current?.contains(event.target as Node)) setOpen(false)
    }
    const esc = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false)
    }
    window.addEventListener("pointerdown", away)
    window.addEventListener("keydown", esc)
    return () => {
      window.removeEventListener("pointerdown", away)
      window.removeEventListener("keydown", esc)
    }
  }, [open, history])

  const rows = open ? history.shown() : []
  return (
    <div className="history" ref={held}>
      <button type="button" className="tb" aria-label="Undo" title={!kept ? none : undo ? `Undo: ${undo.words}, by ${said(undo.by)} (${UNDO_KEY})` : "Nothing to undo"} disabled={!undo} onClick={() => store.undo()}>
        <Icon name="undo" />
      </button>
      <button type="button" className="tb" aria-label="Redo" title={!kept ? none : redo ? `Redo: ${redo.words} (${REDO_KEY})` : "Nothing to redo"} disabled={!redo} onClick={() => store.redo()}>
        <Icon name="redo" />
      </button>
      <button type="button" className={`tb ${open ? "on" : ""}`} aria-label="History" aria-expanded={open} title={kept ? "History" : none} disabled={!kept} onClick={() => setOpen(!open)}>
        <Icon name="history" />
      </button>
      {open && (
        <div className="history-list" role="menu" aria-label="History">
          {rows.length === 0 && <p className="history-none">Nothing done yet</p>}
          {rows.map((row) => (
            <button
              type="button"
              role="menuitem"
              key={row.target.entry ?? row.target.edit}
              className={`history-row ${row.undone ? "undone" : ""}`}
              title={row.undone ? "Redo up to here" : "Undo back to here"}
              onClick={() => {
                if (row.target.entry !== undefined) store.stepTo(row.target.entry, !row.undone)
                else if (row.undone) store.redo(row.target)
                else store.undo(row.target)
              }}
            >
              <span className="history-words">{row.words}</span>
              <span className="history-by">
                {said(row.by)} · {row.saving ? "saving" : ago(row.at)}
              </span>
            </button>
          ))}
        </div>
      )}
    </div>
  )
}
