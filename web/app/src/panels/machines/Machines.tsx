import { useQuery, useQueryClient } from "@tanstack/react-query"
import { type FormEvent, useEffect, useState } from "react"
import { Link, useNavigate } from "react-router-dom"
import { machinesQuery } from "../../api/queries"
import { useConnection } from "../../app/Connection"
import { useTo } from "../../app/Shell"
import type { Machine, MachineList } from "../../api/calcifer"
import { Failure, useAsk, useNow, useSignedIn } from "../common"
import { DrawerFrame } from "../Drawer"
import { usd } from "../format"
import { End } from "./End"
import { Rent } from "./Rent"
import { Checks, RenderCheck, Spec, useRenderMachines } from "./RenderHere"
import { endsWords, stableName } from "./words"

function MachineRow({ machine, now, here }: { machine: Machine; now: number; here: boolean }) {
  const to = useTo()
  const trying = machine.state === "booting" && machine.kind === "rental"
  return (
    <li className={`runrow machine ${trying ? "trying" : ""} ${here ? "render-here" : ""}`} data-machine={machine.name}>
      <Spec machine={machine} now={now} />
      {Boolean(machine.running || machine.queued) && (
        <small className="under">
          {machine.running ?? 0} running, {machine.queued ?? 0} queued
        </small>
      )}
      {trying && machine.attempt && <small className="under">Still trying, attempt {machine.attempt.number}</small>}
      {machine.state === "ready" && machine.kind === "rental" && <small className="under">{endsWords(machine, now)}</small>}
      {machine.end_failed && <small className="under warn">Not shut down, may still be billing: {machine.end_failed}</small>}
      {machine.state === "ending" && <small className="under warn">{machine.ending ? `Waiting to end it: ${machine.ending}` : "Bills until its Hub affirms it gone"}</small>}
      <Checks machine={machine}>
        <Link className="more" to={to(`machines/${encodeURIComponent(stableName(machine))}`)}>
          Details
        </Link>
        {trying && machine.ordered !== false && <End machine={machine} />}
      </Checks>
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
  const { project, chosen } = useRenderMachines()

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
  // The project's machines that aren't up are listed too, so each can be taken off.
  const gone = chosen.filter((name) => !machines.some((machine) => machine.name === name))

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
      {project && data && !chosen.length && <p className="muted">Nothing checked: this project's renders are placed automatically. Check machines to keep them there.</p>}
      <ul className="rows">
        {mine.map((machine) => (
          <MachineRow key={machine.name} machine={machine} now={now} here={chosen.includes(machine.name)} />
        ))}
        {gone.map((name) => (
          <li key={name} className="runrow machine render-here" data-machine={name}>
            <div className="spec">
              <b>{name}</b>
              <span>not up</span>
            </div>
            <small className="under warn">This project's renders wait for it or another checked machine</small>
            <div className="checks">
              <RenderCheck name={name} />
              <Link className="more" to={to(`machines/${encodeURIComponent(name)}`)}>
                Details
              </Link>
            </div>
          </li>
        ))}
      </ul>
      {others.length > 0 && (
        <details className="sect">
          <summary>
            {others.length} rented from other computers, {usd(hourlyOf(others))}/h
          </summary>
          <ul className="rows">
            {others.map((machine) => (
              <MachineRow key={machine.name} machine={machine} now={now} here={chosen.includes(machine.name)} />
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
