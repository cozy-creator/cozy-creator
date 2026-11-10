import { useQuery, useQueryClient } from "@tanstack/react-query"
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react"
import { flushSync } from "react-dom"
import { Link, Outlet, useLocation, useMatch, useMatches, useNavigate } from "react-router-dom"
import { TRIES } from "../api/client"
import { ProjectLocked, vaultKey, vaultQuery } from "../panels/ProjectVault"
import { protectProjectView, forgetProjectView } from "./view"
import { notes } from "../ui/notes"
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
function ProjectTabs({ id, room, readable }: { id: string | null; room: HTMLElement | null; readable: boolean }) {
  const { client, stream } = useConnection()
  const to = useTo()
  const { data: heldProject } = useQuery({ ...projectQuery(client, stream, id ?? ""), enabled: Boolean(id) && readable })
  const data = readable ? heldProject : undefined
  const { data: list } = useQuery(projectsQuery(client))
  const order = useOrder(list?.projects ?? [])
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
    <details
      className="main-menu"
      ref={held}
      onKeyDown={(event) => {
        // These keys activate the menu's native controls, not the canvas's pan or take shortcuts.
        if (event.key === "Enter" || event.key === " ") event.stopPropagation()
      }}
    >
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
        {id && <Link className="tb" to={to("vault")}>Project encryption</Link>}
        <ThemeSwitch />
      </nav>
    </details>
  )
}

export function Shell() {
  const { client, stream, bridge, outbox, canvases, privateViewEpoch } = useConnection()
  const privateEpoch = useSignal(privateViewEpoch)
  const id = useProjectId()
  const vault = useQuery({ ...vaultQuery(client, id ?? ""), enabled: Boolean(id) })
  const [verified, setVerified] = useState<string | null>(null)
  const [checking, setChecking] = useState(false)
  // A cached unlocked status is not authority after selecting this project again.
  // The initial empty-canvas import alone already has a fresh ordinary-project reply.
  const freshOrdinary = useRef<string | null>(null)
  const checked = vault.isSuccess && (vault.isFetchedAfterMount || freshOrdinary.current === id)
  const allowed = Boolean(vault.data && (vault.data.state === "plain" || vault.data.state === "unlocked"))
  const uncertain = Boolean(vault.data?.encrypted && vault.isError)
  const revoked = Boolean(checked && vault.data?.encrypted && (vault.data.state === "locked" || vault.data.state === "locking"))
  const readable = !id || (verified === id && checked && allowed && !checking && !uncertain)
  // Temporary verification covers the existing canvas. Its private drafts, queued edits and
  // imports stay in memory; only a confirmed lock releases their ownership.
  const heldCanvas = !id || (verified === id && !revoked)
  useLayoutEffect(() => {
    if (readable) return
    const pause = (event: Event) => { if (event.target instanceof HTMLMediaElement) event.target.pause() }
    for (const media of document.querySelectorAll<HTMLMediaElement>("video,audio")) media.pause()
    // Also stop a pending source load from starting playback after the cover appeared.
    document.addEventListener("play", pause, true)
    return () => document.removeEventListener("play", pause, true)
  }, [readable])
  useEffect(() => {
    stream.show(readable ? id : null)
    return () => stream.show(null)
  }, [stream, id, readable])
  useEffect(() => {
    if (freshOrdinary.current !== id || vault.isFetchedAfterMount) freshOrdinary.current = null
    if (checked && document.visibilityState === "visible") setChecking(false)
  }, [id, vault.dataUpdatedAt, vault.isFetchedAfterMount, checked])
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

  useLayoutEffect(() => {
    if (!id || !vault.data) { if (id) setVerified(null); return }
    protectProjectView(id, vault.data.encrypted)
    if (checked && allowed && !checking && !uncertain) {
      bridge.allowProject(id)
      outbox.allowProject(id)
      setVerified(id)
      return
    }
    if (!revoked) return
    setVerified(null)
    bridge.forgetProject(id)
    outbox.discardProject(id)
    canvases.get(id)?.revokePrivateView()
    canvases.delete(id)
    forgetProjectView(id)
    for (const media of document.querySelectorAll<HTMLMediaElement>("video,audio")) {
      media.pause(); media.removeAttribute("src"); media.load()
    }
    for (const image of document.querySelectorAll<HTMLImageElement>("img")) {
      if (image.src.startsWith("blob:")) URL.revokeObjectURL(image.src)
      image.removeAttribute("src")
    }
    const privateData = ({ queryKey }: { queryKey: readonly unknown[] }) =>
      (["project", "canvas", "yaml"].includes(String(queryKey[0])) && queryKey[1] === id) || ["runs", "run", "tools", "history"].includes(String(queryKey[0]))
    void queries.cancelQueries({ predicate: privateData })
    queries.removeQueries({ predicate: privateData })
    notes.set([])
  }, [id, vault.data, checked, allowed, checking, uncertain, revoked, bridge, outbox, canvases, queries])

  useEffect(() => {
    if (!id || !vault.data?.encrypted) return
    const cover = () => flushSync(() => setChecking(true))
    const recheck = () => {
      cover()
      void vault.refetch().then((answer) => { if (!answer.isError) setChecking(false) })
    }
    const visible = () => { if (document.visibilityState === "hidden") cover(); else recheck() }
    document.addEventListener("visibilitychange", visible)
    window.addEventListener("pagehide", cover)
    window.addEventListener("pageshow", recheck)
    return () => {
      document.removeEventListener("visibilitychange", visible)
      window.removeEventListener("pagehide", cover)
      window.removeEventListener("pageshow", recheck)
    }
  }, [id, vault.data?.encrypted, vault.refetch])

  useEffect(() => {
    if (!id || !vault.data?.encrypted || vault.data.state !== "unlocked" || vault.data.expires_at_ms == null) return
    const timer = setTimeout(() => {
      flushSync(() => setChecking(true))
      void vault.refetch().then((answer) => { if (!answer.isError) setChecking(false) })
    }, Math.max(0, vault.data.expires_at_ms - Date.now()))
    return () => clearTimeout(timer)
  }, [id, vault.data?.encrypted, vault.data?.state, vault.data?.expires_at_ms, vault.refetch])

  useEffect(() => {
    if (!id || !readable || !vault.data?.encrypted) return
    let last = Number.NEGATIVE_INFINITY
    const interval = Math.min(10_000, Math.max(100, (vault.data?.idle_timeout_s || 30) * 1000 / 3))
    const activity = (event: Event) => {
      if (!event.isTrusted || document.visibilityState !== "visible" || performance.now() - last < interval) return
      if ((event.type === "play" || event.type === "timeupdate") && (!(event.target instanceof HTMLMediaElement) || event.target.paused || !event.target.controls)) return
      if (event.type === "pointermove" && (!(event instanceof PointerEvent) || event.buttons === 0)) return
      last = performance.now()
      void client.vaultChange(id, { action: "activity" }).catch(() => { void vault.refetch() })
    }
    for (const event of ["pointerdown", "pointermove", "keydown", "play", "timeupdate"]) document.addEventListener(event, activity, true)
    return () => { for (const event of ["pointerdown", "pointermove", "keydown", "play", "timeupdate"]) document.removeEventListener(event, activity, true) }
  }, [id, readable, vault.data?.encrypted, vault.data?.idle_timeout_s, client, vault.refetch])

  const adopt = useCallback(
    (project: string) => {
      adopted.current.set(project, "empty")
      // This callback only adopts a newly created ordinary project. Preserve the initial
      // import's live canvas/outbox while its first status read catches up.
      freshOrdinary.current = project
      queries.setQueryData(vaultKey(project), { encrypted: false, state: "plain", path: "", idle_timeout_s: 0, next: [] })
      setVerified(project)
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

  const mixedDrawer = /(?:^|\/)(?:runs|new|packages|models)(?:\/|$)/.test(location.pathname)
  const drawerEpoch = mixedDrawer ? privateEpoch : 0
  const handle = matches[matches.length - 1]?.handle as { drawer?: "normal" | "wide" } | undefined
  const drawerReadable = readable || (location.pathname.endsWith("/vault") && checked && !checking && !uncertain)
  return (
    <div className="shell" data-drawer={handle?.drawer ?? "none"}>
      <header className="top">
        <span className="history-controls" ref={setHistory} />
        <ProjectTabs id={id} room={room} readable={readable} />
        <Connectivity />
        <span className="gap" ref={setRoom} />
        <MachineTabs to={to} projectId={id} />
        <span className="canvas-controls" ref={setSlot} />
        <MainMenu id={id} />
      </header>
      {heldCanvas && <div hidden={!readable} inert={!readable} style={{ display: readable ? "contents" : "none" }}><CanvasHost key={hostKey} projectId={id} active={readable} slot={readable ? slot : null} history={readable ? history : null} onAdopt={adopt} /></div>}
      {!readable && id && (checking || !checked || uncertain ? <main className="gate"><p>{vault.isError ? "Project protection could not be checked." : "Checking project protection…"}</p>{vault.isError && <button className="tb" type="button" onClick={() => void vault.refetch()}>Try again</button>}</main> : location.pathname.endsWith("/vault") ? <main className="gate"><p>Project protection</p></main> : <ProjectLocked id={id} />)}
      {handle?.drawer && (heldCanvas || drawerReadable) && (
        <aside className="drawer" hidden={!drawerReadable} inert={!drawerReadable} style={drawerReadable ? undefined : { display: "none" }}>
          <Outlet key={drawerEpoch} />
        </aside>
      )}
      {!handle?.drawer && readable && <Outlet />}
      {readable && <Notes />}
    </div>
  )
}
