import { useEffect, useRef } from "react"
import { Icon } from "../ui/Icon"
import { useSignal } from "../ui/signal"
import type { ElementView } from "./CanvasStore"
import { useCanvasStore } from "./context"
import { CloseInPlace, RenderOnNew, useClosesInPlace } from "./RenderOnNew"

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
 *  (cl-1004). Asked of Calcifer while it shows, and again once what's typed settles, a take lands or the machines change. Before
 *  them, "Ready on kamitsure" once the machine a render of it would go to has its tool and models (cl-1104): Calcifer's guess, which
 *  goes when the guess does. */
export function RenderTime({ name, view }: { name: string; view: ElementView }) {
  const store = useCanvasStore()
  const estimates = useSignal(store.estimates)
  const machines = useSignal(store.machineSet)
  useSignal(store.ahead)
  const asked = JSON.stringify([view.asset?.tool, view.asset?.fields, view.asset?.prefixes?.length])
  // biome-ignore lint/correctness/useExhaustiveDependencies: asked again when what it renders, or where, changes
  useEffect(() => {
    const later = setTimeout(() => void store.estimate(name), SETTLE_MS)
    return () => clearTimeout(later)
  }, [store, name, asked, machines])
  const estimate = estimates.get(store.resolveName(name))
  const prepared = store.preparing(name)
  const ready = prepared?.guess && prepared.ready ? (prepared.machine === "local" ? "this computer" : prepared.machine) : undefined
  if (!estimate && !ready) return null
  const machine = estimate?.machine === "local" ? "this computer" : estimate?.machine
  const where = estimate?.card ? `${machine}'s ${estimate.card}` : machine
  return (
    <>
      {ready && (
        <span className="ready-on" role="note" aria-label="Ready" title={`This box's tool and models are already on ${ready}`}>
          Ready on {ready}
        </span>
      )}
      {estimate && estimate.run_ms >= SHOWN_MS && (
        <span className="render-time" role="note" aria-label="Render time" title={`Typical time on ${where}, once it starts`}>
          {about(estimate.run_ms)}
        </span>
      )}
      {estimate?.faster && (
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

/** Slow here, opened under the box's fields: how long it takes where it would go against a rented GPU, then Render on a new one of that GPU,
 *  its price asked for this render and agreed in place (cl-1004, cl-1066). Closes on its ✕, Escape, or a click whose press began outside it. */
export function SlowNotice({ name, view }: { name: string; view: ElementView }) {
  const store = useCanvasStore()
  const estimates = useSignal(store.estimates)
  const faster = estimates.get(store.resolveName(name))?.faster
  const own = useRef<HTMLDivElement>(null)
  const open = Boolean(view.slowOpen && faster)
  const close = () => store.patch(store.resolveName(name), { slowOpen: undefined })
  useClosesInPlace(open, (element) => Boolean(own.current?.contains(element) || element.closest(".slow-here")?.getAttribute("data-box") === name), close)
  if (!open || !faster) return null
  return (
    <div ref={own} className="status slow-notice canvas-control" role="group" aria-label="Slow here">
      <p>{faster.line}</p>
      <p>{faster.rent.line}</p>
      <RenderOnNew name={name} sku={faster.rent.sku} onDone={close} />
      <CloseInPlace onClose={close} />
    </div>
  )
}
