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

/** A machine's checkbox, beside it wherever it's listed (Paul 11-12: "Just make it a checkbox"): checked, the open project's renders
 *  go here too, and it's green; several can be checked, the project's preferred machines; none is automatic. */
export function RenderCheck({ name, called = name }: { name: string; called?: string }) {
  const { project, chosen, toggle } = useRenderMachines()
  if (!project) return null
  return (
    <input
      type="checkbox"
      className="render-check"
      aria-label={`Render on ${called}`}
      title="This project renders here (any checked machine, whichever finishes first)"
      checked={chosen.includes(name)}
      onChange={() => toggle(name)}
    />
  )
}
