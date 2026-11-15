import { useQuery, useQueryClient } from "@tanstack/react-query"
import { useEffect, useState } from "react"
import { Link, useLocation, useNavigate } from "react-router-dom"
import { projectsQuery } from "../api/queries"
import { projectPath, rememberProject } from "../app/project"
import { useConnection } from "../app/Connection"
import { say } from "../ui/notes"
import { DrawerFrame } from "./Drawer"
import { ProjectCreate } from "./ProjectCreate"
import type { Opened } from "../api/calcifer"

/** Past this many, a field searches them by name or folder. */
const FINDS = 8

/** What the tabs ask of this drawer: the projects with no tab first, or a new one's name. */
export interface ProjectsAsk {
  first?: string[]
  new?: boolean
}

export function Projects() {
  const { client } = useConnection()
  const queries = useQueryClient()
  const navigate = useNavigate()
  const { key, state } = useLocation()
  const ask: ProjectsAsk = state ?? {}
  const { data, error } = useQuery(projectsQuery(client))
  const [find, setFind] = useState("")

  useEffect(() => {
    void queries.invalidateQueries({ queryKey: ["projects"] })
  }, [queries])
  useEffect(() => {
    if (error) say("Couldn't list projects", error)
  }, [error])

  const all = data?.projects ?? []
  const finds = all.length > FINDS
  const words = finds ? find.trim().toLowerCase() : ""
  const first = new Set(ask.first)
  const listed = all
    .filter((project) => !words || project.name.toLowerCase().includes(words) || project.path.toLowerCase().includes(words))
    .sort((a, b) => Number(first.has(b.id)) - Number(first.has(a.id)))

  const created = (made: Opened) => {
    void queries.invalidateQueries({ queryKey: ["projects"] })
    for (const line of made.said ?? []) say(line)
    rememberProject(queries, made)
    navigate(projectPath(made))
  }

  return (
    <DrawerFrame title="Projects">
      <ProjectCreate key={key} autoFocus={ask.new} onCreated={created} />
      {finds && (
        <input
          className="field wide find"
          aria-label="Search projects"
          placeholder="Search projects, or a folder"
          spellCheck={false}
          value={find}
          onChange={(event) => setFind(event.target.value)}
        />
      )}
      <ul className="list">
        {listed.map((project, at) => (
          <li key={project.id} className={at > 0 && first.has(listed[at - 1].id) && !first.has(project.id) ? "tabbed" : undefined}>
            {/* Its name; its folder on hover (cl-958: no paths on screen until asked). */}
            <Link to={projectPath(project)} title={project.path}>
              <b>{project.name}</b>
            </Link>
          </li>
        ))}
      </ul>
      {data && !all.length && <p className="muted">Drop a file on the canvas to start one.</p>}
      {words && !listed.length && <p className="muted">No project's name or folder has that.</p>}
    </DrawerFrame>
  )
}
