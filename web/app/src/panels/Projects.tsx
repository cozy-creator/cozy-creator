import { useQuery, useQueryClient } from "@tanstack/react-query"
import { useEffect, useState } from "react"
import { Link } from "react-router-dom"
import { projectsQuery } from "../api/queries"
import type { KnownProject } from "../api/calcifer"
import { projectPath } from "../app/project"
import { useConnection } from "../app/Connection"
import { say } from "../ui/notes"
import { signal, useSignal } from "../ui/signal"
import { DrawerFrame } from "./Drawer"

/** When this page opened each project: its use, before the index read next says it. */
const usedHere = signal<ReadonlyMap<string, number>>(new Map())

export function opened(id: string) {
  usedHere.set(new Map(usedHere.get()).set(id, Date.now()))
}

/** Most recently used first (Paul, 11-16), the tabs and the Projects list alike. */
export function useByUse<T extends Pick<KnownProject, "id" | "used_at_ms">>(rows: T[]): T[] {
  const here = useSignal(usedHere)
  const at = (row: T) => Math.max(row.used_at_ms, here.get(row.id) ?? 0)
  return [...rows].sort((a, b) => at(b) - at(a))
}

/** Past this many, a field searches them by name or folder. */
const FINDS = 8

/** Every project, most recently used first, and a search past a few (Paul, 11-16). The bar's + makes a new one. */
export function Projects() {
  const { client } = useConnection()
  const queries = useQueryClient()
  const { data, error } = useQuery(projectsQuery(client))
  const [find, setFind] = useState("")

  useEffect(() => {
    void queries.invalidateQueries({ queryKey: ["projects"] })
  }, [queries])
  useEffect(() => {
    if (error) say("Couldn't list projects", error)
  }, [error])

  const all = useByUse(data?.projects ?? [])
  const finds = all.length > FINDS
  const words = finds ? find.trim().toLowerCase() : ""
  const listed = all.filter((project) => !words || project.name.toLowerCase().includes(words) || project.path.toLowerCase().includes(words))

  return (
    <DrawerFrame title="Projects">
      {finds && (
        <input
          className="field wide find"
          aria-label="Search projects"
          placeholder="Search by name or folder"
          spellCheck={false}
          autoFocus
          value={find}
          onChange={(event) => setFind(event.target.value)}
        />
      )}
      <ul className="list">
        {listed.map((project) => (
          <li key={project.id}>
            {/* Its name; its folder on hover (cl-958: no paths on screen until asked). */}
            <Link to={projectPath(project)} title={project.path}>
              <b>{project.name}</b>
            </Link>
          </li>
        ))}
      </ul>
      {data && !all.length && <p className="muted">No projects yet. Press + to make one, or drop a file on the canvas.</p>}
      {words && !listed.length && <p className="muted">No project matches that name or folder.</p>}
    </DrawerFrame>
  )
}
