import { useQuery } from "@tanstack/react-query"
import { Link } from "react-router-dom"
import type { LiveRun, Machine, MachineStatus, Operation, Run } from "../../api/calcifer"
import { projectQuery } from "../../api/queries"
import { useConnection } from "../../app/Connection"
import { useProjectId, useTo } from "../../app/project"
import { Icon } from "../../ui/Icon"
import { Bar, Section } from "../common"
import { ordinal } from "../format"
import { runName as runLabel, workWords } from "../runs/words"
import { fetchedWords, fractionOf, operationName, ownActivity, runName, waitsOnOf } from "./activity"

const sentence = (text: string) => text.charAt(0).toUpperCase() + text.slice(1)

const runsOf = (operation: Operation) => {
  const runs = operation.names?.length ? operation.names : operation.runs.map(String)
  return runs.length === 1 ? `run ${runs[0]}` : `runs ${runs.join(", ")}`
}

/** Running first, then getting ready, then queued in line order. */
const STEP: Record<string, number> = { running: 0, preparing: 1, queued: 2 }
const order = (a: LiveRun, b: LiveRun) =>
  (STEP[a.state] ?? 1) - (STEP[b.state] ?? 1) || (a.run?.queue_position ?? Infinity) - (b.run?.queue_position ?? Infinity) || (a.run?.number ?? 0) - (b.run?.number ?? 0)

function OperationRow({ operation }: { operation: Operation }) {
  const fraction = fractionOf(operation)
  const download = operation.kind === "download"
  return (
    <li className="qrow" data-queue={operation.kind}>
      <span className={`mark ${download ? "download" : "install"}`}>
        <Icon name={download ? "download" : "package"} />
      </span>
      <b title={operation.subject}>{operationName(operation)}</b>
      <span className="kind">{download ? "Download" : "Install"}</span>
      {fraction !== undefined && <Bar fraction={fraction} />}
      <span className="said">
        {download ? fetchedWords(operation) : sentence(operation.stage || "installing")} · for {runsOf(operation)}
      </span>
    </li>
  )
}

function RunRow({ held, activity, project, boxes }: { held: LiveRun; activity: Operation[]; project: string | null; boxes: Map<number, string> }) {
  const to = useTo()
  const run: Run | null | undefined = held.run
  const state = held.state
  const going = state === "running"
  if (!run || (run.project_id && run.project_id !== project)) {
    return (
      <li className="qrow" data-queue="run" data-state={state}>
        <span className={`dot ${going ? "work" : "idle"}`} />
        <b>{run ? "Another project's run" : "A run this computer has no record of"}</b>
        <span className="kind">{sentence(state)}</span>
      </li>
    )
  }
  const on = waitsOnOf(activity, run)
  const fraction = run.progress?.done_fraction
  const waiting = held.waiting || run.waiting
  const said = on
    ? `Waiting for ${operationName(on)} to ${on.kind === "download" ? "download" : "install"}`
    : going
      ? workWords(run) || (waiting ? sentence(waiting) : "Running")
      : state === "queued"
        ? run.queue_position
          ? `Queued, ${ordinal(run.queue_position)} in line`
          : waiting
            ? sentence(waiting)
            : "Queued"
        : waiting
          ? sentence(waiting)
          : "Getting ready"
  return (
    <li className="qrow" data-queue="run" data-run={run.number} data-state={state}>
      <span className={`dot ${going && !on ? "work" : "idle"}`} />
      <Link to={run.project_id ? to(`runs/${run.number}`) : `/runs/${run.number}`} title={`Run ${runLabel(run)}: ${run.target}`}>
        <b>{runName(run, boxes)}</b>
      </Link>
      <span className="kind">Run {runLabel(run)}</span>
      {going && !on && fraction != null && <Bar fraction={fraction} />}
      <span className="said">{said}</span>
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
  return (
    <Section title="Queue">
      {operations.length || runs.length ? (
        <ul className="queue">
          {operations.map((operation) => (
            <OperationRow key={operation.id} operation={operation} />
          ))}
          {runs.map((held) => (
            <RunRow key={held.id} held={held} activity={activity} project={project} boxes={boxes} />
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
