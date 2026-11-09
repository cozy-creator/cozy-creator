import { useQuery } from "@tanstack/react-query"
import { Link } from "react-router-dom"
import { runsQuery } from "../../api/queries"
import { useConnection } from "../../app/Connection"
import type { RunList } from "../../api/calcifer"

const going = (list?: RunList) => (list?.runs.length ?? 0) + (list?.older ?? 0)

/** Runs in the top bar, with how many are going: read once, then kept by `run.updated`. */
export function RunsLink({ to }: { to: string }) {
  const { client } = useConnection()
  const running = useQuery(runsQuery(client, { status: "in_progress" }, 100))
  const queued = useQuery(runsQuery(client, { status: "queued" }, 100))
  const count = going(running.data) + going(queued.data)
  return (
    <Link className="tb" to={to}>
      Runs
      {count > 0 && <span className="count">{count}</span>}
    </Link>
  )
}
