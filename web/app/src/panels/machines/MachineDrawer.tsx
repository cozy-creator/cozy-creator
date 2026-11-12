import { useQuery } from "@tanstack/react-query"
import { type FormEvent, useRef, useState } from "react"
import { Link, useNavigate, useParams } from "react-router-dom"
import { machineQuery, machinesQuery } from "../../api/queries"
import { useConnection } from "../../app/Connection"
import { useTo } from "../../app/Shell"
import type { Machine, MachineUpdate } from "../../api/calcifer"
import { Copyable } from "../../ui/Copyable"
import { say } from "../../ui/notes"
import { Failure, KV, Section, useAsk, useNow } from "../common"
import { DrawerFrame } from "../Drawer"
import { bytes, shown, span } from "../format"
import { End } from "./End"
import { Installed } from "./Installed"
import { MachineLogs } from "./MachineLogs"
import { RenderCheck, Spec, useRenderMachines } from "./RenderHere"
import { called, endsWords, kindWords, named, slowWords, spentWords } from "./words"

/** Its idle clock renewed (`cozy machine keepalive`), which only a person asks for: a rental ends 15 minutes after its last job. */
function KeepRunning({ machine }: { machine: Machine }) {
  const { client } = useConnection()
  const ask = useAsk()
  if (machine.kind !== "rental" || machine.state !== "ready" || machine.ordered === false) return null
  return (
    <>
      <button type="button" className="tb" title="Starts its 15 idle minutes again" disabled={ask.busy} onClick={() => void ask.run(() => client.keepAlive(machine.name))}>
        Keep running
      </button>
      <Failure error={ask.error} lead="Couldn't keep it running" />
    </>
  )
}

/** To a release, by version: dev builds stay in the CLI. Nothing named moves each to its Hub's target. */
function Update({ machine }: { machine: Machine }) {
  const { client } = useConnection()
  const navigate = useNavigate()
  const to = useTo()
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
      updating.warnings?.forEach((warning) => say(warning))
      if (updating.run) navigate(to(`runs/${updating.run.number}`))
      else setSaid(`It runs ${updating.target}: nothing to update.`)
    })
  }

  const status = machine.status
  return (
    <details className="sect">
      <summary>Update</summary>
      <form className="stack" onSubmit={update}>
        <label>
          Runtime
          <input className="field" spellCheck={false} placeholder={status?.runtime || "the Hub's target"} value={runtime} onChange={(event) => setRuntime(event.target.value)} />
        </label>
        <label>
          TensorFS
          <input className="field" spellCheck={false} placeholder={status?.tensorfs || "the Hub's target"} value={tensorfs} onChange={(event) => setTensorfs(event.target.value)} />
        </label>
        <label>
          cozy
          <input className="field" spellCheck={false} placeholder={status?.version || "the Hub's target"} value={tensord} onChange={(event) => setTensord(event.target.value)} />
        </label>
        <div className="row end">
          <button type="submit" className="tb" disabled={ask.busy}>
            {runtime || tensorfs || tensord ? "Update" : "Update to the Hub's target"}
          </button>
        </div>
      </form>
      {machine.kind === "local" && tensord.trim() && <p className="muted">Calcifer itself moves to that cozy at the next `cozy` command in a terminal.</p>}
      {said && <p className="muted">{said}</p>}
      <Failure error={ask.error} lead="Couldn't update it" />
    </details>
  )
}

/** Under a tenth of its disk free, said before a render finds out: what a render does about it, and what holds more. */
function DiskShort({ machine }: { machine: Machine }) {
  const status = machine.status
  if (machine.state === "ended" || !status?.disk_total_bytes || (status.disk_free_bytes ?? 0) >= status.disk_total_bytes / 10) return null
  const models = status.disk_models_bytes ? `, models take ${bytes(status.disk_models_bytes)}` : ""
  return (
    <p className="warn">
      Short of disk: {bytes(status.disk_free_bytes ?? 0)} free of {bytes(status.disk_total_bytes)}{models}. A render evicts the models no queued or running job is using; one that still doesn't fit is refused before it starts. For more, rent a machine with a larger disk.
    </p>
  )
}

/** All the rest it knows, shown when its Details is: versions, disk, addresses and keys, its runs one by one, updating it, what it holds,
 *  its logs (Paul 11-12: its face is its spec, what it's doing, when it ends, and what to do with it). */
function Details({ machine, now }: { machine: Machine; now: number }) {
  const to = useTo()
  const ended = machine.state === "ended"
  const status = machine.status
  const rental = machine.rental
  const disk = status?.disk_total_bytes ? `${bytes(status.disk_free_bytes ?? 0)} free of ${bytes(status.disk_total_bytes)}` : rental?.disk_gb ? `${rental.disk_gb} GB` : ""
  return (
    <div className="sect machine-details">
      <KV
      rows={[
        ["Kind", kindWords(machine)],
        ...(status?.gpus ?? []).map((gpu): [string, string] => [`GPU ${gpu.index}`, `${gpu.name} · ${bytes(gpu.memory_bytes)}${gpu.driver ? ` · driver ${gpu.driver}` : ""}`]),
        ["Up", rental ? span((now - rental.rented_at_ms) / 1000) : status && span((now - status.started_at_ms) / 1000)],
        ["cozy", status && `${status.version}${status.dev ? " (dev)" : ""}`],
        ["Runtime", status?.runtime],
        ["TensorFS", status?.tensorfs],
        ["RC", machine.rc && `${machine.rc.rc} ${machine.rc.state}: ${machine.rc.detail}`],
        ["PyTorch", status?.torch],
        ["Update", status?.update],
        ["Disk", disk],
        ["Address", machine.address],
        ["Fingerprint", machine.fingerprint],
        ["Controller key", machine.controller_key],
        ["Add it at", machine.reach],
        ["Placed", rental?.placement_note],
        ["Failure", rental?.failure],
        ["Reason", machine.reason],
        ["End failed", machine.end_failed],
      ]}
      />
      {machine.ssh && (
        <Section title="Log in">
          <Copyable text={machine.ssh.line} />
        </Section>
      )}
      {Boolean(machine.status?.runs?.length) && (
        <Section title="Its runs now">
          <ul className="rows">
            {machine.status?.runs?.map((held) => (
              <li className="heldrow" key={held.id}>
                {held.run ? (
                  <Link className="more" to={to(`runs/${held.run.number}`)}>
                    #{held.run.number} {shown(held.run.target)}
                  </Link>
                ) : (
                  <span className="nm">No record of it here</span>
                )}
                <small>{[held.state, held.waiting].filter(Boolean).join(" · ")}</small>
              </li>
            ))}
          </ul>
        </Section>
      )}
      {!ended && (
        <p>
          <Link className="more" to={`${to("runs")}?all&machine=${encodeURIComponent(machine.name)}`}>
            Its runs
          </Link>
        </p>
      )}
      {machine.state === "ready" && <Update machine={machine} />}
      {!ended && <Installed machine={machine} />}
      {!ended && <MachineLogs machine={machine} />}
    </div>
  )
}

/** What it's doing, in jobs, its GPUs' work (cl-889), what it's cost so far, and when it ends. */
function Doing({ machine, now }: { machine: Machine; now: number }) {
  const jobs = machine.running || machine.queued ? `${machine.running} running, ${machine.queued} queued` : "No jobs"
  const ends = endsWords(machine, now)
  const spent = spentWords(machine)
  return (
    <p className="doing-now">
      <span>{jobs}</span>
      {spent && <span className="muted">{spent} so far</span>}
      {ends && <span className={machine.status?.idle_deadline_ms ? "warn" : "muted"}>{ends}</span>}
    </p>
  )
}

/** One machine, read from the live list while it's in it: reading it alone would start `local`. */
export function MachineDrawer() {
  const { client } = useConnection()
  const to = useTo()
  const name = useParams().name ?? ""
  const { data: list } = useQuery(machinesQuery(client))
  const listed = list?.machines.find((machine) => named(machine, name))
  const { data: record, error } = useQuery({ ...machineQuery(client, name), enabled: Boolean(list && !listed) })
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
      {ended && <p className="muted">Ended. Nothing of it bills now, and what landed here stays.</p>}
      {machine.state === "booting" && (
        <p className="muted">
          Still trying{machine.attempt ? `, attempt ${machine.attempt.number}` : ""}. It bills once it's ready, and goes on with this page closed.
          {machine.attempt?.replaced && ` The last attempt ended: ${machine.attempt.replaced.failure}.`}
        </p>
      )}
      {machine.offline && <p className="warn">It's offline and refuses runs. It returns if its provider resumes it, and ends after 15 minutes idle.</p>}
      {machine.unreachable && !ended && <p className="muted">It didn't answer just now ({machine.unreachable}). This is from its last report.</p>}
      {!ended && <Doing machine={machine} now={now} />}
      <div className="machine-actions">
        <KeepRunning machine={machine} />
        <End machine={machine} />
      </div>
      <DiskShort machine={machine} />
      {slowWords(machine).map((line) => (
        <p className="warn slow" key={line}>
          {line}
        </p>
      ))}
      {machine.status?.gpu_problems?.map((problem) => (
        <p className="warn" key={problem}>
          {problem}
        </p>
      ))}
      {machine.status?.stuck?.map((stuck) => (
        <p className="warn" key={stuck.pid}>
          {stuck.gpus.length
            ? `GPU ${stuck.gpus.join(", ")} is out of service: pid ${stuck.pid} (${stuck.command}) was killed and is stuck in the kernel${stuck.wchan ? ` (${stuck.wchan})` : ""}, holding ${bytes(stuck.gpu_bytes)} of its memory. Runs go to its other GPUs or machines; it comes back once the process exits.`
            : `Pid ${stuck.pid} (${stuck.command}) was killed and is stuck in the kernel${stuck.wchan ? ` (${stuck.wchan})` : ""}: what it holds stays held until it exits.`}
        </p>
      ))}
      {details && <Details machine={machine} now={now} />}
    </DrawerFrame>
  )
}
