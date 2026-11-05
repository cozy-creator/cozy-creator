import { type DragEvent, useEffect, useRef } from "react"
import { OpenCard } from "./Card"
import { useCanvasStore, useElement } from "./context"
import { DRAGGED_FILE } from "./drag"

const GROW_MS = 180

const typing = (target: EventTarget | null) => target instanceof HTMLElement && (target.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName))

/** A box opened: the canvas dims behind it, it grows from where it sits, and Esc, a click outside or its X puts it back. What's typed in it is saved as it goes. */
export function OpenBox({ name, close, addFiles, addTake }: { name: string; close: () => void; addFiles: (files: File[], then: (name: string) => void) => void; addTake: (file: string, x: number, y: number) => void }) {
  const store = useCanvasStore()
  const view = useElement(name)
  const box = useRef<HTMLDivElement>(null)
  const there = Boolean(view)
  // Whichever way it shuts, what's typed stands.
  useEffect(() => () => void store.save(name), [store, name])
  // Deleted from under it, by anyone: there's nothing to look at.
  useEffect(() => {
    if (!there && store.hasRead) close()
  }, [there, store, close])
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
      // A menu or a name field takes its own Esc first.
      if (event.key === "Escape" && !store.menu.get() && store.get(name)?.draft === undefined) close()
      const take = store.lookingAt(name)
      if ((event.key === "Delete" || event.key === "Backspace") && !typing(event.target) && take !== undefined) store.discardTake(name, take)
    }
    window.addEventListener("keydown", key)
    return () => window.removeEventListener("keydown", key)
  }, [close, name, store])
  if (!view) return null
  const carried = (event: DragEvent) => [DRAGGED_FILE, "Files"].some((type) => event.dataTransfer.types.includes(type))
  const dropped = (event: DragEvent) => {
    if (!carried(event)) return
    event.preventDefault()
    event.stopPropagation()
    // A take dragged out onto the canvas behind is its own box there.
    const take = event.dataTransfer.getData(DRAGGED_FILE)
    if (take) return addTake(take, event.clientX, event.clientY)
    addFiles([...event.dataTransfer.files], (other) => store.takeInto(name, [other]))
  }
  return (
    <div
      className="open-back"
      onPointerDown={(event) => event.target === event.currentTarget && close()}
      onDragOver={(event) => {
        if (!carried(event)) return
        event.preventDefault()
        event.dataTransfer.dropEffect = "copy"
      }}
      onDrop={dropped}
    >
      <div ref={box} className="open-box" role="dialog" aria-label={view.name}>
        <OpenCard name={name} close={close} />
      </div>
    </div>
  )
}
