import { useQuery, useQueryClient } from "@tanstack/react-query"
import { type FormEvent, useEffect, useState } from "react"
import { Link, useLocation, useNavigate } from "react-router-dom"
import { useGpuTrace, useShowGpus } from "../../api/gpus"
import { machinesQuery } from "../../api/queries"
import { useConnection } from "../../app/Connection"
import { useTo } from "../../app/Shell"
import type { Machine, MachineList } from "../../api/calcifer"
import { Failure, useAsk, useNow, useSignedIn } from "../common"
import { DrawerFrame } from "../Drawer"
import { bytes, usd } from "../format"
import { nearlyFull } from "./Disk"
import { End } from "./End"
import { GpuStrip } from "./GpuGauges"
import { Rent } from "./Rent"
import { Checks, RenderCheck, Spec, useRenderMachines } from "./RenderHere"
import { byRelation, clientFrom, clientFunction, clientWords } from "./activity"
import { endsWords, holdWords, stableName } from "./words"

/** Its GPUs' gauges while it's up (cl-1191). */
function MachineGpus({ machine }: { machine: Machine }) {
  const { stream } = useConnection()
  const trace = useGpuTrace(stream, machine.name)
  if (machine.state !== "ready") return null
  return <GpuStrip trace={trace} gpus={machine.status?.gpus ?? []} />
}

function MachineRow({ machine, now, here, tabbed }: { machine: Machine; now: number; here: boolean; tabbed?: boolean }) {
  const to = useTo()
  const trying = machine.state === "booting" && machine.kind === "rental"
  // Seen before a render finds it (cl-1175).
  const full = machine.status && nearlyFull(machine.status)
  return (
    <li className={`runrow machine ${trying ? "trying" : ""} ${here ? "render-here" : ""} ${tabbed ? "tabbed" : ""}`} data-machine={machine.name}>
      <Spec machine={machine} now={now} />
      <MachineGpus machine={machine} />
      {Boolean(machine.running || machine.queued) && (
        <small className="under">
          {machine.running ?? 0} running, {machine.queued ?? 0} queued
        </small>
      )}
      {machine.status?.others?.map((other, at) => (
        <small className="under" key={at} data-client={other.client}>
          {clientFrom(other)}: {clientFunction(other)} · {clientWords(other, now)}
        </small>
      ))}
      {trying && machine.attempt && <small className="under">Starting up, attempt {machine.attempt.number}</small>}
      {machine.state === "ready" && machine.kind === "rental" && (
        <small className="under">{[endsWords(machine, now), ...holdWords(machine, now), machine.ordered === false ? "Rented by another computer" : ""].filter(Boolean).join(" · ")}</small>
      )}
      {full && (
        <small className="under warn">
          Disk nearly full: {bytes(full.room)} of room for models, and its usual models need {bytes(full.needs)}
        </small>
      )}
      {machine.end_failed && <small className="under warn">Didn't shut down and may still cost money: {machine.end_failed}</small>}
      {machine.end_unconfirmed && <small className="under warn">The Hub hasn't confirmed it has ended: {machine.end_unconfirmed}</small>}
      {machine.state === "ending" && !machine.end_unconfirmed && <small className="under warn">{machine.ending ? `Ending once this finishes: ${machine.ending}` : "Costs money until the Hub confirms it has ended"}</small>}
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
          <input className="field" spellCheck={false} placeholder="sha256:… (ask that computer's owner)" value={fingerprint} onChange={(event) => setFingerprint(event.target.value)} />
        </label>
        <div className="row end">
          <button type="submit" className="tb" disabled={ask.busy || !name.trim() || !address.trim() || !fingerprint.trim()}>
            Add
          </button>
        </div>
      </form>
      <Failure error={ask.error} lead="Couldn't add that computer" />
    </details>
  )
}

/** What the tabs ask of this drawer: the machines with no tab first. */
export interface MachinesAsk {
  first?: string[]
}

/** Signed in at every Hub the machines list read: renting is open. */
function useCanRent(data: MachineList | undefined) {
  return useSignedIn() && !data?.signed_out?.length
}

/** The machines, live, and renting one. */
export function Machines() {
  const { client, stream } = useConnection()
  useShowGpus(stream)
  const queries = useQueryClient()
  const to = useTo()
  const { data, error } = useQuery(machinesQuery(client))
  const { project, chosen } = useRenderMachines()
  const ask: MachinesAsk = useLocation().state ?? {}
  const first = new Set(ask.first)

  // Every Hub listed again, as `cozy machine ls` does, for rentals other computers made. The list shows meanwhile, and a Hub that doesn't answer says so.
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
  // One list: every machine, and every rental of the account, whichever computer rented it (cl-1193): `local`, then those working for
  // the open project, then the rest. Opened from "N more", those with no tab first.
  const byTab = (a: Machine, b: Machine) => Number(first.has(b.name)) - Number(first.has(a.name))
  const listed = [...machines].sort(byRelation(project ?? null, chosen)).sort(byTab)
  const rentals = listed.filter((machine) => machine.rental)
  const hourly = rentals.reduce((sum, machine) => sum + (machine.rental?.usd_micros_per_hour ?? 0), 0)
  const signedIn = useCanRent(data)
  const renting = !client.transport.channel
  // The project's machines that aren't up are listed too, so each can be taken off.
  const gone = chosen.filter((name) => !machines.some((machine) => machine.name === name))

  return (
    <DrawerFrame title="Machines">
      <Failure error={error} lead="Couldn't load your machines" />
      {rentals.length > 0 && (
        <p className="muted">
          {rentals.length} {rentals.length === 1 ? "rental" : "rentals"}, {usd(hourly)}/h in all. Each ends after 15 minutes idle.
        </p>
      )}
      {data?.unanswered?.map((problem) => (
        <p className="warn" key={problem.hub}>
          {problem.hub} didn't respond ({problem.message}). Its rentals may still be costing money.
        </p>
      ))}
      {project && data && !chosen.length && <p className="muted">Cozy picks where this project renders. Check Prefer on a machine to render there.</p>}
      <ul className="rows">
        {listed.map((machine, at) => (
          <MachineRow key={machine.name} machine={machine} now={now} here={chosen.includes(machine.name)} tabbed={at > 0 && first.has(listed[at - 1].name) && !first.has(machine.name)} />
        ))}
        {gone.map((name) => (
          <li key={name} className="runrow machine render-here" data-machine={name}>
            <div className="spec">
              <b>{name}</b>
              <span>Not running</span>
            </div>
            <small className="under warn">This project's renders wait for it or another preferred machine.</small>
            <div className="checks">
              <RenderCheck name={name} />
              <Link className="more" to={to(`machines/${encodeURIComponent(name)}`)}>
                Details
              </Link>
            </div>
          </li>
        ))}
      </ul>
      {data && !machines.length && (
        <p className="muted">
          No machines yet.{" "}
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

/** The header's +: the rent form alone, as Machines holds it. */
export function NewMachine() {
  const { client } = useConnection()
  const { data } = useQuery(machinesQuery(client))
  const signedIn = useCanRent(data)
  return (
    <DrawerFrame title="New machine">
      <Rent signedIn={signedIn} />
    </DrawerFrame>
  )
}
