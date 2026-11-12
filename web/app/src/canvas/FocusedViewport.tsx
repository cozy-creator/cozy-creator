import { useEffect, useRef } from "react"
import { useCanvasStore, useElement } from "./context"
import { FocusedCard } from "./Card"
import { Icon } from "../ui/Icon"
import { LET_GO, type LetGo } from "./carry"
import type { CarriedFile } from "./CanvasStore"
import { NameField } from "./NameField"
import { shownOf } from "./shown"
import "./FocusedViewport.css"

/** A workspace for one item. The Wander camera stays behind it, unchanged. */
export function FocusedViewport({ name, onClose, onCopyVersion }: { name: string; onClose(): void; onCopyVersion(file: CarriedFile, x: number, y: number): void }) {
  const store = useCanvasStore()
  const view = useElement(name)
  const element = useRef<HTMLElement>(null)
  useEffect(() => {
    const held = element.current
    if (!held) return
    const drop = (event: Event) => {
      const { load, x, y } = (event as LetGo).detail
      if (load.take) { event.stopPropagation(); onCopyVersion(load.take, x, y) }
      else if (load.tool) { event.stopPropagation(); store.putTool(name, load.tool, load.operation) }
    }
    held.addEventListener(LET_GO, drop)
    return () => held.removeEventListener(LET_GO, drop)
  }, [name, store, onCopyVersion])
  if (!view?.asset && !view?.entity) return null
  const version = shownOf(view)?.number
  return (
    <section ref={element} className="focus-viewport" aria-label="Focused asset" data-focused-asset={name}
      onContextMenu={(event) => {
        if ((event.target as Element).closest("input, textarea, [contenteditable]")) return
        event.preventDefault()
        store.menu.set({ name, x: event.clientX, y: event.clientY })
      }}>
      <header className="focus-heading">
        <button type="button" className="tb" aria-label="Back to canvas" onClick={onClose}><Icon name="back" />Back to canvas</button>
        <div className="focus-title">
          {view.draft !== undefined ? <NameField name={name} draft={view.draft} onCancel={() => store.patch(name, { draft: undefined })} /> : <button type="button" className="focus-name" aria-label="Rename node" title="Rename" onClick={() => store.openName(name)}>{view.name}</button>}
          {view.asset?.kind === "image" && <span className="focus-version-label" aria-label="Image version">{version === undefined ? "New" : `v${version}`}</span>}
        </div>
        <button type="button" className="iconb" aria-label="Close focus" title="Close focus" onClick={onClose}><Icon name="close" /></button>
      </header>
      <div className="focus-body"><FocusedCard name={name} /></div>
    </section>
  )
}
