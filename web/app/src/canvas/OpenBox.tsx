import { type DragEvent, useEffect, useRef } from "react"
import { useSignal } from "../ui/signal"
import { OpenCard } from "./Card"
import { useCanvasStore, useElement } from "./context"
import { DRAGGED_FILE, DRAGGED_NAME } from "./drag"
import { Thumb } from "./Media"
import { shownOf } from "./shown"

const GROW_MS = 180

const typing = (target: EventTarget | null) => target instanceof HTMLElement && (target.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName))

/** An Edit's clips to choose from, while the canvas is behind: a click puts one in, and one can be dragged to a place. A prompt reads pictures by name. */
function Pictures({ name, room }: { name: string; room: () => HTMLElement | null }) {
  const store = useCanvasStore()
  useSignal(store.names)
  const held = store.namesLike("", name).flatMap((other) => {
    const view = store.get(other)
    const shown = view && shownOf(view)
    return shown && !shown.absent && (shown.field === "image" || shown.field === "video") ? [{ other, sha256: shown.sha256 }] : []
  })
  if (!held.length) return null
  return (
    <div className="pictures" aria-label="The project's pictures">
      {held.map(({ other, sha256 }) => (
        <button
          key={other}
          type="button"
          className="picture nodrag"
          title={other}
          aria-label={`Put ${other} in`}
          draggable
          onDragStart={(event) => event.dataTransfer.setData(DRAGGED_NAME, other)}
          onClick={() => {
            const place = room()?.querySelector("[data-drop-target]")
            const target = place && store.dropTargets.get(place)
            if (target) target.drop([other], 0, 0)
            else store.takeInto(name, [other])
          }}
        >
          <Thumb sha256={sha256} cardWidth={120} />
        </button>
      ))}
    </div>
  )
}

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
  const carried = (event: DragEvent) => [DRAGGED_NAME, DRAGGED_FILE, "Files"].some((type) => event.dataTransfer.types.includes(type))
  const dropped = (event: DragEvent) => {
    if (!carried(event)) return
    event.preventDefault()
    event.stopPropagation()
    // A take dragged out onto the canvas behind is its own box there.
    const take = event.dataTransfer.getData(DRAGGED_FILE)
    if (take) return addTake(take, event.clientX, event.clientY)
    const at = (event.target as Element).closest("[data-drop-target]") ?? box.current?.querySelector("[data-drop-target]")
    const target = at && store.dropTargets.get(at)
    const put = (other: string) => (target ? target.drop([other], event.clientX, event.clientY) : store.takeInto(name, [other]))
    const carriedName = event.dataTransfer.getData(DRAGGED_NAME)
    if (carriedName) put(carriedName)
    else addFiles([...event.dataTransfer.files], (other) => store.takeInto(name, [other]))
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
        {view.asset?.timeline && <Pictures name={name} room={() => box.current} />}
      </div>
    </div>
  )
}
