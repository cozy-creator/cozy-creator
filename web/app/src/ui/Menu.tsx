import { type MouseEvent, type ReactNode, useCallback, useEffect, useRef, useState } from "react"
import { createPortal } from "react-dom"

/** A right click's menu at the pointer, over everything: a click off it or Esc shuts it. The keys go to its first item, the arrows,
 *  Home and End move between its items, and shut, it gives the keys back to what had them when it opened (cl-1052). */
export function Menu({ x, y, label, close, children }: { x: number; y: number; label: string; close: () => void; children: ReactNode }) {
  const shell = useRef<HTMLDivElement>(null)
  const items = () => [...(shell.current?.querySelectorAll<HTMLElement>("[role=menuitem]:not(:disabled)") ?? [])]
  useEffect(() => {
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null
    items()[0]?.focus({ preventScroll: true })
    // Shut, the keys go back where they were, unless something else has taken them meanwhile (an item that opens a field, a click elsewhere).
    return () => {
      if ((!document.activeElement || document.activeElement === document.body) && opener?.isConnected) opener.focus({ preventScroll: true })
    }
  }, [])
  useEffect(() => {
    const away = (event: Event) => {
      if (!shell.current?.contains(event.target as globalThis.Node)) close()
    }
    const key = (event: KeyboardEvent) => {
      if (event.key === "Escape") return close()
      if (!shell.current?.contains(document.activeElement)) return
      const rows = items(), at = rows.indexOf(document.activeElement as HTMLElement)
      const to = ({ ArrowDown: at + 1, ArrowUp: at - 1, Home: 0, End: rows.length - 1 } as Record<string, number>)[event.key]
      if (to === undefined || !rows.length) return
      event.preventDefault()
      rows[(to + rows.length) % rows.length].focus({ preventScroll: true })
    }
    window.addEventListener("pointerdown", away)
    window.addEventListener("keydown", key)
    return () => {
      window.removeEventListener("pointerdown", away)
      window.removeEventListener("keydown", key)
    }
  }, [close])
  const left = Math.min(x, window.innerWidth - 300)
  const top = Math.min(y, window.innerHeight - 340)
  return createPortal(
    <div ref={shell} className="asset-menu" role="menu" aria-label={label} style={{ left, top }}>
      {children}
    </div>,
    document.body,
  )
}

/** Where a right click opened a menu held here, if one is open. */
export function useMenuAt() {
  const [at, setAt] = useState<{ x: number; y: number } | null>(null)
  const open = (event: MouseEvent) => {
    event.preventDefault()
    event.stopPropagation()
    setAt({ x: event.clientX, y: event.clientY })
  }
  const close = useCallback(() => setAt(null), [])
  return { at, open, close }
}
