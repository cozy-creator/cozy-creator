import { useQuery } from "@tanstack/react-query"
import { CopyError, runText } from "../../ui/CopyError"
import { Link, Navigate, useParams } from "react-router-dom"
import { maintenanceRunFullQuery, maintenanceRunQuery, runFullQuery, runQuery } from "../../api/queries"
import { useConnection } from "../../app/Connection"
import { useTo } from "../../app/Shell"
import { useProjectHref, useProjectId } from "../../app/project"
import { Bar, Confirm, Failure, KV, useAsk, useNow } from "../common"
import { DrawerFrame } from "../Drawer"
import { masked, usd, when } from "../format"
import { Outputs } from "./Outputs"
import { Report } from "./Report"
import { RunLog } from "./RunLog"
import { eta, progressWords, runName, runTime, settled, stalledWords, statusClass, statusWord, target } from "./words"

/** A run's name, `SE-5` (cl-987): a link from anywhere. */
const NAMED = /^[A-Za-z][A-Za-z0-9]{1,9}-\d+$/

/** One run, by its number here or by its name. */
export function RunDrawer() {
  const param = useParams().number ?? ""
  return NAMED.test(param) ? <NamedRun name={param.toUpperCase()} /> : <NumberedRun />
}

/** A run by its name: Calcifer finds its project, and the drawer goes where the run is. */
function NamedRun({ name }: { name: string }) {
  const { client } = useConnection()
  const to = useTo()
  const { data: run, error } = useQuery({ queryKey: ["named-run", name], queryFn: ({ signal }) => client.maintenanceRun(name, signal) })
  const href = useProjectHref(run?.project_id, `runs/${run?.number}`)
  if (run && !run.project_id) return <Navigate replace to={`/runs/${run.number}`} />
  if (href) return <Navigate replace to={href} />
  return (
    <DrawerFrame title={`Run ${name}`} back={to("runs")}>
      <Failure error={error} lead="Couldn't find the run" />
    </DrawerFrame>
  )
}

/** One run: its state, progress, log, outputs, report, cancel, and running it again. */
function NumberedRun() {
  const { client } = useConnection()
  const to = useTo()
  const number = Number(useParams().number)
  const project = useProjectId()
  const { data: run, error } = useQuery({ ...(project ? runQuery(client, project, number) : maintenanceRunQuery(client, number)), enabled: Number.isSafeInteger(number) })
  const { data: full } = useQuery({ ...(project ? runFullQuery(client, project, number, run?.settled_at_ms ?? 0) : maintenanceRunFullQuery(client, number, run?.settled_at_ms ?? 0)), enabled: Boolean(run) })
  const again = useProjectHref(run?.project_id, `new?again=${number}`)
  const live = Boolean(run && !settled(run))
  const now = useNow(live)
  const cancel = useAsk()

  if (!run) {
    return (
      <DrawerFrame title={`Run ${number}`} back={to("runs")}>
        <Failure error={error} lead="Couldn't read the run" />
      </DrawerFrame>
    )
  }

  const asked = full?.request
  const cancelled = run.cancel_requested
  return (
    <DrawerFrame
      title={`Run ${runName(run)}`}
      back={to("runs")}
      actions={
        <>
          {live && (
            <Confirm
              label={cancelled ? "Canceling…" : "Cancel"}
              disabled={cancelled || cancel.busy}
              says={`Cancel run ${runName(run)}? What it hasn't finished is lost. Its machine goes on.`}
              yes="Cancel it"
              onYes={() => void cancel.run(() => project ? client.cancelRun({ project, run: run.id }) : client.cancelMaintenanceRun(run.id))}
            />
          )}
          {again && asked && (
            <Link className="tb" to={again}>
              Run again
            </Link>
          )}
        </>
      }
    >
      <div className="run-head">
        <span className={`dot ${statusClass(run)}`} />
        <b className="nm">{target(run)}</b>
        <span className="sp" />
        <small>{statusWord(run)}</small>
      </div>
      <Failure error={cancel.error} lead="Couldn't cancel it" />
      {run.stalled && (
        <p className="failure" role="alert">
          {stalledWords(run.stalled, now)}. Nothing ends it but you.
          {run.stalled.usd_micros_per_hour != null && <span className="remedy">{run.stalled.machine} bills {usd(run.stalled.usd_micros_per_hour)}/h meanwhile.</span>}
        </p>
      )}
      {run.reason && (
        <div className="failure error-text" role="alert">
          <span>{run.reason.message}</span>
          <CopyError text={runText(run, run.reason)} />
          {run.reason.detail && (
            <details>
              <summary>What it said</summary>
              <pre className="log">{run.reason.detail}</pre>
            </details>
          )}
        </div>
      )}
      {live && run.progress?.done_fraction != null && <Bar fraction={run.progress.done_fraction} />}
      {live && <p className="muted">{progressWords(run)}</p>}
      <KV
        rows={[
          ["Machine", run.machine && <Link className="more" to={to(`machines/${encodeURIComponent(run.machine)}`)}>{run.machine}</Link>],
          ["Waits", run.queue_position == null ? run.waiting : `${run.queue_position === 1 ? "next" : `${run.queue_position - 1} ahead`}${run.waiting ? `: ${run.waiting}` : ""}`],
          ["Left", live ? eta(run) : null],
          ["Time", runTime(run, now)],
          ["Asked", when(run.created_at_ms)],
        ]}
      />
      {/* A mention or input its function can't use was left out (cl-944), or it ran from a dev home's packages checkout (xs-195): a note,
          nothing to act on. */}
      {run.warnings?.map((warning) => (
        <p className={warning.code === "mention_unused" || warning.code === "from_source" ? "muted" : "warn"} key={`${warning.code}:${warning.message}`}>
          {warning.message}
        </p>
      ))}
      {run.adjustments?.map((adjustment) =>
        adjustment.kind === "ignored" ? (
          <p className="muted" key={adjustment.field}>
            {adjustment.field}: {adjustment.requested} left out. {adjustment.reason}
          </p>
        ) : (
          <p className="warn" key={adjustment.field}>
            {adjustment.field}: {adjustment.requested} became {adjustment.applied}. {adjustment.reason}
          </p>
        ),
      )}
      <Outputs run={run} project={project} />
      {asked && (
        <details className="sect">
          <summary>What it was asked</summary>
          <pre className="yaml">{masked(asked.input)}</pre>
          {asked.seed !== undefined && <p className="muted">Seed {asked.seed}</p>}
        </details>
      )}
      <RunLog run={run} project={project} />
      {full?.report && Boolean(full.report.measured?.length) && (
        <details className="sect">
          <summary>Report</summary>
          <Report report={full.report} />
        </details>
      )}
      {settled(run) && run.result !== undefined && run.result !== null && (
        <details className="sect">
          <summary>Result</summary>
          <pre className="yaml">{masked(run.result)}</pre>
        </details>
      )}
    </DrawerFrame>
  )
}
