import { useQuery } from "@tanstack/react-query"
import { Link, useParams } from "react-router-dom"
import { runFullQuery, runQuery } from "../../api/queries"
import { useConnection } from "../../app/Connection"
import { useTo } from "../../app/Shell"
import { useProjectHref } from "../../app/project"
import { Bar, Confirm, Failure, KV, useAsk, useNow } from "../common"
import { DrawerFrame } from "../Drawer"
import { masked, usd, when } from "../format"
import { Outputs } from "./Outputs"
import { Report } from "./Report"
import { RunLog } from "./RunLog"
import { eta, progressWords, runTime, settled, stalledWords, statusClass, statusWord, target } from "./words"

/** One run: its state, progress, log, outputs, report, cancel, and running it again. */
export function RunDrawer() {
  const { client } = useConnection()
  const to = useTo()
  const number = Number(useParams().number)
  const { data: run, error } = useQuery({ ...runQuery(client, number), enabled: Number.isSafeInteger(number) })
  const { data: full } = useQuery({ ...runFullQuery(client, number, run?.settled_at_ms ?? 0), enabled: Boolean(run) })
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
      title={`Run ${run.number}`}
      back={to("runs")}
      actions={
        <>
          {live && (
            <Confirm
              label={cancelled ? "Canceling…" : "Cancel"}
              disabled={cancelled || cancel.busy}
              says={`Cancel run ${run.number}? What it hasn't finished is lost. Its machine goes on.`}
              yes="Cancel it"
              onYes={() => void cancel.run(() => client.cancelRun(run.id))}
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
        <div className="failure" role="alert">
          <span>{run.reason.message}</span>
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
      {run.warnings?.map((warning) => (
        <p className="warn" key={`${warning.code}:${warning.message}`}>
          {warning.message}
        </p>
      ))}
      {run.adjustments?.map((adjustment) => (
        <p className="warn" key={adjustment.field}>
          {adjustment.field}: {adjustment.requested} became {adjustment.applied}. {adjustment.reason}
        </p>
      ))}
      <Outputs run={run} />
      {asked && (
        <details className="sect">
          <summary>What it was asked</summary>
          <pre className="yaml">{masked(asked.input)}</pre>
          {asked.seed !== undefined && <p className="muted">Seed {asked.seed}</p>}
        </details>
      )}
      <RunLog run={run} />
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
