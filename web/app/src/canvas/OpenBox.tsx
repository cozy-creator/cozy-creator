import { useEffect, useRef } from "react"
import { OpenCard } from "./Card"
import { useElement } from "./context"

const GROW_MS = 180

/** A box opened: the canvas dims behind it, it grows from where it sits, and Esc, a click outside or its X puts it back. What's typed in it is saved as it goes. */
export function OpenBox({ name, close }: { name: string; close: () => void }) {
  const view = useElement(name)
  const box = useRef<HTMLDivElement>(null)
  const there = Boolean(view)
  // biome-ignore lint/correctness/useExhaustiveDependencies: once, when it's there to grow
  useEffect(() => {
    const to = box.current?.getBoundingClientRect()
    const from = document.querySelector(`.react-flow__node .card[data-asset="${CSS.escape(view?.name ?? name)}"]`)?.getBoundingClientRect()
    if (!to || !from || matchMedia("(prefers-reduced-motion: reduce)").matches) return
    const scale = from.width / to.width
    const [x, y] = [from.left + from.width / 2 - (to.left + to.width / 2), from.top + from.height / 2 - (to.top + to.height / 2)]
    box.current?.animate([{ transform: `translate(${x}px, ${y}px) scale(${scale})`, opacity: 0.6 }, { transform: "none", opacity: 1 }], { duration: GROW_MS, easing: "cubic-bezier(0.2, 0.9, 0.3, 1)" })
  }, [there])
  useEffect(() => {
    const key = (event: KeyboardEvent) => {
      if (event.key === "Escape") close()
    }
    window.addEventListener("keydown", key)
    return () => window.removeEventListener("keydown", key)
  }, [close])
  if (!view) return null
  return (
    <div className="open-back" onPointerDown={(event) => event.target === event.currentTarget && close()}>
      <div ref={box} className="open-box" role="dialog" aria-label={view.name}>
        <OpenCard name={name} close={close} />
      </div>
    </div>
  )
}
