import { useQuery, useQueryClient } from "@tanstack/react-query"
import { type FormEvent, useEffect, useState } from "react"
import { Link, useNavigate } from "react-router-dom"
import { projectsQuery } from "../api/queries"
import { useConnection } from "../app/Connection"
import { say } from "../ui/notes"
import { DrawerFrame } from "./Drawer"

export function Projects() {
  const { client } = useConnection()
  const queries = useQueryClient()
  const navigate = useNavigate()
  const { data, error } = useQuery(projectsQuery(client))
  const [name, setName] = useState("")

  useEffect(() => {
    void queries.invalidateQueries({ queryKey: ["projects"] })
  }, [queries])
  useEffect(() => {
    if (error) say("Couldn't list projects", error)
  }, [error])

  const create = async (event: FormEvent) => {
    event.preventDefault()
    try {
      const made = await client.newProject(name.trim() || "untitled", `project-${crypto.randomUUID()}`)
      void queries.invalidateQueries({ queryKey: ["projects"] })
      navigate(`/projects/${made.id}`)
    } catch (failure) {
      say("Couldn't make the project", failure)
    }
  }

  return (
    <DrawerFrame title="Projects">
      <form className="row" onSubmit={create}>
        <input
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
      <ul className="list">
        {data?.projects.map((project) => (
          <li key={project.id}>
            <Link to={`/projects/${project.id}`} title={project.path}>
              <b>{project.name}</b>
              {project.missing && <small>Its folder is gone</small>}
            </Link>
          </li>
        ))}
      </ul>
      {data && !data.projects.length && <p className="muted">Drop a file on the canvas to start one.</p>}
    </DrawerFrame>
  )
}
