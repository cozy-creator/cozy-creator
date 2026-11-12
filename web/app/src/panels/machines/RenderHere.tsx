import { useQuery, useQueryClient } from "@tanstack/react-query"
import { projectQuery } from "../../api/queries"
import { useConnection } from "../../app/Connection"
import { useProjectId } from "../../app/project"
import type { Project } from "../../types"
import { say } from "../../ui/notes"

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
    if (!id) return
    const now = queries.getQueryData<Project>(["project", id])?.machines ?? []
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
  }
  return { project: id, chosen, toggle }
}

/** A machine's toggle, on its chip top right and in its drawer: the open project's renders go here too, and pressed again, no longer.
 *  Several can be on: the project's preferred machines (Paul 11-12). */
export function RenderHere({ name }: { name: string }) {
  const { project, chosen, toggle } = useRenderMachines()
  if (!project) return null
  const on = chosen.includes(name)
  return (
    <button
      type="button"
      className={`tb render-here ${on ? "on" : ""}`}
      aria-pressed={on}
      title={on ? "This project renders here. Press again to stop." : "This project's renders go here, or to another green machine if it finishes first."}
      onClick={() => toggle(name)}
    >
      Render here
    </button>
  )
}
