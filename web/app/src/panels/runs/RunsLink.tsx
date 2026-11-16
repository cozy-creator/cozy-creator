import { useQuery } from "@tanstack/react-query"
import { Link } from "react-router-dom"
import { runsQuery } from "../../api/queries"
import { useProjectId } from "../../app/project"
import { useConnection } from "../../app/Connection"
import type { RunList } from "../../api/calcifer"

const going = (list?: RunList) => (list?.runs.length ?? 0) + (list?.older ?? 0)

/** Runs in the top bar, with how many are going: read once, then kept by `run.updated`. */
export function RunsLink({ to }: { to: string }) {
  const { client } = useConnection()
  const project = useProjectId()
  // Runs are a project's (cl-1028): with none open, none are counted.
  const running = useQuery({ ...runsQuery(client, project ?? "", { status: "in_progress" }, 100), enabled: Boolean(project) })
  const queued = useQuery({ ...runsQuery(client, project ?? "", { status: "queued" }, 100), enabled: Boolean(project) })
  const count = going(running.data) + going(queued.data)
  return (
    <Link className="tb" to={to}>
      Runs
      {count > 0 && <span className="count">{count}</span>}
    </Link>
  )
}
