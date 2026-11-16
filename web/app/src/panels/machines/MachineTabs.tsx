import { useQuery } from "@tanstack/react-query"
import { useEffect, useLayoutEffect, useRef, useState } from "react"
import { flushSync } from "react-dom"
import { Link } from "react-router-dom"
import { machinesQuery, maintenanceRunsQuery, projectQuery, runsQuery } from "../../api/queries"
import { useConnection } from "../../app/Connection"
import type { Machine, Operation } from "../../api/calcifer"
import { Icon } from "../../ui/Icon"
import { Bar } from "../common"
import { ordinal } from "../format"
import { runName as runLabel, workWords } from "../runs/words"
import { Checks, Spec } from "./RenderHere"
import { type Doing, doingOf, fetchedWords, fractionOf, operationName, runName, waits, working } from "./activity"
import type { MachinesAsk } from "./Machines"
import { called, gpusWords, slowWords, stableName, stateWords } from "./words"

type Mark = "broken" | "slow" | "up" | "download" | "install" | "work" | "idle"

/** Red blinks broken and yellow coming up. Up, each thing it does: an arrow glowing while weights arrive, a package while one installs, green
 *  blinking while it runs inference; steady grey with nothing to do. Steady red first while something makes its runs slow (cl-913). */
function marksOf(machine: Machine, doing: Doing[]): Mark[] {
  if (machine.offline || machine.broken || machine.end_failed || machine.unreachable_since_ms || machine.rental?.unreachable_since_ms) return ["broken"]
  if (machine.state === "booting" || machine.state === "ending") return ["up"]
  if (machine.state !== "ready" && machine.state !== "stopped") return ["broken"]
  const kinds = new Set(doing.map((one) => (one.kind === "run" ? (working(one.run, one.on) ? "work" : "waits") : one.kind)))
  const marks: Mark[] = (["download", "install", "work"] as const).filter((kind) => kinds.has(kind))
  return [...(slowWords(machine).length ? (["slow"] as const) : []), ...(marks.length ? marks : (["idle"] as const))]
}

function MarkOf({ mark }: { mark: Mark }) {
  if (mark === "download" || mark === "install")
    return (
      <span className={`mark ${mark}`}>
        <Icon name={mark === "download" ? "download" : "package"} />
      </span>
    )
  return <span className={`dot ${mark}`} />
}

/** A tab's marks, name and how many function runs, its GPUs' work: an install or a download is listed, not counted (cl-889). */
function Face({ machine, doing }: { machine: Machine; doing: Doing[] }) {
  const total = doing.filter((one) => one.kind === "run" && one.run.kind === "call").length
  return (
    <>
      {marksOf(machine, doing).map((mark) => (
        <MarkOf key={mark} mark={mark} />
      ))}
      <b>{called(machine)}</b>
      {total > 0 && <span className="load">{total}</span>}
    </>
  )
}

const sentence = (text: string) => text.charAt(0).toUpperCase() + text.slice(1)

// The runs waiting on it by name, `KEY-N` (cl-987).
const numbers = (operation: Operation) => {
  const runs = operation.names?.length ? operation.names : operation.runs.map(String)
  return runs.length === 1 ? `run ${runs[0]}` : `runs ${runs.join(", ")}`
}

function Row({ doing, boxes, to }: { doing: Doing; boxes: Map<number, string>; to: (rest?: string) => string }) {
  if (doing.kind === "boot")
    return (
      <li className="doing">
        <MarkOf mark="up" />
        <b>Starting up</b>
        <span className="said">{doing.words}</span>
      </li>
    )
  if (doing.kind !== "run") {
    const { operation } = doing
    const fraction = fractionOf(operation)
    return (
      <li className="doing" data-doing={doing.kind}>
        <MarkOf mark={doing.kind} />
        <b title={operation.subject}>{operationName(operation)}</b>
        <span className="kind">{doing.kind === "download" ? "Download" : "Install"}</span>
        {doing.kind === "download" && fraction !== undefined && <Bar fraction={fraction} />}
        <span className="said">
          {doing.kind === "download" ? fetchedWords(operation) : sentence(operation.stage || "installing")} · for {numbers(operation)}
        </span>
      </li>
    )
  }
  const { run, on } = doing
  const fraction = run.progress?.done_fraction
  const going = working(run, on)
  const said = on
    ? `Waiting for ${operationName(on)} to ${on.kind === "download" ? "download" : "install"}`
    : waits(run)
      ? (run.waiting ? sentence(run.waiting) : run.queue_position ? `Queued, ${ordinal(run.queue_position)} in line` : "Queued")
      : workWords(run) || (run.waiting && sentence(run.waiting))
  return (
    <li className="doing" data-doing="run" data-run={run.number}>
      <MarkOf mark={going ? "work" : "idle"} />
      <Link to={run.project_id ? to(`runs/${run.number}`) : `/runs/${run.number}`} title={`Run ${runLabel(run)}: ${run.target}`}>
        <b>{runName(run, boxes)}</b>
      </Link>
      <span className="kind">Run {runLabel(run)}</span>
      {going && fraction != null && <Bar fraction={fraction} />}
      {said && <span className="said">{said}</span>}
    </li>
  )
}

/** What a machine is doing, under its chip: open until a click elsewhere or Escape. */
function Activity({ machine, doing, boxes, to, chip, close }: { machine: Machine; doing: Doing[]; boxes: Map<number, string>; to: (rest?: string) => string; chip: HTMLElement; close: () => void }) {
  const held = useRef<HTMLDivElement>(null)
  const [at, setAt] = useState<{ top: number; right: number } | null>(null)
  useLayoutEffect(() => {
    const place = () => {
      const box = chip.getBoundingClientRect()
      setAt({ top: box.bottom + 6, right: Math.max(8, innerWidth - box.right) })
    }
    place()
    window.addEventListener("resize", place)
    return () => window.removeEventListener("resize", place)
  }, [chip])
  useEffect(() => {
    const away = (event: PointerEvent) => {
      const target = event.target as globalThis.Node
      if (!held.current?.contains(target) && !chip.contains(target)) close()
    }
    const key = (event: KeyboardEvent) => {
      if (event.key === "Escape") close()
    }
    window.addEventListener("pointerdown", away)
    window.addEventListener("keydown", key)
    return () => {
      window.removeEventListener("pointerdown", away)
      window.removeEventListener("keydown", key)
    }
  }, [chip, close])
  return (
    <div className="activity" ref={held} role="dialog" aria-label={`What ${called(machine)} is doing`} style={at ? { top: at.top, right: at.right } : { visibility: "hidden" }}>
      <header>
        <Spec machine={machine} now={Date.now()} />
        <Checks machine={machine}>
          <Link className="more" to={to(`machines/${encodeURIComponent(stableName(machine))}`)} onClick={close}>
            Details
          </Link>
        </Checks>
      </header>
      {slowWords(machine).map((line) => (
        <p className="warn slow" key={line}>
          {line}
        </p>
      ))}
      {doing.length ? (
        <ul>
          {doing.map((one) => (
            <Row key={one.key} doing={one} boxes={boxes} to={to} />
          ))}
        </ul>
      ) : (
        <p className="said">Idle</p>
      )}
    </div>
  )
}

/** Each machine, top right, `local` first: what it's doing at a glance, and a click shows each thing with how far it is, and Render here,
 *  which makes it one of the open project's preferred machines, green (Paul 11-12). Kept by the stream. As many as fit the strip's width,
 *  the rest behind "N more", which lists them first; then + for a new machine wherever renting is (Paul 11-16). As ProjectTabs fits its
 *  own: a hidden copy of every tab says what each takes, what it holds refits it in that frame, a size changing by the next. */
export function MachineTabs({ to, projectId }: { to: (rest?: string) => string; projectId: string | null }) {
  const { client, stream } = useConnection()
  const { data } = useQuery(machinesQuery(client))
  // The open project's runs, and this computer's own (its downloads, its updates), as on the RC: never another project's.
  const running = useQuery({ ...runsQuery(client, projectId ?? "", { status: "in_progress" }, 100), enabled: Boolean(projectId) })
  const queued = useQuery({ ...runsQuery(client, projectId ?? "", { status: "queued" }, 100), enabled: Boolean(projectId) })
  const ownRunning = useQuery(maintenanceRunsQuery(client, { status: "in_progress" }, 100))
  const ownQueued = useQuery(maintenanceRunsQuery(client, { status: "queued" }, 100))
  const { data: project } = useQuery({ ...projectQuery(client, stream, projectId ?? ""), enabled: Boolean(projectId) })
  const [open, setOpen] = useState<{ name: string; chip: HTMLElement } | null>(null)
  const [count, setCount] = useState(Number.POSITIVE_INFINITY)
  const strip = useRef<HTMLElement>(null)
  const sizes = useRef<HTMLDivElement>(null)
  const renting = !client.transport.channel
  const chosen = project?.machines ?? []
  // Every machine of this computer's, a rental still booting too, and any the project prefers.
  const machines = (data?.machines ?? [])
    .filter((machine) => machine.kind !== "rental" || (machine.state !== "ended" && (machine.ordered !== false || chosen.includes(machine.name))))
    .sort((a, b) => Number(b.kind === "local") - Number(a.kind === "local"))
  const tabs = machines.slice(0, Math.max(count, 1))
  const hidden = machines.slice(tabs.length)
  const named = machines.map((machine) => machine.name).join(" ")
  const fit = useRef(() => count)

  useLayoutEffect(() => {
    fit.current = () => {
      const [held, ghost] = [strip.current, sizes.current]
      if (!held || !ghost) return count
      const style = getComputedStyle(held)
      const space = (Number.parseFloat(style.maxWidth) || Number.POSITIVE_INFINITY) - Number.parseFloat(style.paddingLeft) - Number.parseFloat(style.paddingRight)
      const gap = Number.parseFloat(style.columnGap) || 0
      const widths = [...ghost.children].map((child) => child.getBoundingClientRect().width)
      const more = widths.pop() ?? 0
      const fits = (n: number) => [...widths.slice(0, n), ...(n < widths.length ? [more] : [])].reduce((sum, each) => sum + each + gap, -gap) <= space
      if (fits(widths.length)) return widths.length
      let n = 1
      while (n < widths.length - 1 && fits(n + 1)) n++
      return n
    }
  })
  useLayoutEffect(() => setCount(fit.current()), [named])
  // Never inside the observer: WebKit says the loop (cl-973). The window's width moves the strip's (40vw); a tab's own, the hidden copy's.
  useLayoutEffect(() => {
    let frame = 0
    const refit = () => {
      cancelAnimationFrame(frame)
      frame = requestAnimationFrame(() => flushSync(() => setCount(fit.current())))
    }
    const watch = new ResizeObserver(refit)
    if (sizes.current) watch.observe(sizes.current)
    window.addEventListener("resize", refit)
    return () => {
      cancelAnimationFrame(frame)
      watch.disconnect()
      window.removeEventListener("resize", refit)
    }
  }, [])

  const runs = [...(projectId ? [...(running.data?.runs ?? []), ...(queued.data?.runs ?? [])] : []), ...(ownRunning.data?.runs ?? []), ...(ownQueued.data?.runs ?? [])]
  const boxes = new Map((project?.assets ?? []).flatMap((asset) => (asset.making ?? []).map((number) => [number, asset.name] as const)))
  const shown = open && tabs.find((machine) => machine.name === open.name)
  return (
    <>
      {machines.length > 0 && (
        <nav className="mtabs" aria-label="Machines" ref={strip}>
          {tabs.map((machine) => (
            <button
              key={machine.name}
              type="button"
              className={`mtab ${open?.name === machine.name ? "on" : ""} ${chosen.includes(machine.name) ? "render-here" : ""}`}
              data-machine={machine.name}
              data-slow={slowWords(machine).length ? "" : undefined}
              aria-expanded={open?.name === machine.name}
              title={[[gpusWords(machine), stateWords(machine, Date.now())].filter(Boolean).join(" · "), ...slowWords(machine)].join("\n")}
              onClick={(event) => {
                const chip = event.currentTarget
                setOpen((now) => (now?.name === machine.name ? null : { name: machine.name, chip }))
              }}
            >
              <Face machine={machine} doing={doingOf(machine, runs, projectId)} />
            </button>
          ))}
          {hidden.length > 0 && (
            <Link className="mtab more" to={to("machines")} state={{ first: hidden.map((machine) => machine.name) } satisfies MachinesAsk} title="All machines">
              {hidden.length} more
            </Link>
          )}
        </nav>
      )}
      {renting && (
        <span className="ptabs-add">
          <Link className="ptab add" to={to("machines/new")} aria-label="New machine" title="New machine">
            +
          </Link>
        </span>
      )}
      <div className="mtabs-ghost" aria-hidden>
        <div ref={sizes}>
          {machines.map((machine) => (
            <span key={machine.name} className="mtab">
              <Face machine={machine} doing={doingOf(machine, runs, projectId)} />
            </span>
          ))}
          <span className="mtab">{machines.length} more</span>
        </div>
      </div>
      {open && shown && <Activity machine={shown} doing={doingOf(shown, runs, projectId)} boxes={boxes} to={to} chip={open.chip} close={() => setOpen(null)} />}
    </>
  )
}
