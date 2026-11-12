import { type MouseEvent, type ReactNode, useCallback, useEffect, useRef, useState } from "react"
import { createPortal } from "react-dom"

/** A right click's menu at the pointer, over everything: a click off it or Esc shuts it. */
export function Menu({ x, y, label, close, children }: { x: number; y: number; label: string; close: () => void; children: ReactNode }) {
  const shell = useRef<HTMLDivElement>(null)
  useEffect(() => {
    const away = (event: Event) => {
      if (!shell.current?.contains(event.target as globalThis.Node)) close()
    }
    const key = (event: KeyboardEvent) => {
      if (event.key === "Escape") close()
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
