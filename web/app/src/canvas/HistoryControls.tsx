import { TAB } from "../api/transport"
import type { By } from "../api/calcifer"
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

/** Undo and Redo of the project's history, with the shared file picker beside them. */
export function HistoryControls({ onAddFiles }: { onAddFiles: () => void }) {
  const store = useCanvasStore()
  const { history } = store
  useSignal(history.moved)
  useSignal(history.entries)
  const kept = useSignal(history.kept)
  const undo = kept ? history.nextUndo() : undefined
  const redo = kept ? history.nextRedo() : undefined
  const none = "This Calcifer keeps no history: it's older than this page"

  return (
    <div className="history">
      <button type="button" className="tb" aria-label="Undo" title={!kept ? none : undo ? `Undo: ${undo.words}, by ${said(undo.by)} (${UNDO_KEY})` : "Nothing to undo"} disabled={!undo} onClick={() => store.undo()}>
        <Icon name="undo" />
      </button>
      <button type="button" className="tb" aria-label="Redo" title={!kept ? none : redo ? `Redo: ${redo.words} (${REDO_KEY})` : "Nothing to redo"} disabled={!redo} onClick={() => store.redo()}>
        <Icon name="redo" />
      </button>
      <button type="button" className="tb" aria-label="Add files" title="Add files" onClick={onAddFiles}>
        <Icon name="plus" />
      </button>
    </div>
  )
}
