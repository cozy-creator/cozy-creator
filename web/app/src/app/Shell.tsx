import { useQuery, useQueryClient } from "@tanstack/react-query"
import { type ReactNode, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react"
import { flushSync } from "react-dom"
import { Link, Outlet, useLocation, useMatches, useNavigate } from "react-router-dom"
import { TRIES } from "../api/client"
import { ProjectLocked, vaultQuery } from "../panels/ProjectVault"
import { protectProjectView, privateProject } from "./view"
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
import { projectPath, rememberProject, useProjectId, useTo } from "./project"
export { useProjectId, useTo } from "./project"

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
        trying > 0 && <span className="pill warn">Trying again, attempt {trying}/{TRIES}{owed.count > 0 && ` · ${changes} waiting`}</span>
      )}
      {version && (
        <button type="button" className="pill warn" onClick={() => location.reload()}>
          Calcifer is now {version}: Reload
        </button>
      )}
    </>
  )
}

/** Projects in the order they took their tabs, each staying where it is so a tab doesn't jump from under the pointer. What's new takes the end.
 *  The open project past the `count` that fit takes the last one's place, and keeps it once another is open. */
function useOrder(rows: KnownProject[], id: string | null, count: number): KnownProject[] {
  const order = useRef<string[]>([])
  const known = new Map(rows.map((row) => [row.id, row]))
  const held = [...order.current.filter((one) => known.has(one)), ...rows.map((row) => row.id).filter((one) => !order.current.includes(one))]
  const at = id === null ? -1 : held.indexOf(id)
  const last = Math.max(count, 1) - 1
  if (Number.isFinite(count) && at > last) held.splice(last, 0, ...held.splice(at, 1))
  order.current = held
  return held.flatMap((one) => known.get(one) ?? [])
}

/** The first `count`, with the open project in the last place if it isn't one of them. */
function tabsOf(order: KnownProject[], id: string | null, count: number): KnownProject[] {
  const shown = order.slice(0, Math.max(count, 1))
  const open = order.find((row) => row.id === id)
  if (open && !shown.includes(open)) shown[shown.length - 1] = open
  return shown
}

/** As many tabs as fit their room, the rest behind "N more", and what's said beside them. The room is the bar's free space, whatever its
 *  tabs, so a refit moves nothing it watches: whatever changes (the window, what else is in the bar, the fonts, a word beside them), the
 *  tabs are refit in that frame and none is drawn squeezed. A hidden copy of every tab says what each takes. */
function ProjectTabs({ id, readable, children }: { id: string | null; readable: boolean; children: ReactNode }) {
  const { client, stream } = useConnection()
  const to = useTo()
  const { data: heldProject } = useQuery({ ...projectQuery(client, stream, id ?? ""), enabled: Boolean(id) && readable })
  const data = readable ? heldProject : undefined
  const { data: list } = useQuery(projectsQuery(client))
  const [count, setCount] = useState(Number.POSITIVE_INFINITY)
  const order = useOrder(list?.projects ?? [], id, count)
  const room = useRef<HTMLDivElement>(null)
  const strip = useRef<HTMLElement>(null)
  const beside = useRef<HTMLSpanElement>(null)
  const sizes = useRef<HTMLDivElement>(null)
  const tabs = tabsOf(order, id, count)
  const hidden = order.filter((row) => !tabs.includes(row))
  const saidOfFile = data ? warningsOf(data).ofProject : []
  const name = data?.path.split(/[/\\]/).filter(Boolean).pop()
  // The open project before the list has it, or none.
  const lone = order.some((row) => row.id === id) ? null : (name ?? (id ? "…" : "untitled"))
  const named = `${id} ${lone} ${order.map((row) => `${row.id}:${row.name}`).join(" ")}`
  const fit = useRef(() => count)

  useLayoutEffect(() => {
    fit.current = () => {
      const [area, held, said, ghost] = [room.current, strip.current, beside.current, sizes.current]
      if (!area || !held || !said || !ghost) return count
      const style = getComputedStyle(held)
      const space = Math.min(
        area.getBoundingClientRect().width - said.getBoundingClientRect().width - (Number.parseFloat(getComputedStyle(area).columnGap) || 0),
        Number.parseFloat(style.maxWidth) || Number.POSITIVE_INFINITY,
      )
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
  })
  useLayoutEffect(() => setCount(fit.current()), [named])
  // Made once, before anything else can watch the bar: what watches it later sees the tabs already refit.
  useLayoutEffect(() => {
    const watch = new ResizeObserver(() => flushSync(() => setCount(fit.current())))
    for (const held of [room.current, beside.current, sizes.current]) if (held) watch.observe(held)
    return () => watch.disconnect()
  }, [])

  return (
    <div className="ptabs-room" ref={room}>
      <nav className="ptabs" aria-label="Projects" ref={strip}>
        {tabs.map((tab) => (
          <Link key={tab.id} className={`ptab ${tab.id === id ? "on" : ""}`} to={projectPath(tab)} title={tab.path} aria-current={tab.id === id ? "page" : undefined}>
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
      <span className="ptabs-said" ref={beside}>
        {saidOfFile.length > 0 && (
          <Link className="sum ood" to={to("yaml")} title={noHash(saidOfFile.join("\n"))}>
            !
          </Link>
        )}
        {children}
      </span>
      <div className="ptabs-ghost" aria-hidden>
        <div ref={sizes}>
          {order.map((row) => (
            <span key={row.id} className={row.id === id ? "on" : undefined}>
              {row.name}
            </span>
          ))}
          <span>{order.length} more</span>
          {lone !== null && <span className="on">{lone}</span>}
        </div>
      </div>
    </div>
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
  const { client, stream, bridge, outbox, canvases, retireProject, privateViewEpoch } = useConnection()
  const privateEpoch = useSignal(privateViewEpoch)
  const id = useProjectId()
  const vault = useQuery({ ...vaultQuery(client, id ?? ""), enabled: Boolean(id) })
  const [verified, setVerified] = useState<string | null>(null)
  const [checking, setChecking] = useState(false)
  // A cached unlocked status is not authority after selecting or adopting a project.
  const freshChecked = vault.isSuccess && vault.isFetchedAfterMount
  // Cached ordinary content stays editable offline; a remembered private project needs fresh verification.
  const knownPlainCache = Boolean(id && vault.data?.encrypted === false && vault.data.state === "plain" && !privateProject(id))
  const checked = freshChecked || knownPlainCache
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
    if (checked && document.visibilityState === "visible") setChecking(false)
  }, [id, vault.dataUpdatedAt, vault.isFetchedAfterMount, checked])
  const to = useTo()
  const navigate = useNavigate()
  const location = useLocation()
  const matches = useMatches()
  const [slot, setSlot] = useState<HTMLElement | null>(null)
  const [history, setHistory] = useState<HTMLElement | null>(null)
  // The first import on an empty canvas makes a project: the same canvas stays mounted, so nothing on it is lost.
  const adopted = useRef(new Map<string, string>())
  const hostKey = (id && adopted.current.get(id)) ?? id ?? "empty"
  const queries = useQueryClient()

  useLayoutEffect(() => {
    if (!id || !vault.data) { if (id) setVerified(null); return }
    if (vault.data.encrypted) protectProjectView(id, true)
    else if (freshChecked && vault.data.encrypted === false && vault.data.state === "plain") protectProjectView(id, false)
    if (checked && allowed && !checking && !uncertain) {
      bridge.allowProject(id)
      outbox.allowProject(id)
      setVerified(id)
      return
    }
    if (!revoked) return
    setVerified(null)
    retireProject(id)
    for (const media of document.querySelectorAll<HTMLMediaElement>("video,audio")) {
      media.pause(); media.removeAttribute("src"); media.load()
    }
    for (const image of document.querySelectorAll<HTMLImageElement>("img")) {
      if (image.src.startsWith("blob:")) URL.revokeObjectURL(image.src)
      image.removeAttribute("src")
    }

  }, [id, vault.data, freshChecked, checked, allowed, checking, uncertain, revoked, bridge, outbox, canvases, queries, retireProject])

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
    (project: string, code: string) => {
      // Keep the pending canvas in memory while its own fresh protection read
      // completes. Creation may be encrypted; never replace its validated status
      // with invented ordinary authority or allow persistence before that read.
      protectProjectView(project, true)
      adopted.current.set(project, "empty")
      rememberProject(queries, { id: project, code })
      setVerified(project)
      void queries.invalidateQueries({ queryKey: ["projects"] })
      navigate(projectPath({ id: project, code }, location.pathname.replace(/^\/$/, "")), { replace: true })
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
        <ProjectTabs id={id} readable={readable}>
          <Connectivity />
        </ProjectTabs>
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
