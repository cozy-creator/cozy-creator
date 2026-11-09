import { useQuery, useQueryClient } from "@tanstack/react-query"
import { type FormEvent, useEffect, useState } from "react"
import { Link, useNavigate } from "react-router-dom"
import { machinesQuery } from "../../api/queries"
import { chooseCalcifer, useConnection } from "../../app/Connection"
import { useTo } from "../../app/Shell"
import type { Machine, MachineList } from "../../api/calcifer"
import { Failure, useAsk, useNow, useSignedIn } from "../common"
import { DrawerFrame } from "../Drawer"
import { usd } from "../format"
import { End } from "./End"
import { Rent } from "./Rent"
import { endsWords, gpusWords, named, stableName, stateClass, stateWords } from "./words"

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

const lookAt = (machine: string | null) => {
  // A project's id is its folder's on one computer, so a route under one means nothing on the other.
  history.replaceState(null, "", `${location.pathname}${location.search}#/machines`)
  chooseCalcifer(machine)
}

/**
 * Whose world every drawer shows: this computer's, or a rental's own Calcifer through this one, where the pod is `local`. One at a time, since
 * its stream costs a connection. Its End stays here, on this computer's Calcifer.
 */
function Switcher() {
  const { here, remote, client } = useConnection()
  // Here, the list the stream keeps live. Through a door the stream is the pod's and nothing feeds this computer's list, so each opening reads it.
  const { data } = useQuery({
    queryKey: remote ? ["here", "machines"] : ["machines"],
    queryFn: () => here.machines(),
    refetchOnMount: remote ? "always" : true,
    enabled: !client.transport.channel,
  })
  const doors = data?.machines.filter((machine) => machine.kind === "rental" && machine.state === "ready" && machine.ordered !== false && !machine.offline) ?? []
  const looked = remote ? data?.machines.find((machine) => named(machine, remote)) : undefined
  if (!remote && !doors.length) return null
  return (
    <div className="switcher">
      <div className="row center">
        <label className="check">
          Looking at
          <select className="field" aria-label="Which Calcifer" value={remote ?? ""} onChange={(event) => lookAt(event.target.value || null)}>
            <option value="">This computer</option>
            {remote && !doors.some((machine) => named(machine, remote)) && <option value={remote}>{remote}</option>}
            {doors.map((machine) => (
              <option key={machine.name} value={machine.name}>
                {machine.name}
              </option>
            ))}
          </select>
        </label>
        <span className="sp" />
        {looked && looked.state !== "ending" && <End machine={looked} through={here} onAsked={() => lookAt(null)} />}
      </div>
      {remote && <p className="muted">It's <code>local</code> here. What's private stays on this computer.</p>}
    </div>
  )
}

/** The machines, live, and renting one. */
export function Machines() {
  const { client, remote } = useConnection()
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
  const renting = !remote && !client.transport.channel

  return (
    <DrawerFrame title="Machines">
      <Switcher />
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
      {!remote && <AddMachine />}
    </DrawerFrame>
  )
}
