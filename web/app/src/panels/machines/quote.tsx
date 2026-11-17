import type { Alternative, RentalSku, RunQuote } from "../../api/calcifer"
import { usd } from "../format"

/** A new rental's offer as its button states it, `1× CPU at $0.83/h`: pressing that button is agreeing to it. */
export const offered = (quote: RunQuote) => `${quote.gpus ?? 1}× ${quote.sku.toUpperCase()} at ${usd(quote.quote.usd_micros_per_hour)}/h`

/** Whether the Hub stated a price that can be agreed to. */
export const agreeable = (quote: RunQuote | null | undefined) => Boolean(quote?.quote.agreed)

/** What a quote says beside its price: why the Hub picked it, what it changed, a price it didn't confirm, the machines it would rent
 *  instead (`onOther` picks one), and what renting costs. The run form's and a render's new rental alike (cl-1066); `why` false where
 *  what's around it already says why (Slow here). */
export function QuoteNotes({ quote, onOther, why = true }: { quote: RunQuote; onOther?: (other: Alternative) => void; why?: boolean }) {
  return (
    <div className="quoted">
      {why && quote.why && <p className="muted">{quote.why}</p>}
      {[...(quote.quote.warnings ?? []), ...(quote.warnings ?? [])].map((warning) => (
        <p className="warn" key={warning}>
          {warning}
        </p>
      ))}
      {!agreeable(quote) && <p className="warn">Cozy Creator didn't confirm this price, so you can't rent it from here.</p>}
      {onOther &&
        quote.alternatives?.map((other) => (
          <button type="button" className="more alt" key={`${other.sku}:${other.gpus}`} onClick={() => onOther(other)}>
            or {other.gpus}× {other.sku.toUpperCase()}
            {other.label ? ` (${other.label})` : ""}: {other.time.toFixed(2)}× the time at {usd(other.usd_micros_per_hour)}/h
          </button>
        ))}
      <p className="muted">You pay for it until you end it. It also ends by itself after 15 minutes idle.</p>
    </div>
  )
}

/** A select's new rentals, by `new[:sku]`: the Hub's pick, then each product it rents now, by its price an hour. */
export function NewOptions({ skus }: { skus: RentalSku[] | undefined }) {
  return (
    <>
      <option value="new">A new rental (the Hub picks)</option>
      {skus?.map((sku) => (
        <option key={sku.name} value={`new:${sku.name.toLowerCase()}`}>
          A new {sku.name} rental, {usd(sku.widths[0]?.price_usd_micros_per_hour ?? 0)}/h
        </option>
      ))}
    </>
  )
}
