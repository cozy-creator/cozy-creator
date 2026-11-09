import { useQuery, useQueryClient } from "@tanstack/react-query"
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react"
import { flushSync } from "react-dom"
import { Link, Outlet, useLocation, useMatch, useMatches, useNavigate } from "react-router-dom"
import { TRIES } from "../api/client"
import { projectQuery, projectsQuery } from "../api/queries"
import { CanvasHost } from "../canvas/CanvasHost"
import { warningsOf } from "../canvas/graph"
import { noHash } from "../canvas/text"
import { MachineTabs } from "../panels/machines/MachineTabs"
import type { ProjectsAsk } from "../panels/Projects"
import { RunsLink } from "../panels/runs/RunsLink"
import type { KnownProject } from "../api/calcifer"
import { Icon } from "../ui/Icon"
import { Notes } from "../ui/NoteList"
import { useSignal } from "../ui/signal"
import { landscape, setLandscape, setTheme, theme } from "../ui/theme"
import { useConnection } from "./Connection"

export function useProjectId(): string | null {
  return useMatch("/projects/:id/*")?.params.id ?? null
}

/** A path under the current project, or under nothing. */
export function useTo() {
  const id = useProjectId()
  return (rest = "") => (id ? `/projects/${id}${rest && `/${rest}`}` : `/${rest}`)
}

/** A few tries in, it isn't a blip. Nothing restarts Calcifer on Windows, and `cozy open` does anywhere. */
const SAYS_HOW = 5

/** Whether Calcifer answers, and what's waiting for it: said, and nothing waits on it. Gone once all of it is saved. */
function Connectivity() {
  const { stream, attempt, bridge, outbox } = useConnection()
  const health = useSignal(stream.health)
  const trying = useSignal(attempt)
  const owed = useSignal(outbox.owed)
  const version = useSignal(bridge.newVersion)
  const changes = `${owed.count} ${owed.count === 1 ? "change" : "changes"}`
  return (
    <>
      {health.state === "reconnecting" ? (
        <span className="pill warn" role="status">
          Calcifer offline{owed.count > 0 && `: ${changes} waiting`}
          {health.attempt >= SAYS_HOW && (
            <>
              {" "}
              · run <code>cozy open</code>
            </>
          )}
        </span>
      ) : owed.count > 0 && owed.slow ? (
        <span className="pill warn" role="status">
          {owed.held ? `Not saved: ${noHash(owed.held)}: ${changes} waiting` : `Saving ${changes}`}
        </span>
      ) : (
        trying > 0 && <span className="pill warn">Trying again, attempt {trying}/{TRIES}</span>
      )}
      {version && (
        <button type="button" className="pill warn" onClick={() => location.reload()}>
          Calcifer is now {version}: Reload
        </button>
      )}
    </>
  )
}

/** Projects in the order they took their tabs, each staying where it is so a tab doesn't jump from under the pointer. What's new takes the end. */
function useOrder(rows: KnownProject[]): KnownProject[] {
  const order = useRef<string[]>([])
  const known = new Map(rows.map((row) => [row.id, row]))
  order.current = [...order.current.filter((held) => known.has(held)), ...rows.map((row) => row.id).filter((held) => !order.current.includes(held))]
  return order.current.flatMap((held) => known.get(held) ?? [])
}

/** The first `count`, with the open project in the last place if it isn't one of them. */
function tabsOf(order: KnownProject[], id: string | null, count: number): KnownProject[] {
  const shown = order.slice(0, Math.max(count, 1))
  const open = order.find((row) => row.id === id)
  if (open && !shown.includes(open)) shown[shown.length - 1] = open
  return shown
}

/** As many tabs as fit the bar, the rest behind "N more". `room` is the bar's free space past them; a hidden copy of every tab says what each takes. */
function ProjectTabs({ id, room }: { id: string | null; room: HTMLElement | null }) {
  const { client } = useConnection()
  const to = useTo()
  const { data } = useQuery({ ...projectQuery(client, id ?? ""), enabled: Boolean(id) })
  const { data: list } = useQuery(projectsQuery(client))
  const order = useOrder((list?.projects ?? []).filter((row) => !row.missing))
  const [count, setCount] = useState(Number.POSITIVE_INFINITY)
  const strip = useRef<HTMLElement>(null)
  const sizes = useRef<HTMLDivElement>(null)
  const tabs = tabsOf(order, id, count)
  const hidden = order.filter((row) => !tabs.includes(row))
  const saidOfFile = data ? warningsOf(data).ofProject : []
  const name = data?.path.split(/[/\\]/).filter(Boolean).pop()
  // The open project before the list has it, or none.
  const lone = order.some((row) => row.id === id) ? null : (name ?? (id ? "…" : "untitled"))
  const named = `${id} ${lone} ${order.map((row) => `${row.id}:${row.name}`).join(" ")}`

  useLayoutEffect(() => {
    const held = strip.current
    const ghost = sizes.current
    if (!held || !ghost || !room) return
    const fit = () => {
      const style = getComputedStyle(held)
      const space = Math.min(held.getBoundingClientRect().width + room.getBoundingClientRect().width, Number.parseFloat(style.maxWidth) || Number.POSITIVE_INFINITY)
      const gap = Number.parseFloat(style.columnGap) || 0
      const widths = [...ghost.children].map((child) => child.getBoundingClientRect().width)
      const alone = lone === null ? [] : [widths.pop() ?? 0]
      const more = widths.pop() ?? 0
      const width = new Map(order.map((row, at) => [row, widths[at]]))
      const plus = held.lastElementChild?.getBoundingClientRect().width ?? 0
      const fits = (n: number) => {
        const shown = tabsOf(order, id, n)
        const items = [...shown.map((row) => width.get(row) ?? 0), ...alone, ...(shown.length < order.length ? [more] : []), plus]
        return Number.parseFloat(style.paddingLeft) + Number.parseFloat(style.paddingRight) + items.reduce((sum, each) => sum + each + gap, -gap) <= space
      }
      if (fits(order.length)) return order.length
      let n = 1
      while (n < order.length - 1 && fits(n + 1)) n++
      return n
    }
    setCount(fit())
    // The window resizing: in its own frame, so no tab is squeezed before it goes. What else moves in the bar waits a frame, as two
    // resizes of one element in one frame are a loop to ResizeObserver.
    const bar = held.parentElement ?? held
    const watch = new ResizeObserver((seen) => {
      if (seen.some((one) => one.target === bar)) flushSync(() => setCount(fit()))
      else setCount(fit())
    })
    watch.observe(bar)
    watch.observe(room)
    return () => watch.disconnect()
  }, [named, room])

  return (
    <>
      <nav className="ptabs" aria-label="Projects" ref={strip}>
        {tabs.map((tab) => (
          <Link key={tab.id} className={`ptab ${tab.id === id ? "on" : ""}`} to={`/projects/${tab.id}`} title={tab.path} aria-current={tab.id === id ? "page" : undefined}>
            {tab.name}
          </Link>
        ))}
        {lone !== null && <span className="ptab on">{lone}</span>}
        {hidden.length > 0 && (
          <Link className="ptab more" to={to("projects")} state={{ first: hidden.map((row) => row.id) } satisfies ProjectsAsk} title="All projects">
            {hidden.length} more
          </Link>
        )}
        <Link className="ptab new" to={to("projects")} state={{ new: true } satisfies ProjectsAsk} title="New project" aria-label="New project">
          <Icon name="plus" />
        </Link>
      </nav>
      <div className="ptabs-ghost" ref={sizes} aria-hidden>
        {order.map((row) => (
          <span key={row.id} className={row.id === id ? "on" : undefined}>
            {row.name}
          </span>
        ))}
        <span>{order.length} more</span>
        {lone !== null && <span className="on">{lone}</span>}
      </div>
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
  const shown = useSignal(landscape)
  return (
    <>
      <button type="button" className="tb" onClick={() => setTheme(now === "dark" ? "paper" : "dark")}>
        {now === "dark" ? "Paper" : "Dark"}
      </button>
      {now === "paper" && (
        <button type="button" className="tb" onClick={() => setLandscape(!shown)}>
          {shown ? "Hide landscape" : "Show landscape"}
        </button>
      )}
    </>
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
  const { stream } = useConnection()
  const id = useProjectId()
  // Calcifer holds the project open while a tab shows it.
  useEffect(() => stream.show(id), [stream, id])
  const to = useTo()
  const navigate = useNavigate()
  const location = useLocation()
  const matches = useMatches()
  const [slot, setSlot] = useState<HTMLElement | null>(null)
  const [history, setHistory] = useState<HTMLElement | null>(null)
  const [room, setRoom] = useState<HTMLElement | null>(null)
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
        <span className="history-controls" ref={setHistory} />
        <ProjectTabs id={id} room={room} />
        <Connectivity />
        <span className="gap" ref={setRoom} />
        <MachineTabs to={to} projectId={id} />
        <span className="canvas-controls" ref={setSlot} />
        <MainMenu id={id} />
      </header>
      <CanvasHost key={hostKey} projectId={id} slot={slot} history={history} onAdopt={adopt} />
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
