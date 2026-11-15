import { useQuery, useQueryClient } from "@tanstack/react-query"
import { type ReactNode, useState } from "react"
import type { Machine } from "../../api/calcifer"
import { projectQuery } from "../../api/queries"
import { useConnection } from "../../app/Connection"
import { useProjectId } from "../../app/project"
import type { Project } from "../../types"
import { say } from "../../ui/notes"
import { usd } from "../format"
import { called, gpusWords, stateWords } from "./words"

/** Only the latest ask's answer is kept: an earlier one landing after it would undo it. */
let asked = 0

/**
 * The open project's machines (`cozy machine use`, cl-873): its renders go to whichever of them finishes first, and with none they're placed
 * automatically. A press shows at once; Calcifer's answer, or the CLI's change on the stream, is what stays.
 */
export function useRenderMachines() {
  const { client, stream } = useConnection()
  const queries = useQueryClient()
  const id = useProjectId()
  const { data } = useQuery({ ...projectQuery(client, stream, id ?? ""), enabled: Boolean(id) })
  const chosen = data?.machines ?? []
  const keep = (machines: string[]) => id && queries.setQueryData<Project>(["project", id], (state) => state && { ...state, machines })
  const toggle = (name: string) => {
    const held = id ? queries.getQueryData<Project>(["project", id]) : undefined
    // Not read yet, what it holds isn't known: a click would replace it.
    if (!id || !held) return false
    const now = held.machines ?? []
    const next = now.includes(name) ? now.filter((other) => other !== name) : [...now, name]
    const mine = ++asked
    keep(next)
    void client.setMachines(id, next).then(
      ({ machines }) => mine === asked && keep(machines),
      (error) => {
        if (mine === asked) void queries.invalidateQueries({ queryKey: ["project", id] })
        say("Couldn't choose where this project renders", error)
      },
    )
    return true
  }
  return { project: id, known: data !== undefined, chosen, toggle }
}

/** A machine's checkbox, beside it wherever it's listed (Paul 11-12: "Just make it a checkbox"): checked, the open project's renders
 *  go here too, and it's green; several can be checked, the project's preferred machines; none is automatic. */
export function RenderCheck({ name, called = name }: { name: string; called?: string }) {
  const { project, known, chosen, toggle } = useRenderMachines()
  const on = chosen.includes(name)
  // The press, shown in its own render: the cache tells its readers a task later, and a controlled box springs back meanwhile.
  const [press, setPress] = useState<{ from: boolean; to: boolean } | null>(null)
  if (press && press.from !== on) setPress(null)
  if (!project) return null
  return (
    <label className="render-prefer" title="This project renders here (any checked machine, whichever finishes first)">
      <input
        type="checkbox"
        className="render-check"
        disabled={!known}
        aria-label={`Render on ${called}`}
        checked={press ? press.to : on}
        onChange={(event) => {
          if (toggle(name)) setPress({ from: on, to: event.target.checked })
        }}
      />
      Prefer
    </label>
  )
}

/** A machine's full spec, a row of its own that wraps rather than cuts (Paul 11-12): its name, its GPUs and their memory, what it costs an
 *  hour, its state. */
export function Spec({ machine, now, name = true }: { machine: Machine; now: number; name?: boolean }) {
  const gpus = gpusWords(machine)
  return (
    <div className="spec">
      {name && <b>{called(machine)}</b>}
      {gpus && <span>{gpus}</span>}
      {machine.rental && <span>{usd(machine.rental.usd_micros_per_hour)}/h</span>}
      <span>{stateWords(machine, now)}</span>
    </div>
  )
}

/** The row under a machine's spec: its check, and its Details. */
export function Checks({ machine, children }: { machine: Machine; children: ReactNode }) {
  return (
    <div className="checks">
      <RenderCheck name={machine.name} called={called(machine)} />
      {children}
    </div>
  )
}
