import { useQuery, useQueryClient } from "@tanstack/react-query"
import { type FormEvent, useEffect, useRef, useState } from "react"
import { Link, useLocation, useNavigate } from "react-router-dom"
import { projectsQuery } from "../api/queries"
import { useConnection } from "../app/Connection"
import { say } from "../ui/notes"
import { DrawerFrame } from "./Drawer"

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
  const [name, setName] = useState("")
  const [find, setFind] = useState("")
  const naming = useRef<HTMLInputElement>(null)

  useEffect(() => {
    void queries.invalidateQueries({ queryKey: ["projects"] })
  }, [queries])
  useEffect(() => {
    if (error) say("Couldn't list projects", error)
  }, [error])
  useEffect(() => {
    if (ask.new) naming.current?.focus()
  }, [key, ask.new])

  const all = data?.projects ?? []
  const finds = all.length > FINDS
  const words = finds ? find.trim().toLowerCase() : ""
  const first = new Set(ask.first)
  const listed = all
    .filter((project) => !words || project.name.toLowerCase().includes(words) || project.path.toLowerCase().includes(words))
    .sort((a, b) => Number(first.has(b.id)) - Number(first.has(a.id)))

  const create = async (event: FormEvent) => {
    event.preventDefault()
    try {
      const made = await client.newProject(name.trim() || "untitled", `project-${crypto.randomUUID()}`)
      void queries.invalidateQueries({ queryKey: ["projects"] })
      for (const line of made.said ?? []) say(line)
      navigate(`/projects/${made.id}`)
    } catch (failure) {
      say("Couldn't make the project", failure)
    }
  }

  return (
    <DrawerFrame title="Projects">
      <form className="row" onSubmit={create}>
        <input
          ref={naming}
          className="field"
          aria-label="New project's name"
          placeholder="New project's name"
          spellCheck={false}
          value={name}
          onChange={(event) => setName(event.target.value)}
        />
        <button type="submit" className="tb">
          New
        </button>
      </form>
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
            <Link to={`/projects/${project.id}`} title={project.path}>
              <b>{project.name}</b>
              <small className="project-path">{project.missing ? "Its folder is gone" : project.path}</small>
            </Link>
          </li>
        ))}
      </ul>
      {data && !all.length && <p className="muted">Drop a file on the canvas to start one.</p>}
      {words && !listed.length && <p className="muted">No project's name or folder has that.</p>}
    </DrawerFrame>
  )
}
