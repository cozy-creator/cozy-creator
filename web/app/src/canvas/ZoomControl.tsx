import { type KeyboardEvent, useEffect, useId, useRef, useState } from "react"
import { Icon } from "../ui/Icon"

/** The zooms − and + (and ⌘− ⌘+) step along, the canvas's own ends at either end. A pinch or Ctrl and the wheel stays smooth. */
export const ZOOMS = [0.05, 0.1, 0.15, 0.2, 0.25, 0.33, 0.5, 0.67, 0.75, 0.9, 1, 1.25, 1.5, 2, 2.5, 3, 4]

/** The next zoom along from `zoom`, in or out: a zoom between steps goes to the nearest one that way. */
export const stepZoom = (zoom: number, by: 1 | -1) =>
  by > 0 ? (ZOOMS.find((step) => step > zoom * 1.001) ?? ZOOMS[ZOOMS.length - 1]) : ([...ZOOMS].reverse().find((step) => step < zoom / 1.001) ?? ZOOMS[0])

const MAC = /Mac|iPhone|iPad/.test(navigator.platform)
/** A shortcut as this computer writes it: a Mac's in its symbols, run together (⌘+, ⇧1), others' in words (Ctrl +, Shift 1). */
const MAC_KEYS: Record<string, string> = { Mod: "⌘", Shift: "⇧" }
export const keys = (...parts: string[]) => parts.map((part) => (MAC ? (MAC_KEYS[part] ?? part) : part === "Mod" ? "Ctrl" : part)).join(MAC ? "" : " ")

export type ZoomItem = { label: string; keys?: string; run: () => void; disabled?: boolean } | "rule"

/** − the zoom +, in a corner of the canvas. The zoom opens what else it does, in place: each with its shortcut. */
export function ZoomControl({ zoom, zoomIn, zoomOut, items }: { zoom: number; zoomIn: () => void; zoomOut: () => void; items: () => ZoomItem[] }) {
  const [open, setOpen] = useState(false)
  const held = useRef<HTMLDivElement>(null)
  const button = useRef<HTMLButtonElement>(null)
  const menu = useId()
  const percent = `${Math.round(zoom * 100)}%`

  useEffect(() => {
    if (!open) return
    held.current?.querySelector<HTMLButtonElement>("[role=menuitem]:not(:disabled)")?.focus()
    const away = (event: PointerEvent) => {
      if (!held.current?.contains(event.target as Node)) setOpen(false)
    }
    window.addEventListener("pointerdown", away)
    return () => window.removeEventListener("pointerdown", away)
  }, [open])

  const shut = () => {
    setOpen(false)
    button.current?.focus()
  }
  // What's pressed here is this control's: Enter on a button presses it, not the take a box being worked on shows, and Esc shuts the menu.
  const press = (event: KeyboardEvent) => {
    if (event.key === "Enter" || event.key === " ") return event.stopPropagation()
    if (!open) return
    const rows = [...(held.current?.querySelectorAll<HTMLButtonElement>("[role=menuitem]:not(:disabled)") ?? [])]
    const at = rows.indexOf(document.activeElement as HTMLButtonElement)
    const to = { ArrowDown: at + 1, ArrowUp: at - 1, Home: 0, End: rows.length - 1 }[event.key]
    if (event.key === "Escape") shut()
    else if (event.key === "Tab") return setOpen(false)
    else if (to !== undefined) rows[(to + rows.length) % rows.length]?.focus()
    else return
    event.preventDefault()
    event.stopPropagation()
  }

  return (
    <div className="zoom" ref={held} onKeyDown={press}>
      <button type="button" className="zoom-step" onClick={zoomOut} aria-label="Zoom out" title={`Zoom out  ${keys("Mod", "−")}`}>
        <Icon name="minus" />
      </button>
      <button
        ref={button}
        type="button"
        className="zoom-at"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? menu : undefined}
        aria-label={`Zoom ${percent}, more zoom options`}
        onClick={() => setOpen(!open)}
      >
        {percent}
      </button>
      <button type="button" className="zoom-step" onClick={zoomIn} aria-label="Zoom in" title={`Zoom in  ${keys("Mod", "+")}`}>
        <Icon name="plus" />
      </button>
      {open && (
        <div className="zoom-menu" id={menu} role="menu" aria-label="Zoom">
          {items().map((item, at) =>
            item === "rule" ? (
              // biome-ignore lint/suspicious/noArrayIndexKey: the list is fixed
              <hr key={at} />
            ) : (
              <button
                key={item.label}
                type="button"
                role="menuitem"
                disabled={item.disabled}
                onClick={() => {
                  item.run()
                  shut()
                }}
              >
                <span>{item.label}</span>
                {item.keys && <kbd>{item.keys}</kbd>}
              </button>
            ),
          )}
        </div>
      )}
    </div>
  )
}
