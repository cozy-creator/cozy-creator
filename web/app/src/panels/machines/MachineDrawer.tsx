import { useQuery, useQueryClient } from "@tanstack/react-query"
import { type FormEvent, useEffect, useRef, useState } from "react"
import { useParams } from "react-router-dom"
import { machineQuery, machinesQuery } from "../../api/queries"
import { useConnection } from "../../app/Connection"
import { useTo } from "../../app/Shell"
import type { Machine, MachineUpdate } from "../../api/calcifer"
import { Copyable } from "../../ui/Copyable"
import { Failure, KV, Section, useAsk, useNow } from "../common"
import { DrawerFrame } from "../Drawer"
import { span } from "../format"
import { Disk } from "./Disk"
import { End } from "./End"
import { Gpus } from "./Gpus"
import { Models, Packages } from "./Installed"
import { MachineLogs } from "./MachineLogs"
import { Queue } from "./Queue"
import { RenderCheck, Spec, useRenderMachines } from "./RenderHere"
import { statusWord, target } from "../runs/words"
import { brokenWords, called, endsWords, kindWords, kernelsWords, named, spentWords } from "./words"
import "./MachineDrawer.css"

/** Its idle clock renewed (`cozy machine keepalive`), which only a person asks for: a rental ends 15 minutes after its last job. */
function KeepRunning({ machine }: { machine: Machine }) {
  const { client } = useConnection()
  const ask = useAsk()
  if (machine.kind !== "rental" || machine.state !== "ready" || machine.ordered === false) return null
  return (
    <>
      <button type="button" className="tb" title="Restart its 15-minute idle timer" disabled={ask.busy} onClick={() => void ask.run(() => client.keepAlive(machine.name))}>
        Keep running
      </button>
      <Failure error={ask.error} lead="Couldn't keep it running" />
    </>
  )
}

/** To a release, by version: dev builds stay in the CLI. Nothing named moves each to its Hub's target. */
function Update({ machine }: { machine: Machine }) {
  const { client } = useConnection()
  const ask = useAsk()
  const [runtime, setRuntime] = useState("")
  const [tensorfs, setTensorfs] = useState("")
  const [tensord, setTensord] = useState("")
  const [said, setSaid] = useState("")
  const asked = useRef<{ body: string; key: string } | null>(null)

  const update = (event: FormEvent) => {
    event.preventDefault()
    const body: MachineUpdate = {}
    if (runtime.trim()) body.runtime = { version: runtime.trim() }
    if (tensorfs.trim()) body.tensorfs = { version: tensorfs.trim() }
    if (tensord.trim()) body.tensord = { version: tensord.trim() }
    const text = JSON.stringify(body)
    if (asked.current?.body !== text) asked.current = { body: text, key: `update-${crypto.randomUUID()}` }
    const { key } = asked.current
    void ask.run(async () => {
      const updating = await client.updateMachine(machine.name, body, key)
      asked.current = null
      // Its update is a job of the machine's (cl-1028), listed under its jobs as it goes.
      if (updating.run) setSaid(`Updating to ${updating.target}: it shows under Its jobs.`)
      else setSaid(`Already up to date (${updating.target}).`)
    })
  }

  const status = machine.status
  return (
    <details className="sect">
      <summary>Update</summary>
      <form className="stack" onSubmit={update}>
        <label>
          Runtime
          <input className="field" spellCheck={false} placeholder={status?.runtime || "The Hub's recommended version"} value={runtime} onChange={(event) => setRuntime(event.target.value)} />
        </label>
        <label>
          TensorFS
          <input className="field" spellCheck={false} placeholder={status?.tensorfs || "The Hub's recommended version"} value={tensorfs} onChange={(event) => setTensorfs(event.target.value)} />
        </label>
        <label>
          Cozy
          <input className="field" spellCheck={false} placeholder={status?.version || "The Hub's recommended version"} value={tensord} onChange={(event) => setTensord(event.target.value)} />
        </label>
        <div className="row end">
          <button type="submit" className="tb" disabled={ask.busy}>
            {runtime || tensorfs || tensord ? "Update" : "Update to the recommended versions"}
          </button>
        </div>
      </form>
      {machine.kind === "local" && tensord.trim() && <p className="muted">Cozy switches to that version the next time you use it from a terminal.</p>}
      {said && <p className="muted">{said}</p>}
      <Failure error={ask.error} lead="Couldn't update this machine" />
    </details>
  )
}

/** All the rest it knows, shown when its Details is: versions, addresses and keys, updating it, its logs (Paul 11-12: its face is its
 *  spec, what it's doing, when it ends, and what to do with it; 11-16: then its GPUs, disk, queue, packages and models). */
function Details({ machine, now }: { machine: Machine; now: number }) {
  const ended = machine.state === "ended"
  const status = machine.status
  const rental = machine.rental
  return (
    <div className="sect machine-details">
      <KV
      rows={[
        ["Type", kindWords(machine)],
        ["Running for", rental ? span((now - rental.rented_at_ms) / 1000) : status && span((now - status.started_at_ms) / 1000)],
        ["Cozy", status && `${status.version}${status.dev ? " (dev)" : ""}`],
        ["Runtime", status?.runtime],
        ["TensorFS", status?.tensorfs],
        ["RC", machine.rc && `${machine.rc.rc} ${machine.rc.state}: ${machine.rc.detail}`],
        ["PyTorch", status?.torch],
        ["Update", status?.update],
        ["Disk", !status?.disk_total_bytes && rental?.disk_gb ? `${rental.disk_gb}GB` : ""],
        ["Address", machine.address],
        ["Fingerprint", machine.fingerprint],
        ["Public key", machine.controller_key],
        ["Address for other computers", machine.reach],
        ["Location", rental?.placement_note],
        ["Why it failed", rental?.failure],
        ["Why it ended", machine.reason],
        ["Couldn't end", machine.end_failed],
      ]}
      />
      {machine.ssh && (
        <Section title="SSH">
          <Copyable text={machine.ssh.line} />
        </Section>
      )}
      {Boolean(machine.jobs?.length) && (
        <Section title="Its jobs">
          <ul className="rows">
            {machine.jobs?.map((job) => (
              <li className="heldrow" key={job.id} data-job={job.target}>
                <span className="nm">{target(job)}</span>
                <small>{[statusWord(job), job.progress?.stage, job.waiting, job.reason?.message].filter(Boolean).join(" · ")}</small>
              </li>
            ))}
          </ul>
        </Section>
      )}
      {machine.state === "ready" && <Update machine={machine} />}
      {!ended && <MachineLogs machine={machine} />}
    </div>
  )
}

/** What it's doing, in jobs, its GPUs' work (cl-889), what it's cost so far, and when it ends. Calcifer counts a job it sent as running;
 *  the machine says which of those it runs now and which wait there, as its queue does. */
function Doing({ machine, now }: { machine: Machine; now: number }) {
  const runs = machine.status?.runs
  const total = machine.running + machine.queued
  const running = runs ? Math.min(total, runs.filter((held) => held.state === "running" && (!held.run || held.run.kind === "call")).length) : machine.running
  const jobs = total ? `${running} running, ${total - running} queued` : "Nothing running"
  const ends = endsWords(machine, now)
  const spent = spentWords(machine)
  return (
    <p className="doing-now">
      <span>{jobs}</span>
      {spent && <span className="muted">{spent}</span>}
      {ends && <span className={machine.status?.idle_deadline_ms ? "warn" : "muted"}>{ends}</span>}
    </p>
  )
}

/** One machine, read from the live list while it's in it: reading it alone would start `local`. */
export function MachineDrawer() {
  const { client } = useConnection()
  const queries = useQueryClient()
  const to = useTo()
  const name = useParams().name ?? ""
  const { data: list } = useQuery(machinesQuery(client))
  const listed = list?.machines.find((machine) => named(machine, name))
  const { data: record, error } = useQuery({ ...machineQuery(client, name), enabled: Boolean(list && !listed) })
  // Its record is what the list last said: once it leaves the list (it ended), that shows until the record is read again.
  useEffect(() => {
    if (listed) queries.setQueryData(machineQuery(client, name).queryKey, listed)
  }, [queries, client, name, listed])
  const machine = listed ?? record
  const now = useNow(Boolean(machine && machine.state !== "ended"))
  const { chosen } = useRenderMachines()
  const [details, setDetails] = useState(false)

  if (!machine) {
    return (
      <DrawerFrame title={name} back={to("machines")}>
        <Failure error={error} lead={`Couldn't read ${name}`} />
      </DrawerFrame>
    )
  }

  const ended = machine.state === "ended"
  const here = chosen.includes(machine.name)
  return (
    <DrawerFrame title={called(machine)} back={to("machines")}>
      <div className={`machine-head ${here ? "render-here" : ""}`}>
        <Spec machine={machine} now={now} name={false} />
        <div className="checks">
          {(!ended || here) && <RenderCheck name={machine.name} called={called(machine)} />}
          <button type="button" className="more" aria-expanded={details} onClick={() => setDetails(!details)}>
            Details
          </button>
        </div>
      </div>
      {ended && <p className="muted">Ended. It costs nothing more, and results already downloaded stay on this computer.</p>}
      {machine.state === "booting" && (
        <p className="muted">
          Starting up{machine.attempt ? ` (attempt ${machine.attempt.number})` : ""}. You pay only once it's ready, and it keeps going if you close this page.
          {machine.attempt?.replaced && ` The last attempt failed: ${machine.attempt.replaced.failure}.`}
        </p>
      )}
      {machine.offline && <p className="warn">It's offline, so it can't take runs. It comes back if its provider resumes it, and ends after 15 minutes idle.</p>}
      {brokenWords(machine) && <p className="warn">{brokenWords(machine)}</p>}
      {machine.unreachable && !ended && <p className="muted">It didn't respond just now ({machine.unreachable}), so this is from its last report.</p>}
      {!ended && <Doing machine={machine} now={now} />}
      <div className="machine-actions">
        <KeepRunning machine={machine} />
        <End machine={machine} />
      </div>
      {machine.status && kernelsWords(machine.status) && !ended && <p className="warn slow">{kernelsWords(machine.status)}</p>}
      {machine.status?.stuck?.map((stuck) => (
        <p className="warn" key={stuck.pid}>
          Process {stuck.pid} ({stuck.command}) was stopped but is stuck in the kernel{stuck.wchan ? ` (${stuck.wchan})` : ""}. What it's using stays in use until it exits.
        </p>
      ))}
      {details && <Details machine={machine} now={now} />}
      {!ended && machine.status && (
        <>
          <Gpus machine={machine} status={machine.status} />
          <Disk machine={machine} status={machine.status} />
          <Queue machine={machine} status={machine.status} />
          <Packages status={machine.status} />
          <Models status={machine.status} />
        </>
      )}
      {!ended && !machine.status && <p className="muted">Its GPUs, disk, queue, packages and models show here once it reports in.</p>}
    </DrawerFrame>
  )
}
