import { useQuery, useQueryClient } from "@tanstack/react-query"
import { Fragment, useEffect, useRef, useState } from "react"
import { Link, useNavigate, useSearchParams } from "react-router-dom"
import { rentalSkusQuery } from "../../api/queries"
import { useConnection } from "../../app/Connection"
import { useTo } from "../../app/Shell"
import type { RentalSku } from "../../types"
import { say } from "../../ui/notes"
import { useSignal } from "../../ui/signal"
import { Failure, Section } from "../common"
import { usd } from "../format"
import { dismiss, order, place, quote } from "./order"
import { stableName } from "./words"

const fieldKey = "cozy.ssh-key"

function readKey(): string {
  try {
    return localStorage.getItem(fieldKey) ?? ""
  } catch {
    return ""
  }
}

function saveKey(key: string) {
  try {
    if (key) localStorage.setItem(fieldKey, key)
    else localStorage.removeItem(fieldKey)
  } catch {
    // A convenience only.
  }
}

/** Its price is what we charge for its GPUs an hour. Nothing about its disk or traffic is a renter's. */
function Sku({ sku, disabled, chosen, onPick }: { sku: RentalSku; disabled: boolean; chosen?: number; onPick: (sku: RentalSku, gpus: number) => void }) {
  const [gpus, setGpus] = useState(chosen ?? sku.widths[0]?.accelerator_count ?? 1)
  const row = useRef<HTMLLIElement>(null)
  useEffect(() => {
    if (chosen !== undefined) row.current?.scrollIntoView({ block: "nearest" })
  }, [chosen])
  const width = sku.widths.find((candidate) => candidate.accelerator_count === gpus) ?? sku.widths[0]
  if (!width) return null
  const cpu = !sku.vram_gb
  return (
    <li ref={row} className={`sku ${chosen !== undefined ? "chosen" : ""}`} aria-current={chosen !== undefined || undefined}>
      <b>{sku.name}</b>
      <small>
        {cpu ? `${width.vcpu_count ?? "?"} vCPU, no GPU` : `${sku.accelerator_model} · ${sku.vram_gb} GB`}
      </small>
      <span className="sp" />
      {sku.widths.length > 1 && (
        <select className="field" aria-label={`${sku.name}: GPUs`} value={gpus} onChange={(event) => setGpus(Number(event.target.value))}>
          {sku.widths.map((choice) => (
            <option key={choice.accelerator_count} value={choice.accelerator_count}>
              {choice.accelerator_count}×
            </option>
          ))}
        </select>
      )}
      <span className="price">{usd(width.price_usd_micros_per_hour)}/h</span>
      <button type="button" className="tb" disabled={disabled} onClick={() => onPick(sku, gpus)}>
        Rent
      </button>
    </li>
  )
}

/** What the order is, and the one click on its stated price. It never rents by itself, and a dearer price rents nothing. */
function Order() {
  const { client } = useConnection()
  const queries = useQueryClient()
  const navigate = useNavigate()
  const to = useTo()
  const now = useSignal(order)
  const here = useRef<HTMLDivElement>(null)
  const shown = now.state !== "idle"
  useEffect(() => {
    if (shown) here.current?.scrollIntoView({ block: "nearest" })
  }, [shown])

  const go = async () => {
    const rented = await place(client)
    if (!rented) return
    if (rented.replay) say("This order was placed already: following it.")
    void queries.invalidateQueries({ queryKey: ["machines"] })
    navigate(to(`machines/${encodeURIComponent(stableName(rented.machine))}`))
  }

  if (now.state === "idle") return null
  const line = (held: { sku: string; gpus?: number }, price: number) => `${held.gpus ?? 1}× ${held.sku.toUpperCase()} at ${usd(price)}/h`
  return (
    <div className="order" aria-live="polite" ref={here}>
      {now.state === "quoting" && <p className="muted">Asking the Hub its price for {now.order.gpus ?? 1}× {now.order.sku}…</p>}
      {now.state === "quoted" && (
        <>
          {now.quote.warnings?.map((warning) => (
            <p className="warn" key={warning}>
              {warning}
            </p>
          ))}
          <p>It bills until you end it, or after 15 minutes idle.</p>
          {!now.quote.agreed && <p className="warn">This Calcifer didn't say what's agreed at this price, so it can't be ordered from here.</p>}
          <div className="row">
            <button type="button" className="tb primary" disabled={!now.quote.agreed} onClick={() => void go()}>
              Rent {line(now.order, now.quote.usd_micros_per_hour)}
            </button>
            <button type="button" className="tb" onClick={dismiss}>
              Not now
            </button>
          </div>
        </>
      )}
      {now.state === "placing" && <p>Ordering {line(now.order, now.quote.usd_micros_per_hour)}…</p>}
      {now.state === "uncertain" && (
        <>
          <Failure error={now.error} lead="The order may stand, since no answer came" />
          <p>If it does, it's in the list above. Asking again is the same order, and can't place a second.</p>
          <div className="row">
            <button type="button" className="tb" onClick={() => void go()}>
              Ask again, the same order
            </button>
            <button type="button" className="tb" onClick={dismiss}>
              Stop asking
            </button>
          </div>
        </>
      )}
      {now.state === "refused" && (
        <>
          <Failure error={now.error} lead={now.rose ? "It costs more than the price you agreed to" : "Nothing was rented"} />
          <div className="row">
            <button type="button" className="tb" onClick={() => void quote(client, now.order)}>
              {now.rose ? "See the new price" : "Quote again"}
            </button>
            <button type="button" className="tb" onClick={dismiss}>
              Not now
            </button>
          </div>
        </>
      )}
    </div>
  )
}

/** The SKU list. Rent is one click on a stated price from the quote. */
export function Rent({ signedIn }: { signedIn: boolean }) {
  const { client } = useConnection()
  const to = useTo()
  const now = useSignal(order)
  const [sshKey, setSshKey] = useState(readKey)
  const { data: skus, error } = useQuery({ ...rentalSkusQuery(client), staleTime: 0, refetchOnMount: "always" })
  const asking = now.state === "placing" || now.state === "uncertain" || now.state === "quoting"
  const key = sshKey.trim()
  const badKey = key.includes("PRIVATE KEY") || /[\r\n]/.test(key)
  // A render asked for this one: it's chosen, and signed in its price is asked, which rents nothing.
  const [search] = useSearchParams()
  const wanted = search.get("rent")
  const wantedGpus = Number(search.get("gpus")) || 1
  const chosen = skus?.find((sku) => sku.name === wanted)

  const pick = (sku: RentalSku, gpus: number) => {
    saveKey(badKey ? "" : key)
    // As the CLI: one GPU, or none on the CPU product, is what naming no count asks for.
    void quote(client, { sku: sku.name, gpus: gpus > 1 ? gpus : undefined, ssh_key: key && !badKey ? key : undefined })
  }
  // biome-ignore lint/correctness/useExhaustiveDependencies: once for each one a render asks for
  useEffect(() => {
    if (chosen && signedIn && !asking) pick(chosen, wantedGpus)
  }, [chosen?.name, wantedGpus, signedIn])

  return (
    <Section title="Rent a machine">
      {!signedIn && (
        <p className="muted">
          <Link className="more" to={to("account")}>
            Sign in
          </Link>{" "}
          to rent.
        </p>
      )}
      <Failure error={error} lead="The Hub didn't list its machines" />
      <ul className="rows">
        {skus?.map((sku) => (
          <Fragment key={sku.name}>
            <Sku sku={sku} disabled={asking || !signedIn} chosen={sku === chosen ? wantedGpus : undefined} onPick={pick} />
            {now.state !== "idle" && now.order.sku === sku.name && (
              <li className="order-row">
                <Order />
              </li>
            )}
          </Fragment>
        ))}
      </ul>
      {skus && !skus.length && <p className="muted">No machine is for rent on this Hub right now.</p>}
      <details className="sect">
        <summary>Log in over SSH</summary>
        <input
          className="field wide"
          aria-label="An OpenSSH public key"
          placeholder="ssh-ed25519 AAAA… (the .pub file's one line)"
          spellCheck={false}
          value={sshKey}
          onChange={(event) => setSshKey(event.target.value)}
        />
        {badKey && <p className="warn">That isn't one public key line, so it won't be sent.</p>}
      </details>
    </Section>
  )
}
