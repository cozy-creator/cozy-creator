import { useQuery } from "@tanstack/react-query"
import { useEffect, useRef, useState } from "react"
import type { RunQuote } from "../api/calcifer"
import { rentalSkusQuery } from "../api/queries"
import { useConnection } from "../app/Connection"
import { Failure } from "../panels/common"
import { agreeable, NewOptions, offered, QuoteNotes } from "../panels/machines/quote"
import { Icon } from "../ui/Icon"
import { pressBeganOutside } from "../ui/press"
import { useCanvasStore } from "./context"

/** A notice's ✕, top right. */
export function CloseInPlace({ onClose }: { onClose: () => void }) {
  return (
    <button type="button" className="tb canvas-control in-place-close" aria-label="Close" title="Close" onClick={onClose}>
      <Icon name="close" />
    </button>
  )
}

/** A surface opened in place closes on Escape, or on a click whose press began outside it and its opener (`inside`): a press that begins
 *  inside and ends outside leaves it open (Paul, 11-12). Slow here's and Waits for a machine's alike. */
export function useClosesInPlace(open: boolean, inside: (element: Element) => boolean, close: () => void) {
  const said = useRef({ inside, close })
  said.current = { inside, close }
  useEffect(() => {
    if (!open) return
    const click = (event: MouseEvent) => {
      const { inside, close } = said.current
      if (event.button === 0 && event.target instanceof Element && !inside(event.target) && pressBeganOutside(inside)) close()
    }
    const key = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return
      event.preventDefault()
      event.stopPropagation()
      said.current.close()
    }
    document.addEventListener("click", click, true)
    window.addEventListener("keydown", key, true)
    return () => {
      document.removeEventListener("click", click, true)
      window.removeEventListener("keydown", key, true)
    }
  }, [open])
}

/**
 * Render `name` on a new rental, in place where it's offered (Slow here, Waits for a machine: cl-1066). Its price is Calcifer's for exactly
 * this render (`render-quote`: one rental for all it sends), said on its button as the run form says one, and pressing it agrees to that
 * price and renders there. `sku`: the product offered, Slow here's card; none, the run form's choice of new rentals. Another product takes
 * the price away until its own comes. Never a pop-up: it sits in the notice it was opened in, whose ✕ closes it; agreed, `onDone`.
 */
export function RenderOnNew({ name, sku, onDone }: { name: string; sku?: string; onDone: () => void }) {
  const store = useCanvasStore()
  const { client } = useConnection()
  const [on, setOn] = useState(sku ? `new:${sku.toLowerCase()}` : "new")
  const { data: skus } = useQuery({ ...rentalSkusQuery(client), enabled: !sku })
  const [quoted, setQuoted] = useState<{ on: string; answer: RunQuote | null } | null>(null)
  const [failed, setFailed] = useState<{ on: string; error: unknown } | null>(null)
  // Asked as it opens and again for another product; an answer for one no longer chosen is dropped.
  useEffect(() => {
    let live = true
    store.renderQuote(name, on).then(
      (answer) => live && setQuoted({ on, answer }),
      (error) => live && setFailed({ on, error }),
    )
    return () => {
      live = false
    }
  }, [store, name, on])
  const current = quoted?.on === on ? quoted : null
  const answer = current?.answer
  const error = failed?.on === on && !current ? failed.error : null
  return (
    <div className="on-new">
      <div className="row">
        {!sku && (
          <select className="field" aria-label="New rental" value={on} onChange={(event) => setOn(event.target.value)}>
            <NewOptions skus={skus} />
          </select>
        )}
        {current && !answer ? (
          <span className="muted">Nothing it renders needs a machine.</span>
        ) : (
          <button
            type="button"
            className="tb primary canvas-control"
            disabled={!answer || !agreeable(answer)}
            onClick={() => {
              if (!answer?.quote.agreed) return
              store.run(name, { on, agreed: answer.quote.agreed })
              onDone()
            }}
          >
            {answer ? `Render on a new ${offered(answer)}` : error ? "No price" : "Getting the price…"}
          </button>
        )}
      </div>
      {answer && <QuoteNotes quote={answer} why={false} />}
      <Failure error={error} lead="Couldn't get a price" />
    </div>
  )
}
