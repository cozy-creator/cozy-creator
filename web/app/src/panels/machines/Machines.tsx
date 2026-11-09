import { useQuery, useQueryClient } from "@tanstack/react-query"
import { type FormEvent, useEffect, useState } from "react"
import { Link, useNavigate } from "react-router-dom"
import { machinesQuery } from "../../api/queries"
import { useConnection } from "../../app/Connection"
import { useTo } from "../../app/Shell"
import type { Machine, MachineList } from "../../types"
import { Failure, useAsk, useNow, useSignedIn } from "../common"
import { DrawerFrame } from "../Drawer"
import { usd } from "../format"
import { End } from "./End"
import { Rent } from "./Rent"
import { endsWords, gpusWords, stableName, stateClass, stateWords } from "./words"

function MachineRow({ machine, now }: { machine: Machine; now: number }) {
  const to = useTo()
  const trying = machine.state === "booting" && machine.kind === "rental"
  return (
    <li className={`runrow ${trying ? "trying" : ""}`}>
      <Link to={to(`machines/${encodeURIComponent(stableName(machine))}`)}>
        <span className="line">
          <span className={`dot ${stateClass(machine)}`} />
          <b>{machine.name}</b>
          <small>{gpusWords(machine)}</small>
          <span className="sp" />
          {machine.rental && <small>{usd(machine.rental.usd_micros_per_hour)}/h</small>}
          {Boolean(machine.running || machine.queued) && (
            <small>
              {machine.running ?? 0} running, {machine.queued ?? 0} queued
            </small>
          )}
        </span>
        <small className="under">
          {trying ? `Still trying${machine.attempt ? `, attempt ${machine.attempt.number}` : ""}: ${stateWords(machine, now)}` : stateWords(machine, now)}
        </small>
        {machine.state === "ready" && machine.kind === "rental" && <small className="under">{endsWords(machine, now)}</small>}
        {machine.state === "ending" && <small className="under warn">{machine.ending ? `Waiting to end it: ${machine.ending}` : machine.reason === "release_unconfirmed" ? "The Hub hasn't confirmed it gone: it may still be billing" : "Bills until its Hub affirms it gone"}</small>}
      </Link>
      {trying && machine.ordered !== false && (
        <div className="rowend">
          <End machine={machine} />
        </div>
      )}
    </li>
  )
}

function AddMachine() {
  const { client } = useConnection()
  const queries = useQueryClient()
  const navigate = useNavigate()
  const to = useTo()
  const ask = useAsk()
  const [name, setName] = useState("")
  const [address, setAddress] = useState("")
  const [fingerprint, setFingerprint] = useState("")

  const add = (event: FormEvent) => {
    event.preventDefault()
    void ask.run(async () => {
      const added = await client.addMachine(name.trim(), address.trim(), fingerprint.trim())
      await queries.invalidateQueries({ queryKey: ["machines"] })
      navigate(to(`machines/${encodeURIComponent(added.name)}`))
    })
  }

  return (
    <details className="sect">
      <summary>Add another computer</summary>
      <form className="stack" onSubmit={add}>
        <label>
          Name
          <input className="field" spellCheck={false} value={name} onChange={(event) => setName(event.target.value)} />
        </label>
        <label>
          Address
          <input className="field" spellCheck={false} placeholder="host:port" value={address} onChange={(event) => setAddress(event.target.value)} />
        </label>
        <label>
          Fingerprint
          <input className="field" spellCheck={false} placeholder="sha256:… from `cozy machine show local` there" value={fingerprint} onChange={(event) => setFingerprint(event.target.value)} />
        </label>
        <div className="row end">
          <button type="submit" className="tb" disabled={ask.busy || !name.trim() || !address.trim() || !fingerprint.trim()}>
            Add
          </button>
        </div>
      </form>
      <Failure error={ask.error} lead="Couldn't add it" />
    </details>
  )
}

/** The machines, live, and renting one. */
export function Machines() {
  const { client } = useConnection()
  const queries = useQueryClient()
  const to = useTo()
  const { data, error } = useQuery(machinesQuery(client))

  // Rentals other computers made, as `cozy machine ls` lists every Hub first. The list shows meanwhile, and a Hub that doesn't answer says so.
  useEffect(() => {
    let live = true
    client
      .machines(true)
      .then((list) => {
        if (live) queries.setQueryData<MachineList>(["machines"], list)
      })
      .catch(() => {})
    return () => {
      live = false
    }
  }, [client, queries])

  const machines = data?.machines.filter((machine) => machine.state !== "ended") ?? []
  const now = useNow(machines.some((machine) => machine.state !== "stopped"))
  // This computer's, and apart, what other computers rented on the same account.
  const mine = machines.filter((machine) => machine.ordered !== false)
  const others = machines.filter((machine) => machine.ordered === false)
  const hourlyOf = (list: Machine[]) => list.reduce((sum, machine) => sum + (machine.rental?.usd_micros_per_hour ?? 0), 0)
  const rentals = mine.filter((machine) => machine.rental)
  const hourly = hourlyOf(rentals)
  const signedIn = useSignedIn() && !data?.signed_out?.length
  const renting = !client.transport.channel

  return (
    <DrawerFrame title="Machines">
      <Failure error={error} lead="Couldn't list the machines" />
      {rentals.length > 0 && (
        <p className="muted">
          {rentals.length} rented, {usd(hourly)}/h. Each ends after 15 minutes idle.
        </p>
      )}
      {data?.unanswered?.map((problem) => (
        <p className="warn" key={problem.hub}>
          {problem.hub} didn't answer: {problem.message}. Its rentals may still be billing.
        </p>
      ))}
      <ul className="rows">
        {mine.map((machine) => (
          <MachineRow key={machine.name} machine={machine} now={now} />
        ))}
      </ul>
      {others.length > 0 && (
        <details className="sect">
          <summary>
            {others.length} rented from other computers, {usd(hourlyOf(others))}/h
          </summary>
          <ul className="rows">
            {others.map((machine) => (
              <MachineRow key={machine.name} machine={machine} now={now} />
            ))}
          </ul>
        </details>
      )}
      {data && !machines.length && (
        <p className="muted">
          Renders run on a machine.{" "}
          {!signedIn && renting && (
            <Link className="more" to={to("account")}>
              Sign in to rent one
            </Link>
          )}
        </p>
      )}
      {renting && <Rent signedIn={signedIn} />}
      <AddMachine />
    </DrawerFrame>
  )
}
