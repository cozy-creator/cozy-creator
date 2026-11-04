import { Copyable } from "./Copyable"
import { dismiss, notes } from "./notes"
import { useSignal } from "./signal"

export function Notes() {
  const lines = useSignal(notes)
  if (!lines.length) return null
  return (
    <div className="notes" role="status">
      {lines.map((note) => (
        <div className="note" key={note.id}>
          <span className="line">{note.message}</span>
          {note.remedy && <span className="remedy">{note.remedy}</span>}
          {note.next?.map((command) => (
            <Copyable key={command} text={command} />
          ))}
          <button type="button" className="iconb" aria-label="Dismiss" onClick={() => dismiss(note.id)}>
            ×
          </button>
        </div>
      ))}
    </div>
  )
}
