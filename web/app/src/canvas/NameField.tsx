import { type ChangeEvent, type KeyboardEvent, useLayoutEffect, useRef } from "react"
import { useCanvasStore } from "./context"
import { typingName } from "./names"

/** The name as it will be, as it's typed. Enter or clicking away accepts; Esc cancels an import, or a rename. */
export function NameField({ name, draft, onCancel }: { name: string; draft: string; onCancel: () => void }) {
  const store = useCanvasStore()
  const input = useRef<HTMLInputElement>(null)
  const caret = useRef<number | null>(null)
  const cancelled = useRef(false)

  useLayoutEffect(() => {
    input.current?.focus({ preventScroll: true })
    input.current?.select()
  }, [])

  useLayoutEffect(() => {
    if (caret.current === null) return
    input.current?.setSelectionRange(caret.current, caret.current)
    caret.current = null
  }, [draft])

  const change = (event: ChangeEvent<HTMLInputElement>) => {
    const { value, selectionStart } = event.target
    if ((event.nativeEvent as InputEvent).isComposing) return store.draft(name, value)
    const next = typingName(value)
    caret.current = Math.max(0, next.length - (value.length - (selectionStart ?? value.length)))
    store.draft(name, next)
  }

  const key = (event: KeyboardEvent<HTMLInputElement>) => {
    event.stopPropagation()
    if (event.key === "Enter") input.current?.blur()
    if (event.key === "Escape") {
      cancelled.current = true
      onCancel()
    }
  }

  return (
    <input
      ref={input}
      className="nm-field nodrag nopan"
      aria-label="Name"
      spellCheck={false}
      value={draft}
      onChange={change}
      onCompositionEnd={(event) => store.draft(name, typingName(event.currentTarget.value))}
      onKeyDown={key}
      onBlur={(event) => {
        if (!cancelled.current) store.acceptName(name, event.currentTarget.value)
      }}
    />
  )
}
