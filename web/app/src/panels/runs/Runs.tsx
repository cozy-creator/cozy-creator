import { useQuery } from "@tanstack/react-query"
import { useState } from "react"
import { Link, useSearchParams } from "react-router-dom"
import { machinesQuery, runsQuery } from "../../api/queries"
import { useConnection } from "../../app/Connection"
import { useProjectId, useTo } from "../../app/Shell"
import type { Run } from "../../api/calcifer"
import { Bar, Failure, useNow } from "../common"
import { DrawerFrame } from "../Drawer"
import { progressWords, runTime, settled, stalledWords, statusClass, statusWord, target } from "./words"

const STATUSES = ["queued", "in_progress", "completed", "failed", "canceled"]
const PAGE = 50

function RunRow({ run, now }: { run: Run; now: number }) {
  const to = useTo()
  const live = !settled(run)
  const line = run.status === "failed" || run.status === "canceled" ? run.reason?.message : live ? progressWords(run) : ""
  return (
    <li className={`runrow ${run.stalled ? "stalled" : ""}`}>
      <Link to={to(`runs/${run.number}`)}>
        <span className="line">
          <span className={`dot ${statusClass(run)}`} />
          <b>#{run.number}</b>
          <span className="nm">{target(run)}</span>
          <span className="sp" />
          <small>{statusWord(run)}</small>
          {run.machine && <small>{run.machine}</small>}
          <small>{runTime(run, now)}</small>
        </span>
        {line && <small className="under">{line}</small>}
        {run.stalled && <small className="under warn">{stalledWords(run.stalled, now)}</small>}
        {live && run.progress?.done_fraction != null && <Bar fraction={run.progress.done_fraction} />}
      </Link>
    </li>
  )
}

/** The runs, live: this project's, or every project's with `?all`; by machine and status. */
export function Runs() {
  const { client } = useConnection()
  const to = useTo()
  const id = useProjectId()
  const [params, setParams] = useSearchParams()
  const [limit, setLimit] = useState(PAGE)
  const all = !id || params.has("all")
  const status = params.get("status") ?? ""
  const machine = params.get("machine") ?? ""
  const { data, error } = useQuery(runsQuery(client, { project: all ? undefined : id, status, machine }, limit))
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
        {id && (
          <label className="check">
            <input type="checkbox" checked={all} onChange={(event) => set("all", event.target.checked ? "" : null)} />
            Every project
          </label>
        )}
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
          <RunRow key={run.number} run={run} now={now} />
        ))}
      </ul>
      {data && !runs.length && (
        <p className="muted">
          {status || machine ? "No runs match." : all ? "No runs yet." : "No runs in this project yet."}
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
