import { useQuery } from "@tanstack/react-query"
import { type ReactNode, useRef, useState } from "react"
import { Link } from "react-router-dom"
import { machinesQuery, rentalSkusQuery } from "../../api/queries"
import { useConnection } from "../../app/Connection"
import { useTo } from "../../app/Shell"
import type { Agreed, RunQuote } from "../../types"
import { Failure, useAsk, useSignedIn } from "../common"
import { usd } from "../format"
import { lost } from "./order"
import { gpusWords } from "./words"

export interface Placed {
  on: string
  agreed?: Agreed
  gpus?: number
}

const isNew = (on: string) => on === "new" || on.startsWith("new:")

/**
 * Where an ask goes, and its button. A `new` machine costs money, so its button is a stated price from the ask's quote: changing anything takes
 * the price away, and it never rents by itself. One ask is one key: the same ask sent again after a lost answer can't be done twice.
 */
export function OnMachine<Body extends object, Answer>({
  body,
  label,
  disabled = false,
  quote,
  ask,
  onDone,
  children,
}: {
  body: Body
  label: string
  disabled?: boolean
  quote: (ask: Body & Placed) => Promise<RunQuote | null>
  ask: (ask: Body & Placed, key: string) => Promise<Answer>
  onDone: (answer: Answer) => void
  children?: ReactNode
}) {
  const { client, status, remote } = useConnection()
  const to = useTo()
  const { data: list } = useQuery(machinesQuery(client))
  const signedIn = useSignedIn() && !list?.signed_out?.length
  const { data: skus } = useQuery({ ...rentalSkusQuery(client), enabled: signedIn })
  const machines = list?.machines.filter((machine) => machine.state === "ready" || machine.state === "booting" || (machine.kind === "local" && machine.state === "stopped")) ?? []

  const fallback = status.local_usable ? "local" : (machines.find((machine) => machine.state === "ready")?.name ?? machines[0]?.name ?? (signedIn ? "new" : ""))
  const [chosen, setChosen] = useState<string | null>(null)
  const [gpus, setGpus] = useState<number | undefined>()
  const on = chosen ?? fallback

  const placed = (agreed?: Agreed): Body & Placed => ({ ...body, on, ...(gpus ? { gpus } : {}), ...(agreed ? { agreed } : {}) })
  const asked = JSON.stringify(placed())
  const pricing = useAsk()
  const sending = useAsk()
  const [quoted, setQuoted] = useState<{ asked: string; answer: RunQuote | null } | null>(null)
  const current = quoted?.asked === asked ? quoted : null
  const sent = useRef<{ asked: string; key: string } | null>(null)

  const price = () =>
    void pricing.run(async () => {
      const answer = await quote(placed())
      setQuoted({ asked, answer })
    })

  const send = () => {
    const stated = current?.answer?.quote
    if (isNew(on) && current?.answer && !stated?.agreed) return
    if (sent.current?.asked !== asked) sent.current = { asked, key: `ask-${crypto.randomUUID()}` }
    const { key } = sent.current
    const work = () => ask(placed(stated?.agreed), key)
    void sending.run(async () => {
      const answer = await work()
      sent.current = null
      setQuoted(null)
      onDone(answer)
    })
  }

  const needsPrice = isNew(on) && !current
  const stated = current?.answer
  const priced = stated && `${stated.gpus ?? 1}× ${stated.sku.toUpperCase()} at ${usd(stated.quote.gpu_usd_micros_per_hour)}/h`
  const busy = pricing.busy || sending.busy
  const blocked = disabled || busy || !on || (isNew(on) && Boolean(stated) && !stated?.quote.agreed)

  return (
    <div className="onmachine">
      <div className="row center">
        <label className="check">
          On
          <select
            className="field"
            aria-label="Which machine"
            value={on}
            onChange={(event) => {
              setChosen(event.target.value)
              setGpus(undefined)
            }}
          >
            {!on && <option value="">No machine</option>}
            {status.local_usable && <option value="local">this computer</option>}
            {machines
              .filter((machine) => machine.kind !== "local")
              .map((machine) => (
                <option key={machine.name} value={machine.name}>
                  {[machine.name, gpusWords(machine), machine.rental && `${usd(machine.rental.usd_micros_per_hour)}/h`, machine.state === "booting" && "booting"].filter(Boolean).join(" · ")}
                </option>
              ))}
            {machines.length > 1 && <option value="any">any of these</option>}
            {signedIn && <option value="new">a new machine, the Hub's pick</option>}
            {signedIn &&
              skus?.map((sku) => (
                <option key={sku.name} value={`new:${sku.name.toLowerCase()}`}>
                  a new {sku.name}, {usd(sku.widths[0]?.price_usd_micros_per_hour ?? 0)}/h
                </option>
              ))}
          </select>
        </label>
        <span className="sp" />
        {children}
        {needsPrice ? (
          <button type="button" className="tb" disabled={disabled || busy} onClick={price}>
            Get the price
          </button>
        ) : (
          <button type="button" className="tb primary" disabled={blocked} onClick={send}>
            {priced ? `${label} on a new ${priced}` : `${label}${on && !isNew(on) ? (on === "local" ? " on this computer" : on === "any" ? "" : ` on ${on}`) : ""}`}
          </button>
        )}
      </div>
      {!signedIn && !remote && !client.transport.channel && (
        <p className="muted">
          <Link className="more" to={to("account")}>
            Sign in
          </Link>{" "}
          to rent a machine for this.
        </p>
      )}
      {stated && (
        <div className="quoted">
          {stated.why && <p className="muted">{stated.why}</p>}
          {[...(stated.quote.warnings ?? []), ...(stated.warnings ?? [])].map((warning) => (
            <p className="warn" key={warning}>
              {warning}
            </p>
          ))}
          {!stated.quote.agreed && <p className="warn">This Calcifer didn't say what's agreed at this price, so it can't be ordered from here.</p>}
          {stated.alternatives?.map((other) => (
            <button
              type="button"
              className="more alt"
              key={`${other.sku}:${other.gpus}`}
              onClick={() => {
                setChosen(`new:${other.sku.toLowerCase()}`)
                setGpus(other.gpus > 1 ? other.gpus : undefined)
              }}
            >
              or {other.gpus}× {other.sku.toUpperCase()}
              {other.label ? ` (${other.label})` : ""}: {other.time.toFixed(2)}× the time, {usd(other.usd_micros_per_hour)}/h
            </button>
          ))}
          <p className="muted">It bills until you end it and the Hub affirms it gone, or after 15 minutes idle.</p>
        </div>
      )}
      <Failure error={pricing.error} lead="No price" />
      <Failure error={sending.error} lead={isNew(on) && lost(sending.error) ? "It may have been asked, since no answer came. Asking again is the same ask" : undefined} />
    </div>
  )
}
