import { useQuery } from "@tanstack/react-query"
import { useCallback, useEffect, useRef, useState } from "react"
import { Link, Outlet, useLocation, useMatch, useMatches, useNavigate } from "react-router-dom"
import { TRIES } from "../api/client"
import { projectQuery } from "../api/queries"
import { CanvasHost } from "../canvas/CanvasHost"
import { Notes } from "../ui/Notes"
import { useSignal } from "../ui/signal"
import { useConnection } from "./Connection"

export function useProjectId(): string | null {
  return useMatch("/projects/:id/*")?.params.id ?? null
}

/** A path under the current project, or under nothing. */
export function useTo() {
  const id = useProjectId()
  return (rest = "") => (id ? `/projects/${id}${rest && `/${rest}`}` : `/${rest}`)
}

function Connectivity() {
  const { stream, attempt, bridge } = useConnection()
  const health = useSignal(stream.health)
  const trying = useSignal(attempt)
  const version = useSignal(bridge.newVersion)
  return (
    <>
      {health.state === "reconnecting" && <span className="pill warn">Reconnecting, attempt {health.attempt}</span>}
      {trying > 0 && <span className="pill warn">Trying again, attempt {trying}/{TRIES}</span>}
      {version && (
        <button type="button" className="pill warn" onClick={() => location.reload()}>
          Calcifer is now {version}: Reload
        </button>
      )}
    </>
  )
}

function ProjectName({ id }: { id: string | null }) {
  const { client } = useConnection()
  const to = useTo()
  const { data } = useQuery({ ...projectQuery(client, id ?? ""), enabled: Boolean(id) })
  const name = data?.path.split(/[/\\]/).filter(Boolean).pop() ?? (id ? "…" : "untitled")
  const assets = data?.assets ?? []
  const making = assets.filter((asset) => asset.standing === "making").length
  const late = assets.filter((asset) => asset.standing === "out_of_date").length
  return (
    <>
      <Link className="proj" to={to("projects")} title="Projects">
        {name}
      </Link>
      {data && <span className="path">{data.path}</span>}
      {making > 0 && <span className="sum mak">{making} making</span>}
      {late > 0 && <span className="sum ood">{late} out of date</span>}
    </>
  )
}

export function Shell() {
  const id = useProjectId()
  const to = useTo()
  const navigate = useNavigate()
  const location = useLocation()
  const matches = useMatches()
  const [slot, setSlot] = useState<HTMLElement | null>(null)
  // The first import on an empty canvas makes a project: the same canvas stays mounted, so nothing on it is lost.
  const adopted = useRef(new Map<string, string>())
  const hostKey = (id && adopted.current.get(id)) ?? id ?? "empty"

  const adopt = useCallback(
    (project: string) => {
      adopted.current.set(project, "empty")
      navigate(`/projects/${project}${location.pathname.replace(/^\/$/, "")}`, { replace: true })
    },
    [navigate, location.pathname],
  )

  useEffect(() => {
    // A file dropped outside the canvas must not make the browser open it.
    const stop = (event: DragEvent) => {
      if (event.dataTransfer?.types.includes("Files")) event.preventDefault()
    }
    window.addEventListener("dragover", stop)
    window.addEventListener("drop", stop)
    return () => {
      window.removeEventListener("dragover", stop)
      window.removeEventListener("drop", stop)
    }
  }, [])

  const handle = matches[matches.length - 1]?.handle as { drawer?: "normal" | "wide" } | undefined
  return (
    <div className="shell" data-drawer={handle?.drawer ?? "none"}>
      <header className="top">
        <ProjectName id={id} />
        <Connectivity />
        <span className="gap" />
        <span className="canvas-controls" ref={setSlot} />
        {id && (
          <Link className="tb" to={to("yaml")}>
            project.yaml
          </Link>
        )}
        <Link className="tb" to={to("runs")}>
          Runs
        </Link>
        <Link className="tb" to={to("machines")}>
          Machines
        </Link>
        <Link className="tb" to={to("packages")}>
          Library
        </Link>
        <Link className="tb" to={to("account")}>
          Account
        </Link>
      </header>
      <CanvasHost key={hostKey} projectId={id} slot={slot} onAdopt={adopt} />
      {handle?.drawer && (
        <aside className="drawer">
          <Outlet />
        </aside>
      )}
      {!handle?.drawer && <Outlet />}
      <Notes />
    </div>
  )
}
