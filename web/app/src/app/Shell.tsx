import { useQuery, useQueryClient } from "@tanstack/react-query"
import { type ReactNode, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react"
import { flushSync } from "react-dom"
import { Link, Outlet, useLocation, useMatches, useNavigate } from "react-router-dom"
import { TRIES } from "../api/client"
import { DrawerFrame } from "../panels/Drawer"
import { ProjectLocked, vaultQuery } from "../panels/ProjectVault"
import { protectProjectView, privateProject } from "./view"
import { say } from "../ui/notes"
import { appearanceQuery, projectQuery, projectsQuery, projectViewQuery } from "../api/queries"
import { CanvasHost } from "../canvas/CanvasHost"
import { warningsOf } from "../canvas/graph"
import { noHash } from "../canvas/text"
import { MachineTabs } from "../panels/machines/MachineTabs"
import { RunsLink } from "../panels/runs/RunsLink"
import type { KnownProject, Opened } from "../api/calcifer"
import { ProjectCreate } from "../panels/ProjectCreate"
import { asked, giveUp, newFolder } from "./making"
import { Icon } from "../ui/Icon"
import logo from "../ui/logo.png"
import { Notes } from "../ui/NoteList"
import { pressBeganOutside } from "../ui/press"
import { useSignal } from "../ui/signal"
import { shown as lookShown, showAppearance } from "../ui/theme"
import { useConnection } from "./Connection"
import { reloadWhenSafe } from "./reload"
import { projectPath, rememberProject, useProjectId, useTo } from "./project"
import { opened, useByUse } from "../panels/Projects"
export { useProjectId, useTo } from "./project"

/** A few tries in, it isn't a blip. Nothing restarts Calcifer on Windows, and starting Cozy again does anywhere. */
const SAYS_HOW = 5
/** How long before an unlock's deadline its status is asked again. */
const EXPIRY_ASK_MS = 2000

/** Whether Calcifer answers, and what's waiting for it: said, and nothing waits on it. Gone once all of it is saved. A new build of Cozy
 *  says nothing: the page reloads itself when nothing would be lost (`reloadWhenSafe`). */
function Connectivity() {
  const { stream, attempt, bridge, outbox } = useConnection()
  const health = useSignal(stream.health)
  const trying = useSignal(attempt)
  const owed = useSignal(outbox.owed)
  const version = useSignal(bridge.newVersion)
  useEffect(() => {
    if (version) reloadWhenSafe(outbox)
  }, [version, outbox])
  const changes = `${owed.count} ${owed.count === 1 ? "change" : "changes"}`
  return (
    <>
      {health.state === "reconnecting" ? (
        <span className="pill warn" role="status">
          Cozy is offline{owed.count > 0 && `, ${changes} waiting`}
          {health.attempt >= SAYS_HOW && ". Restart it"}
        </span>
      ) : owed.count > 0 && owed.slow ? (
        <span className="pill warn" role="status">
          {owed.held ? `Can't save: ${noHash(owed.held)}. ${changes} waiting` : `Saving ${changes}…`}
        </span>
      ) : (
        trying > 0 && <span className="pill warn">Trying again, attempt {trying}/{TRIES}{owed.count > 0 && `, ${changes} waiting`}</span>
      )}
    </>
  )
}

/** The first `count`, with the open project in the last place if it isn't one of them. */
function tabsOf(order: KnownProject[], id: string | null, count: number): KnownProject[] {
  const shown = order.slice(0, Math.max(count, 1))
  const open = order.find((row) => row.id === id)
  if (open && !shown.includes(open)) shown[shown.length - 1] = open
  return shown
}

/** As many tabs as fit their room, the rest behind "N more", and what's said beside them. The room is the bar's free space, whatever its
 *  tabs, so a refit moves nothing it watches. What it holds changing refits it in that frame; its room changing, by the next. A hidden copy
 *  of every tab says what each takes. */
function ProjectTabs({ id, folder, readable, children }: { id: string | null; folder?: string; readable: boolean; children: ReactNode }) {
  const { client, stream } = useConnection()
  const to = useTo()
  const navigate = useNavigate()
  const { data: heldProject } = useQuery({ ...projectQuery(client, stream, id ?? ""), enabled: Boolean(id) && readable })
  const data = readable ? heldProject : undefined
  const { data: list } = useQuery(projectsQuery(client))
  const queries = useQueryClient()
  const making = useSignal(asked)
  const naming = useSignal(newFolder)
  // A tab opens its project at once: its address is known from the list, so the page never waits on it, blank (cl-980).
  useEffect(() => { for (const row of list?.projects ?? []) rememberProject(queries, row) }, [list, queries])
  const [count, setCount] = useState(Number.POSITIVE_INFINITY)
  const order = useByUse(list?.projects ?? [])
  const room = useRef<HTMLDivElement>(null)
  const strip = useRef<HTMLElement>(null)
  const beside = useRef<HTMLSpanElement>(null)
  const sizes = useRef<HTMLDivElement>(null)
  const tabs = tabsOf(order, id, count)
  const hidden = order.filter((row) => !tabs.includes(row))
  const saidOfFile = data ? warningsOf(data).ofProject : []
  const name = (data?.path ?? folder)?.split(/[/\\]/).filter(Boolean).pop()
  // The open project before the list has it; at `/`, the one being made, its folder's name as it's typed, at once; else none.
  const lone = order.some((row) => row.id === id) ? null : id ? (name ?? "…") : making ? (naming || "untitled") : null
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
      const add = widths.pop() ?? 0
      const more = widths.pop() ?? 0
      const width = new Map(order.map((row, at) => [row, widths[at]]))
      const fits = (n: number) => {
        const shown = tabsOf(order, id, n)
        const items = [...shown.map((row) => width.get(row) ?? 0), ...alone, ...(shown.length < order.length ? [more] : []), add]
        return Number.parseFloat(style.paddingLeft) + Number.parseFloat(style.paddingRight) + items.reduce((sum, each) => sum + each + gap, -gap) <= space
      }
      if (fits(order.length)) return order.length
      let n = 1
      while (n < order.length - 1 && fits(n + 1)) n++
      return n
    }
  })
  useLayoutEffect(() => setCount(fit.current()), [named])
  // A size changed from outside (the window, the bar, the fonts, a word beside them) refits at the next frame, never inside the observer: a
  // render there takes in whatever else is waiting (a project just listed grows the hidden copy it watches), and WebKit says the loop (cl-973).
  useLayoutEffect(() => {
    let frame = 0
    const watch = new ResizeObserver(() => {
      cancelAnimationFrame(frame)
      frame = requestAnimationFrame(() => flushSync(() => setCount(fit.current())))
    })
    for (const held of [room.current, beside.current, sizes.current]) if (held) watch.observe(held)
    return () => {
      cancelAnimationFrame(frame)
      watch.disconnect()
    }
  }, [])

  return (
    <div className="ptabs-room" ref={room}>
      <nav className="ptabs" aria-label="Projects" ref={strip}>
        {tabs.map((tab) => (
          // The open project's name opens Projects (web-ui.md §1); + makes a new one (Paul, 11-16).
          <Link
            key={tab.id}
            className={`ptab ${tab.id === id ? "on" : ""}`}
            to={projectPath(tab)}
            title={tab.id === id ? `${tab.path}\nClick to see all projects` : tab.path}
            aria-current={tab.id === id ? "page" : undefined}
            onClick={(event) => {
              if (tab.id !== id) return
              event.preventDefault()
              navigate(to("projects"))
            }}
          >
            {tab.name}
          </Link>
        ))}
        {lone !== null && <span className="ptab on">{lone}</span>}
        {hidden.length > 0 && (
          <Link className="ptab more" to={to("projects")} title="All projects">
            {hidden.length} more
          </Link>
        )}
        {/* The projects' own +, the strip's end (Paul, 11-16: "connected to the project list"). */}
        <button type="button" className="ptab add" aria-label="New project" title="New project"
          onClick={() => {
            if (asked.get()?.by !== "drop") asked.set({ by: "plus" })
            if (id || location.pathname !== "/") navigate("/")
          }}>
          +
        </button>
      </nav>
      <span className="ptabs-said" ref={beside}>
        {saidOfFile.length > 0 && (
          <span className="sum ood" title={noHash(saidOfFile.join("\n"))}>
            !
          </span>
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
          <span>+</span>
          {lone !== null && <span className="on">{lone}</span>}
        </div>
      </div>
    </div>
  )
}

/** How the page looks (cl-1097): the open project's own, else everywhere's. What isn't read yet keeps what's shown; everywhere's, never
 *  set on this computer, is what this browser shows. */
function useAppearance(id: string | null, readable: boolean) {
  const { client, stream } = useConnection()
  const everywhere = useQuery(appearanceQuery(client))
  const { data: state } = useQuery({ ...projectQuery(client, stream, id ?? ""), enabled: Boolean(id) && readable })
  const seeded = useRef(false)
  useEffect(() => {
    if (everywhere.data !== null || seeded.current) return
    seeded.current = true
    void client.setAppearance(lookShown.get()).catch(() => { seeded.current = false })
  }, [everywhere.data, client])
  const own = id && readable ? (state ? (state.appearance ?? null) : undefined) : null
  const wanted = own === undefined ? undefined : (own ?? everywhere.data ?? undefined)
  const key = wanted && JSON.stringify(wanted)
  useLayoutEffect(() => {
    if (wanted && key !== JSON.stringify(lookShown.get())) showAppearance(wanted)
  }, [key])
}

/** A new project, made in place over the empty canvas (cl-980): the strip's + asks for one, as a first drop does. A press that began outside it,
 *  or Esc, puts it away; a drop waiting on it hears so. Made, it opens, or the drop goes on into it. */
function NewProject() {
  const navigate = useNavigate()
  const queries = useQueryClient()
  const held = useRef<HTMLElement>(null)
  const holding = useRef(false)
  const hold = useCallback((now: boolean) => { holding.current = now }, [])
  useEffect(() => {
    const inside = (element: Element) => Boolean(held.current?.contains(element) || element.closest("header.top"))
    const click = (event: MouseEvent) => {
      if (!holding.current && event.button === 0 && event.target instanceof Element && !inside(event.target) && pressBeganOutside(inside)) giveUp()
    }
    const key = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented || holding.current) return
      event.preventDefault()
      giveUp()
    }
    window.addEventListener("click", click)
    window.addEventListener("keydown", key, true)
    return () => {
      window.removeEventListener("click", click)
      window.removeEventListener("keydown", key, true)
    }
  }, [])
  const created = (project: Opened) => {
    const ask = asked.get()
    asked.set(null)
    rememberProject(queries, project)
    for (const line of project.said ?? []) say(line)
    if (ask?.by === "drop") ask.resolve(project)
    else navigate(projectPath(project))
  }
  return (
    <div className="home">
      <section ref={held} className="new-project" aria-label="New project">
        <h2>New project</h2>
        <ProjectCreate autoFocus onCreated={created} onCancel={giveUp} onHolding={hold} />
      </section>
    </div>
  )
}

/** A drawer of the open project's own data, by where it is (its runs, a run, a new run, its assets): its title. */
function ownTitle(path: string): string | null {
  const [, drawer, number] = /\/(runs|new|assets)(?:\/([^/]+))?\/?$/.exec(path) ?? []
  if (drawer === "runs") return number ? `Run ${number}` : "Runs"
  return drawer === "new" ? "New run" : drawer === "assets" ? "Assets" : null
}

/** All but the canvas, a click away. */
function MainMenu({ id, readable }: { id: string | null; readable: boolean }) {
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
        <RunsLink to={to("runs")} counted={readable} />
        <Link className="tb" to={to("machines")}>
          Machines
        </Link>
        <Link className="tb" to={to("packages")}>
          Installations
        </Link>
        {id && (
          <Link className="tb" to={to("assets")}>
            Assets
          </Link>
        )}
        {id && <Link className="tb" to={to("project-settings")}>Project settings</Link>}
        {/* Last, Account then Settings (Paul, 11-16). */}
        {!client.transport.channel && (
          <Link className="tb" to={to("account")}>
            Account
          </Link>
        )}
        <Link className="tb" to={to("settings")}>
          Settings
        </Link>
      </nav>
    </details>
  )
}

export function Shell() {
  const { client, stream, bridge, outbox, canvases, retireProject, privateViewEpoch } = useConnection()
  const privateEpoch = useSignal(privateViewEpoch)
  const id = useProjectId()
  const making = useSignal(asked)
  // A project opened: what the + asked for is put away.
  useEffect(() => { if (id && asked.get()?.by === "plus") asked.set(null) }, [id])
  const vault = useQuery({ ...vaultQuery(client, id ?? ""), enabled: Boolean(id) })
  const live = useSignal(stream.health).state === "live"
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
  // Opened here: used now, as Calcifer records it when it's shown.
  useEffect(() => { if (id && readable) opened(id) }, [id, readable])
  useEffect(() => {
    if (checked && document.visibilityState === "visible" && (live || !vault.data?.encrypted)) setChecking(false)
  }, [id, vault.dataUpdatedAt, vault.isFetchedAfterMount, checked, live, vault.data?.encrypted])
  // An encrypted project's page is covered while its stream is down: a Calcifer that crashed said no `locking`, and the page shows nothing
  // of it until the stream is back and the vault still says unlocked (cl-1067).
  useEffect(() => {
    if (!id || !vault.data?.encrypted) return
    if (!live) { setChecking(true); return }
    void vault.refetch().then((answer) => { if (!answer.isError) setChecking(false) })
  }, [id, vault.data?.encrypted, live, vault.refetch])
  const to = useTo()
  const navigate = useNavigate()
  const location = useLocation()
  const matches = useMatches()
  const [slot, setSlot] = useState<HTMLElement | null>(null)
  const [history, setHistory] = useState<HTMLElement | null>(null)
  // The first import on an empty canvas makes a project: the same canvas stays mounted, so nothing on it is lost.
  const adopted = useRef(new Map<string, string>())
  const hostKey = (id && adopted.current.get(id)) ?? id ?? "empty"
  // Switching projects, the canvas last shown stays, still and inert, until the next is checked: the frame stays, and the screen never goes
  // blank between them (cl-980, Paul 11-13). Not from or to an encrypted project, and never the next one's own canvas (a first drop's,
  // adopted): those are covered until they're checked, as before.
  // The next one's canvas view is read first, as its canvas would read it before drawing anything: until it's here, the last canvas stays.
  const nextView = useQuery({ ...projectViewQuery(client, stream, id ?? ""), enabled: Boolean(id) && readable })
  // And its state, which says how it looks (cl-1097): the last canvas stays until the next one shows in its own colors.
  const nextState = useQuery({ ...projectQuery(client, stream, id ?? ""), enabled: Boolean(id) && readable })
  const viewing = readable && Boolean(id) && (!nextView.data || !nextState.data) && !nextView.error && !nextState.error
  useAppearance(id, readable)
  const shown = useRef<{ id: string | null; key: string } | null>(null)
  if (readable && heldCanvas && !viewing) shown.current = { id, key: hostKey }
  const last = shown.current
  const plainNext = Boolean(id) && !privateProject(id) && !vault.data?.encrypted
  const waiting = (!readable && (checking || !checked) && !vault.isError) || viewing
  const between = plainNext && waiting && last && last.id !== id && last.key !== hostKey && !privateProject(last.id) ? last : null
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
    // The project's pictures, never the page's own (its logo).
    for (const image of document.querySelectorAll<HTMLImageElement>("img:not(.brand img)")) {
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
    const left = Math.max(0, vault.data.expires_at_ms - Date.now())
    // Activity anywhere (another tab, the CLI, a run) moves the deadline without a word to this tab: it's asked shortly before, and the
    // cover falls only on one that holds.
    const ask = left > 0 ? setTimeout(() => void vault.refetch(), Math.max(left / 2, left - EXPIRY_ASK_MS)) : undefined
    const timer = setTimeout(() => {
      flushSync(() => setChecking(true))
      void vault.refetch().then((answer) => { if (!answer.isError) setChecking(false) })
    }, left)
    return () => { clearTimeout(ask); clearTimeout(timer) }
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
      navigate(projectPath({ code }, location.pathname.replace(/^\/$/, "")), { replace: true })
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
  // Every drawer opens and closes whether or not the open project is locked (Paul, 11-16: "should not depend upon the main screen"). One of
  // the project's own data, while it isn't read, holds only its title, and says so once it's known to be locked; one it held stays covered
  // while it's checked again.
  const own = id && handle?.drawer ? ownTitle(location.pathname) : null
  const sealed = Boolean(own && !readable)
  const covered = sealed && heldCanvas
  // A click off the open drawer closes it, as Esc does (Paul, 11-12), but only a press and a release both off it: never a selection or a drag
  // from inside it (cl-923). What's clicked in the top bar, a menu or a dialog is that control's own.
  const drawer = useRef<HTMLElement>(null)
  const open = Boolean(handle?.drawer)
  useEffect(() => {
    if (!open) return
    const inside = (element: Element) => Boolean(drawer.current?.contains(element))
    const own = (element: Element) => inside(element) || Boolean(element.closest("header.top, [role=menu], [role=dialog], dialog, .notes, .new-project"))
    const click = (event: MouseEvent) => {
      if (event.button === 0 && event.target instanceof Element && !own(event.target) && pressBeganOutside(own)) navigate(to())
    }
    const key = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return
      // Esc is first for what's over the canvas: a box in Focus, a menu.
      if (!(event.target instanceof Node && drawer.current?.contains(event.target)) && document.querySelector(".focus-viewport, [role=menu], .activity")) return
      navigate(to())
    }
    window.addEventListener("click", click)
    window.addEventListener("keydown", key)
    return () => {
      window.removeEventListener("click", click)
      window.removeEventListener("keydown", key)
    }
  }, [open, navigate, to])
  return (
    <div className="shell" data-drawer={handle?.drawer ?? "none"}>
      <header className="top">
        <Link className="brand" to="/" title="Cozy Creator" aria-label="Cozy Creator home"><img src={logo} alt="" /></Link>
        <span className="history-controls" ref={setHistory} />
        <ProjectTabs id={id} folder={vault.data?.path} readable={readable}>
          <Connectivity />
        </ProjectTabs>
        <MachineTabs to={to} projectId={id} readable={readable} locked={revoked} />
        <span className="canvas-controls" ref={setSlot} />
        <MainMenu id={id} readable={readable} />
      </header>
      {between ? <div inert style={{ display: "contents" }}><CanvasHost key={between.key} projectId={between.id} active={false} slot={null} history={null} onAdopt={adopt} /></div>
        // A first drop waiting on its project holds the canvas still: its box keeps its name field for when the project is made.
        : heldCanvas && <div hidden={!readable} inert={!readable || making?.by === "drop"} style={{ display: readable ? "contents" : "none" }}><CanvasHost key={hostKey} projectId={id} active={readable} slot={readable ? slot : null} history={readable ? history : null} onAdopt={adopt} /></div>}
      {making && !id && <NewProject />}
      {!readable && id && !between && (checking || !checked || uncertain ? <main className="gate">{vault.isError && <><p>Couldn't check whether this project is locked.</p><button className="tb" type="button" onClick={() => void vault.refetch()}>Try again</button></>}</main> : location.pathname.endsWith("/project-settings") ? <main className="gate"><p>This project is locked.</p></main> : <ProjectLocked id={id} />)}
      {handle?.drawer && (
        <aside ref={drawer} className="drawer" hidden={covered} inert={covered} style={covered ? { display: "none" } : undefined}>
          {sealed && !heldCanvas
            ? <DrawerFrame title={own ?? ""}>{revoked && !checking && <p>This project is locked. Unlock it to see this.</p>}</DrawerFrame>
            : <Outlet key={drawerEpoch} />}
        </aside>
      )}
      {!handle?.drawer && readable && <Outlet />}
      {readable && <Notes />}
    </div>
  )
}
