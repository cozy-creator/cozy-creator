import { useEffect, useRef } from "react"
import { Icon } from "../ui/Icon"
import { pressBeganOutside } from "../ui/press"
import { useSignal } from "../ui/signal"
import type { ElementView } from "./CanvasStore"
import { useCanvasStore } from "./context"

/** "an RTX 4090", "a B200": by how the card's first letter is said. */
const article = (card: string) => (/^[aefhilmnorsx]/i.test(card) ? "an" : "a")

/** A render this long or longer shows how long it takes; a shorter one needs no estimate (Paul, 11-15). */
const SHOWN_MS = 15_000
/** What's typed settles this long before the estimate is asked again. */
const SETTLE_MS = 600

/** `about 40 s`, `about 6 min`, `about 1.5 h`: as Calcifer says a time. */
export function about(ms: number): string {
  const seconds = ms / 1000
  if (seconds < 90) return `about ${Math.max(1, Math.round(seconds))} s`
  if (seconds < 90 * 60) return `about ${Math.round(seconds / 60)} min`
  return `about ${(seconds / 3600).toFixed(1)} h`
}

/** Beside Render: how long a render of it runs where it would go now, from 15 s, and Slow here when a rented GPU runs it far faster
 *  (cl-1004). Asked of Calcifer while it shows, and again once what's typed settles, a take lands or the machines change. */
export function RenderTime({ name, view }: { name: string; view: ElementView }) {
  const store = useCanvasStore()
  const estimates = useSignal(store.estimates)
  const machines = useSignal(store.machineSet)
  const asked = JSON.stringify([view.asset?.tool, view.asset?.fields, view.asset?.prefixes?.length])
  // biome-ignore lint/correctness/useExhaustiveDependencies: asked again when what it renders, or where, changes
  useEffect(() => {
    const later = setTimeout(() => void store.estimate(name), SETTLE_MS)
    return () => clearTimeout(later)
  }, [store, name, asked, machines])
  const estimate = estimates.get(store.resolveName(name))
  if (!estimate) return null
  const machine = estimate.machine === "local" ? "this computer" : estimate.machine
  const where = estimate.card ? `${machine}'s ${estimate.card}` : machine
  return (
    <>
      {estimate.run_ms >= SHOWN_MS && (
        <span className="render-time" role="note" aria-label="Render time" title={`Typical time on ${where}, once it starts`}>
          {about(estimate.run_ms)}
        </span>
      )}
      {estimate.faster && (
        <button
          type="button"
          className={`slow-here canvas-control ${view.slowOpen ? "on" : ""}`}
          data-box={name}
          aria-expanded={Boolean(view.slowOpen)}
          onClick={() => store.patch(store.resolveName(name), { slowOpen: !view.slowOpen || undefined })}
        >
          <Icon name="warning" />
          Slow here
        </button>
      )}
    </>
  )
}

/** Slow here, opened under the box's fields: how long it takes where it would go against a rented GPU, then Rent it, which opens Machines
 *  with that GPU chosen (cl-1004). Closes on its ✕, Escape, or a click whose press began outside it. */
export function SlowNotice({ name, view }: { name: string; view: ElementView }) {
  const store = useCanvasStore()
  const estimates = useSignal(store.estimates)
  const faster = estimates.get(store.resolveName(name))?.faster
  const own = useRef<HTMLDivElement>(null)
  const open = Boolean(view.slowOpen && faster)
  useEffect(() => {
    if (!open) return
    const close = () => store.patch(store.resolveName(name), { slowOpen: undefined })
    const inside = (element: Element) => Boolean(own.current?.contains(element) || element.closest(".slow-here")?.getAttribute("data-box") === name)
    const click = (event: MouseEvent) => {
      if (event.button === 0 && event.target instanceof Element && !inside(event.target) && pressBeganOutside(inside)) close()
    }
    const key = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return
      event.preventDefault()
      event.stopPropagation()
      close()
    }
    document.addEventListener("click", click, true)
    window.addEventListener("keydown", key, true)
    return () => {
      document.removeEventListener("click", click, true)
      window.removeEventListener("keydown", key, true)
    }
  }, [store, name, open])
  if (!open || !faster) return null
  const card = faster.rent.card
  const close = () => store.patch(store.resolveName(name), { slowOpen: undefined })
  return (
    <div ref={own} className="status slow-notice canvas-control" role="group" aria-label="Slow here">
      <p>{faster.line}</p>
      <p>{faster.rent.line}</p>
      <div className="row">
        <button
          type="button"
          className="tb primary canvas-control"
          onClick={() => {
            close()
            store.rent(faster.rent)
          }}
        >
          Rent {article(card)} {card}
        </button>
        <button type="button" className="tb canvas-control slow-close" aria-label="Close" title="Close" onClick={close}>
          <Icon name="close" />
        </button>
      </div>
    </div>
  )
}
