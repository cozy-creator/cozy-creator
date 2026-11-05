import { useQuery } from "@tanstack/react-query"
import { Link } from "react-router-dom"
import { machinesQuery, runsQuery } from "../../api/queries"
import { useConnection } from "../../app/Connection"
import type { Machine, RunList } from "../../types"
import { gpusWords, stableName, stateClass, stateWords } from "./words"

const on = (list: RunList | undefined, machine: Machine) => list?.runs.filter((run) => run.machine === machine.name).length ?? 0

/** Each rental, top right: its name, then what it's running of all it holds, read once and kept by the stream. */
export function MachineTabs({ to }: { to: (rest?: string) => string }) {
  const { client } = useConnection()
  const { data } = useQuery(machinesQuery(client))
  const running = useQuery(runsQuery(client, { status: "in_progress" }, 100))
  const queued = useQuery(runsQuery(client, { status: "queued" }, 100))
  const rentals = data?.machines.filter((machine) => machine.kind === "rental" && machine.recorded !== false && machine.state !== "ended") ?? []
  if (!rentals.length) return null
  return (
    <nav className="mtabs" aria-label="Rented machines">
      {rentals.map((machine) => {
        const going = on(running.data, machine)
        const total = going + on(queued.data, machine)
        return (
          <Link
            key={machine.name}
            className="mtab"
            to={to(`machines/${encodeURIComponent(stableName(machine))}`)}
            title={`${gpusWords(machine)}, ${stateWords(machine, Date.now())}: ${going} running of ${total}`}
          >
            <span className={`dot ${stateClass(machine)}`} />
            <b>{machine.name}</b>
            <span className="load">
              {going} / {total}
            </span>
          </Link>
        )
      })}
    </nav>
  )
}
