import { useQuery } from "@tanstack/react-query"
import { useEffect, useRef } from "react"
import { Link, useLocation } from "react-router-dom"
import type { ClientRun, LiveRun, Machine, MachineStatus, Operation, Run } from "../../api/calcifer"
import { projectQuery } from "../../api/queries"
import { useConnection } from "../../app/Connection"
import { useProjectId, useTo } from "../../app/project"
import { Icon } from "../../ui/Icon"
import { Bar, Section } from "../common"
import { ordinal } from "../format"
import { downloadWait, runName as runLabel, waitWords, workWords } from "../runs/words"
import { anchorOf, clientFraction, clientFrom, clientFunction, clientWords, fetchedWords, fetchesWords, fractionOf, namedOf, operationName, ownActivity, runName, showRow, waitersWords, waitingWords, waitsOnOf } from "./activity"

const sentence = (text: string) => text.charAt(0).toUpperCase() + text.slice(1)

/** Running first, then getting ready, then queued in line order. */
const STEP: Record<string, number> = { running: 0, preparing: 1, queued: 2 }
const order = (a: LiveRun, b: LiveRun) =>
  (STEP[a.state] ?? 1) - (STEP[b.state] ?? 1) || (a.run?.queue_position ?? Infinity) - (b.run?.queue_position ?? Infinity) || (a.run?.number ?? 0) - (b.run?.number ?? 0)

function OperationRow({ operation }: { operation: Operation }) {
  const fraction = fractionOf(operation)
  const download = operation.kind === "download"
  // A box's "Waiting for download" or "Waiting for install" leads here (`#download-…`, `#install-…`; cl-1205, cl-1218): the row is
  // shown and lit.
  const { hash } = useLocation()
  const row = useRef<HTMLLIElement>(null)
  const anchor = anchorOf(operation.id)
  useEffect(() => {
    if (row.current && hash === `#${anchor}`) showRow(row.current, operation.id)
  }, [hash, anchor, operation.id])
  return (
    <li className="qrow" data-queue={operation.kind} data-operation={operation.id} id={anchor} ref={row}>
      <span className={`mark ${download ? "download" : "install"}`}>
        <Icon name={download ? "download" : "package"} />
      </span>
      <b title={fetchesWords(operation)}>{operationName(operation)}</b>
      <span className="kind">{download ? "Download" : "Install"}</span>
      {fraction !== undefined && <Bar fraction={fraction} />}
      <span className="said">
        {download ? fetchedWords(operation) : sentence(operation.stage || "installing")} · for {waitersWords(operation)}
      </span>
    </li>
  )
}

function RunRow({ held, activity, project, boxes }: { held: LiveRun; activity: Operation[]; project: string | null; boxes: Map<number, string> }) {
  const to = useTo()
  const run: Run | null | undefined = held.run
  const state = held.state
  const going = state === "running"
  if (held.locked || !run || (run.project_id && run.project_id !== project)) {
    return (
      <li className="qrow" data-queue="run" data-state={state}>
        <span className={`dot ${going ? "work" : "idle"}`} />
        <b>{held.locked ? "A locked project's run" : run ? "Another project's run" : "A run this computer has no record of"}</b>
        <span className="kind">{sentence(state)}</span>
      </li>
    )
  }
  const on = waitsOnOf(activity, run)
  // Waiting on an install or downloads, it says so and leads there, which alone shows how far it is (cl-1205, cl-1218).
  const waited = namedOf(activity, run, on)
  const fraction = run.progress?.done_fraction
  const waiting = held.waiting || run.waiting
  const said = waited.length
    ? waitingWords(waited)
    : going || downloadWait(run)
      ? workWords(run) || waitWords(waiting) || "Running"
      : state === "queued"
        ? run.queue_position
          ? `Queued, ${ordinal(run.queue_position)} in line`
          : waitWords(waiting) || "Queued"
        : waitWords(waiting) || "Getting ready"
  return (
    <li className="qrow" data-queue="run" data-run={run.number} data-state={state}>
      <span className={`dot ${going && !on ? "work" : "idle"}`} />
      <Link to={run.project_id ? to(`runs/${run.number}`) : `/runs/${run.number}`} title={`Run ${runLabel(run)}: ${run.target}`}>
        <b>{runName(run, boxes)}</b>
      </Link>
      <span className="kind">Run {runLabel(run)}</span>
      {going && !on && fraction != null && <Bar fraction={fraction} />}
      {waited.length ? (
        <a className="said waits" href={`#${anchorOf(waited[0].id)}`} title={waited.map(fetchesWords).join("; ")} onClick={(event) => { event.preventDefault(); showRow(event.currentTarget, waited[0].id) }}>
          {said}
        </a>
      ) : (
        <span className="said">{said}</span>
      )}
    </li>
  )
}

/** A run another computer sent, as the machine says it (cl-1193): what it runs, from which computer, how far. */
function ClientRow({ other }: { other: ClientRun }) {
  const fraction = clientFraction(other)
  return (
    <li className="qrow" data-queue="client" data-state={other.running ? "running" : "queued"}>
      <span className={`dot ${other.running ? "work" : "idle"}`} />
      <b title={other.function}>{clientFunction(other)}</b>
      <span className="kind">{clientFrom(other)}</span>
      {fraction !== undefined && <Bar fraction={fraction} />}
      <span className="said">{clientWords(other, Date.now())}</span>
    </li>
  )
}

/** What it's doing and what waits (Paul 11-16, "it's run queue, list of jobs / operations queued up / in progress"): its downloads and
 *  installs with how far they are, then its runs in the order they go, each once. Kept by the machines' stream. */
export function Queue({ machine, status }: { machine: Machine; status: MachineStatus }) {
  const { client, stream } = useConnection()
  const to = useTo()
  const project = useProjectId()
  const { data: open } = useQuery({ ...projectQuery(client, stream, project ?? ""), enabled: Boolean(project) })
  const boxes = new Map((open?.assets ?? []).flatMap((asset) => (asset.making ?? []).map((number) => [number, asset.name] as const)))
  const activity = ownActivity(machine, project)
  const operations = [...activity].sort((a, b) => Number(a.kind === "download") - Number(b.kind === "download"))
  const runs = [...(status.runs ?? [])].sort(order)
  const others = [...(status.others ?? [])].sort((a, b) => Number(Boolean(b.running)) - Number(Boolean(a.running)))
  return (
    <Section title="Queue">
      {operations.length || runs.length || others.length ? (
        <ul className="queue">
          {operations.map((operation) => (
            <OperationRow key={operation.id} operation={operation} />
          ))}
          {runs.map((held, at) => (
            <RunRow key={held.id || `locked ${at}`} held={held} activity={activity} project={project} boxes={boxes} />
          ))}
          {others.map((other, at) => (
            <ClientRow key={`client ${at}`} other={other} />
          ))}
        </ul>
      ) : (
        <p className="muted">Nothing running or queued.</p>
      )}
      <p>
        <Link className="more" to={`${to("runs")}?all&machine=${encodeURIComponent(machine.name)}`}>
          All runs on this machine
        </Link>
      </p>
    </Section>
  )
}
