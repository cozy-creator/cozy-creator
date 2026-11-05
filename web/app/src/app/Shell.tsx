import { useQuery, useQueryClient } from "@tanstack/react-query"
import { useCallback, useEffect, useRef, useState } from "react"
import { Link, Outlet, useLocation, useMatch, useMatches, useNavigate } from "react-router-dom"
import { TRIES } from "../api/client"
import { projectQuery, projectsQuery } from "../api/queries"
import { CanvasHost } from "../canvas/CanvasHost"
import { warningsOf } from "../canvas/graph"
import { noHash } from "../canvas/text"
import { MachineTabs } from "../panels/machines/MachineTabs"
import { RunsLink } from "../panels/runs/RunsLink"
import type { ProjectRow } from "../types"
import { Icon } from "../ui/Icon"
import { Notes } from "../ui/NoteList"
import { useSignal } from "../ui/signal"
import { setTheme, theme } from "../ui/theme"
import { useConnection } from "./Connection"

export function useProjectId(): string | null {
  return useMatch("/projects/:id/*")?.params.id ?? null
}

/** A path under the current project, or under nothing. */
export function useTo() {
  const id = useProjectId()
  return (rest = "") => (id ? `/projects/${id}${rest && `/${rest}`}` : `/${rest}`)
}

/** A few tries in, it isn't a blip. Nothing restarts Calcifer on Windows, and `cozy open` does anywhere. A pod's Calcifer is its own. */
const SAYS_HOW = 5

function Connectivity() {
  const { stream, attempt, bridge, remote } = useConnection()
  const health = useSignal(stream.health)
  const trying = useSignal(attempt)
  const version = useSignal(bridge.newVersion)
  return (
    <>
      {health.state === "reconnecting" && (
        <span className="pill warn">
          Reconnecting, attempt {health.attempt}
          {!remote && health.attempt >= SAYS_HOW && (
            <>
              {" "}
              · run <code>cozy open</code>
            </>
          )}
        </span>
      )}
      {trying > 0 && <span className="pill warn">Trying again, attempt {trying}/{TRIES}</span>}
      {version && (
        <button type="button" className="pill warn" onClick={() => location.reload()}>
          Calcifer is now {version}: Reload
        </button>
      )}
    </>
  )
}

/** How many projects have a tab. The rest are behind +. */
const TABS = 6

/** The project open, and those used lately, each staying where it is so a tab doesn't jump from under the pointer. What's new takes the end. */
function useTabs(id: string | null, rows: ProjectRow[]): ProjectRow[] {
  const order = useRef<string[]>([])
  const known = new Map(rows.map((row) => [row.id, row]))
  order.current = [...order.current.filter((held) => known.has(held)), ...rows.map((row) => row.id).filter((held) => !order.current.includes(held))]
  const shown = order.current.slice(0, TABS)
  if (id && known.has(id) && !shown.includes(id)) shown[shown.length - 1] = id
  return shown.flatMap((held) => known.get(held) ?? [])
}

function ProjectTabs({ id }: { id: string | null }) {
  const { client } = useConnection()
  const to = useTo()
  const { data } = useQuery({ ...projectQuery(client, id ?? ""), enabled: Boolean(id) })
  const { data: list } = useQuery(projectsQuery(client))
  const tabs = useTabs(id, (list?.projects ?? []).filter((row) => !row.missing))
  const saidOfFile = data ? warningsOf(data).ofProject : []
  const name = data?.path.split(/[/\\]/).filter(Boolean).pop()
  return (
    <>
      <nav className="ptabs" aria-label="Projects">
        {tabs.map((tab) => (
          <Link key={tab.id} className={`ptab ${tab.id === id ? "on" : ""}`} to={`/projects/${tab.id}`} title={tab.path} aria-current={tab.id === id ? "page" : undefined}>
            {tab.name}
          </Link>
        ))}
        {!tabs.some((tab) => tab.id === id) && <span className="ptab on">{name ?? (id ? "…" : "untitled")}</span>}
        <Link className="ptab more" to={to("projects")} title="All projects" aria-label="All projects">
          <Icon name="plus" />
        </Link>
      </nav>
      {saidOfFile.length > 0 && (
        <Link className="sum ood" to={to("yaml")} title={noHash(saidOfFile.join("\n"))}>
          !
        </Link>
      )}
    </>
  )
}

function ThemeSwitch() {
  const now = useSignal(theme)
  return (
    <button type="button" className="tb" onClick={() => setTheme(now === "dark" ? "paper" : "dark")}>
      {now === "dark" ? "Paper" : "Dark"}
    </button>
  )
}

/** All but the canvas, a click away. */
function MainMenu({ id }: { id: string | null }) {
  const { client } = useConnection()
  const to = useTo()
  const held = useRef<HTMLDetailsElement>(null)
  const shut = () => held.current?.removeAttribute("open")
  useEffect(() => {
    const away = (event: Event) => {
      if (!held.current?.contains(event.target as globalThis.Node)) shut()
    }
    window.addEventListener("pointerdown", away)
    return () => window.removeEventListener("pointerdown", away)
  }, [])
  return (
    <details className="main-menu" ref={held}>
      <summary className="tb" aria-label="Menu">
        <Icon name="menu" />
      </summary>
      <nav onClick={shut}>
        <RunsLink to={to("runs")} />
        <Link className="tb" to={to("machines")}>
          Machines
        </Link>
        <Link className="tb" to={to("packages")}>
          Library
        </Link>
        {!client.transport.channel && (
          <Link className="tb" to={to("account")}>
            Account
          </Link>
        )}
        {id && (
          <Link className="tb" to={to("yaml")}>
            project.yaml
          </Link>
        )}
        <ThemeSwitch />
      </nav>
    </details>
  )
}

export function Shell() {
  const { remote } = useConnection()
  const id = useProjectId()
  const to = useTo()
  const navigate = useNavigate()
  const location = useLocation()
  const matches = useMatches()
  const [slot, setSlot] = useState<HTMLElement | null>(null)
  // The first import on an empty canvas makes a project: the same canvas stays mounted, so nothing on it is lost.
  const adopted = useRef(new Map<string, string>())
  const hostKey = (id && adopted.current.get(id)) ?? id ?? "empty"
  const queries = useQueryClient()

  const adopt = useCallback(
    (project: string) => {
      adopted.current.set(project, "empty")
      void queries.invalidateQueries({ queryKey: ["projects"] })
      navigate(`/projects/${project}${location.pathname.replace(/^\/$/, "")}`, { replace: true })
    },
    [navigate, location.pathname, queries],
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
        <ProjectTabs id={id} />
        <MachineTabs to={to} />
        {remote && (
          <Link className="pill warn" to={to("machines")}>
            On {remote}
          </Link>
        )}
        <Connectivity />
        <span className="gap" />
        <span className="canvas-controls" ref={setSlot} />
        <MainMenu id={id} />
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
