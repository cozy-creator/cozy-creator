import { Copyable } from "./Copyable"
import { CopyError } from "./CopyError"
import { Icon } from "./Icon"
import { dismiss, held, notes } from "./notes"
import { useSignal } from "./signal"

export function Notes() {
  const lines = useSignal(notes)
  if (!lines.length) return null
  return (
    <div className="notes" role="status">
      {lines.map((note) => (
        <div className="note" key={note.id} data-note={note.id} onPointerEnter={() => held.add(note.id)} onPointerLeave={() => held.delete(note.id)}>
          <span className="line">{note.message}</span>
          {note.copy && <CopyError text={note.copy} />}
          {note.remedy && <span className="remedy">{note.remedy}</span>}
          {note.next?.map((command) => (
            <Copyable key={command} text={command} />
          ))}
          {note.action && (
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
          <button type="button" className="iconb" aria-label="Dismiss" onClick={() => dismiss(note.id)}>
            <Icon name="close" />
          </button>
        </div>
      ))}
    </div>
  )
}
