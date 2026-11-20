import { useQuery } from "@tanstack/react-query"
import { type ReactNode, useRef, useState } from "react"
import { Link } from "react-router-dom"
import { machinesQuery, rentalSkusQuery } from "../../api/queries"
import { useConnection } from "../../app/Connection"
import { useTo } from "../../app/Shell"
import type { Agreed, RunQuote } from "../../api/calcifer"
import { Failure, useAsk, useSignedIn } from "../common"
import { usd } from "../format"
import { lost } from "./order"
import { NewOptions, offered, QuoteNotes } from "./quote"
import { called, gpusWords } from "./words"

export interface Placed {
  /** Absent: Calcifer says where. */
  on?: string[]
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
  const { client, status } = useConnection()
  const to = useTo()
  const { data: list } = useQuery(machinesQuery(client))
  const signedIn = useSignedIn() && !list?.signed_out?.length
  const { data: skus } = useQuery({ ...rentalSkusQuery(client), enabled: signedIn })
  const machines = list?.machines.filter((machine) => machine.state === "ready" || machine.state === "booting" || (machine.kind === "local" && machine.state === "stopped")) ?? []

  // Unchosen, none is sent: Calcifer says where it goes.
  const [on, setChosen] = useState("")
  const [gpus, setGpus] = useState<number | undefined>()

  const placed = (agreed?: Agreed): Body & Placed => ({ ...body, ...(on ? { on: [on] } : {}), ...(gpus ? { gpus } : {}), ...(agreed ? { agreed } : {}) })
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
  const priced = stated && offered(stated)
  const busy = pricing.busy || sending.busy
  const blocked = disabled || busy || (isNew(on) && Boolean(stated) && !stated?.quote.agreed)

  return (
    <div className="onmachine">
      <div className="row center">
        <label className="check">
          Machine
          <select
            className="field"
            aria-label="Machine"
            value={on}
            onChange={(event) => {
              setChosen(event.target.value)
              setGpus(undefined)
            }}
          >
            <option value="">Cozy picks</option>
            {status.local_usable && <option value="local">This computer</option>}
            {machines
              .filter((machine) => machine.kind !== "local")
              .map((machine) => (
                <option key={machine.name} value={machine.name}>
                  {[called(machine), gpusWords(machine), machine.rental && `${usd(machine.rental.usd_micros_per_hour)}/h`, machine.state === "booting" && "starting up"].filter(Boolean).join(" · ")}
                </option>
              ))}
            {machines.length > 1 && <option value="any">Any of these</option>}
            {signedIn && <NewOptions skus={skus} />}
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
      {!signedIn && !client.transport.channel && (
        <p className="muted">
          <Link className="more" to={to("account")}>
            Sign in
          </Link>{" "}
          to rent a machine for this.
        </p>
      )}
      {stated && (
        <QuoteNotes
          quote={stated}
          onOther={(other) => {
            setChosen(`new:${other.sku.toLowerCase()}`)
            setGpus(other.gpus > 1 ? other.gpus : undefined)
          }}
        />
      )}
      <Failure error={pricing.error} lead="Couldn't get a price" />
      <Failure error={sending.error} lead={isNew(on) && lost(sending.error) ? "No answer came back, so it may have gone through. Trying again won't send it twice" : undefined} />
    </div>
  )
}
