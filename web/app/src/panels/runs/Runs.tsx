import { useQuery } from "@tanstack/react-query"
import { useState } from "react"
import { Link, useSearchParams } from "react-router-dom"
import { machinesQuery, maintenanceRunsQuery, projectQuery, runsQuery } from "../../api/queries"
import { useConnection } from "../../app/Connection"
import { useProjectId, useTo } from "../../app/Shell"
import type { Run } from "../../api/calcifer"
import type { Project } from "../../types"
import { Bar, Failure, useNow } from "../common"
import { DrawerFrame } from "../Drawer"
import { progressWords, runTime, settled, stalledWords, statusClass, statusWord, target } from "./words"

const STATUSES = ["queued", "in_progress", "completed", "failed", "canceled"]
const PAGE = 50

/** Each run by the box it makes, as the open project lists it: what's making now, its takes asked for, and the run behind each version. */
function boxesOf(project: Project | undefined): Map<number, string> {
  const boxes = new Map<number, string>()
  for (const asset of project?.assets ?? []) {
    for (const version of asset.prefixes ?? []) if (version.run != null) boxes.set(version.run, asset.name)
    for (const take of asset.takes ?? []) if (take.run != null) boxes.set(take.run, asset.name)
    for (const number of asset.making ?? []) boxes.set(number, asset.name)
  }
  return boxes
}

function RunRow({ run, now, box }: { run: Run; now: number; box?: string }) {
  const to = useTo()
  const live = !settled(run)
  const line = run.status === "failed" || run.status === "canceled" ? run.reason?.message : live ? progressWords(run) : ""
  return (
    <li className={`runrow ${run.stalled ? "stalled" : ""}`}>
      <Link to={to(`runs/${run.number}`)}>
        <span className="line">
          <span className={`dot ${statusClass(run)}`} />
          <b>#{run.number}</b>
          {/* Its box, the function under it (cl-958); a run of no box here, its function. */}
          {box ? <span className="nm">{box}</span> : <span className="nm">{target(run)}</span>}
          <span className="sp" />
          <small>{statusWord(run)}</small>
          {run.machine && <small>{run.machine}</small>}
          <small>{runTime(run, now)}</small>
        </span>
        {box && <small className="under fn">{target(run)}</small>}
        {line && <small className="under">{line}</small>}
        {run.stalled && <small className="under warn">{stalledWords(run.stalled, now)}</small>}
        {live && run.progress?.done_fraction != null && <Bar fraction={run.progress.done_fraction} />}
      </Link>
    </li>
  )
}

/** This project's runs, or project-independent maintenance; by machine and status. */
export function Runs() {
  const { client, stream } = useConnection()
  const to = useTo()
  const id = useProjectId()
  const { data: project } = useQuery({ ...projectQuery(client, stream, id ?? ""), enabled: Boolean(id) })
  const boxes = boxesOf(project)
  const [params, setParams] = useSearchParams()
  const [limit, setLimit] = useState(PAGE)
  const status = params.get("status") ?? ""
  const machine = params.get("machine") ?? ""
  const { data, error } = useQuery(id ? runsQuery(client, id, { status, machine }, limit) : maintenanceRunsQuery(client, { status, machine }, limit))
  const { data: machines } = useQuery(machinesQuery(client))
  const runs = data?.runs ?? []
  const now = useNow(runs.some((run) => !settled(run)))

  const set = (name: string, value: string | null) => {
    const next = new URLSearchParams(params)
    if (value === null) next.delete(name)
    else next.set(name, value)
    setParams(next, { replace: true })
  }

  const names = new Set(machines?.machines.map((listed) => listed.name))
  if (machine) names.add(machine)

  return (
    <DrawerFrame title="Runs">
      <div className="row filters">
        <span className="sp" />
        <select className="field" aria-label="Status" value={status} onChange={(event) => set("status", event.target.value || null)}>
          <option value="">Any status</option>
          {STATUSES.map((word) => (
            <option key={word} value={word}>
              {word === "in_progress" ? "running" : word}
            </option>
          ))}
        </select>
        <select className="field" aria-label="Machine" value={machine} onChange={(event) => set("machine", event.target.value || null)}>
          <option value="">Any machine</option>
          {[...names].map((name) => (
            <option key={name} value={name}>
              {name}
            </option>
          ))}
        </select>
      </div>
      <Failure error={error} lead="Couldn't list the runs" />
      <ul className="rows">
        {runs.map((run) => (
          <RunRow key={run.number} run={run} now={now} box={run.project_id === id ? boxes.get(run.number) : undefined} />
        ))}
      </ul>
      {data && !runs.length && (
        <p className="muted">
          {status || machine ? "No runs match." : !id ? "No runs yet." : "No runs in this project yet."}
          {id && !status && !machine && (
            <>
              {" "}
              <Link className="more" to={to("new")}>
                Run a function
              </Link>
            </>
          )}
        </p>
      )}
      {data?.unreadable?.map((line) => (
        <p className="muted" key={line}>
          Left out {line}
        </p>
      ))}
      {Boolean(data?.older) && (
        <button type="button" className="tb" onClick={() => setLimit(Math.min(1000, limit + 200))}>
          {data?.older} older
        </button>
      )}
    </DrawerFrame>
  )
}
