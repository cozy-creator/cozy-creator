import { useRef } from "react"
import { useCanvasStore, useElement } from "./context"
import { FocusedCard } from "./Card"
import { Icon } from "../ui/Icon"
import "./FocusedViewport.css"

/** A workspace for one item. The Wander camera stays behind it, unchanged. */
export function FocusedViewport({ name, onClose }: { name: string; onClose(): void }) {
  const store = useCanvasStore()
  const view = useElement(name)
  const pressed = useRef<{ x: number; y: number } | null>(null)
  if (!view?.asset && !view?.entity) return null
  const controls = "button, input, select, textarea, video, audio, [contenteditable], [role=slider], .here-side, .here-under, .tl, .orbit, [role=dialog]"
  return (
    <section className="focus-viewport" aria-label="Focused asset" data-focused-asset={name}
      onContextMenu={(event) => {
        if ((event.target as Element).closest("input, textarea, [contenteditable]")) return
        event.preventDefault()
        store.menu.set({ name, x: event.clientX, y: event.clientY })
      }}
      onWheel={(event) => { if (!(event.target as Element).closest(controls)) onClose() }}
      onPointerDown={(event) => {
        if (event.button === 0 && !(event.target as Element).closest(controls)) pressed.current = { x: event.clientX, y: event.clientY }
      }}
      onPointerMove={(event) => {
        const start = pressed.current
        if (start && event.buttons && Math.hypot(event.clientX - start.x, event.clientY - start.y) >= 6) { pressed.current = null; onClose() }
      }}
      onPointerUp={() => { pressed.current = null }} onPointerCancel={() => { pressed.current = null }}>
      <header className="focus-heading">
        <button type="button" className="tb" aria-label="Back to canvas" onClick={onClose}><Icon name="back" />Back to canvas</button>
        <h2>{view.name}</h2>
        <button type="button" className="iconb" aria-label="Close focus" title="Close focus" onClick={onClose}><Icon name="close" /></button>
      </header>
      <div className="focus-body"><FocusedCard name={name} /></div>
    </section>
  )
}
