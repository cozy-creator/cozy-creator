import { useQuery } from "@tanstack/react-query"
import { useEffect, useLayoutEffect, useRef, useState } from "react"
import { Link } from "react-router-dom"
import { machinesQuery, maintenanceRunsQuery, projectQuery, runsQuery } from "../../api/queries"
import { useConnection } from "../../app/Connection"
import type { Machine } from "../../api/calcifer"
import { Icon } from "../../ui/Icon"
import { Bar } from "../common"
import { workWords } from "../runs/words"
import { Checks, Spec } from "./RenderHere"
import { type Doing, doingOf, fetchedWords, fractionOf, operationName, runName, waits, working } from "./activity"
import { called, gpusWords, slowWords, stableName, stateWords } from "./words"

type Mark = "broken" | "slow" | "up" | "download" | "install" | "work" | "idle"

/** Red blinks broken and yellow coming up. Up, each thing it does: an arrow glowing while weights arrive, a package while one installs, green
 *  blinking while it runs inference; steady grey with nothing to do. Steady red first while something makes its runs slow (cl-913). */
function marksOf(machine: Machine, doing: Doing[]): Mark[] {
  if (machine.offline || machine.end_failed || machine.unreachable_since_ms || machine.rental?.unreachable_since_ms) return ["broken"]
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

const sentence = (text: string) => text.charAt(0).toUpperCase() + text.slice(1)

const numbers = (runs: number[]) => (runs.length === 1 ? `run ${runs[0]}` : `runs ${runs.join(", ")}`)

function Row({ doing, boxes, to }: { doing: Doing; boxes: Map<number, string>; to: (rest?: string) => string }) {
  if (doing.kind === "boot")
    return (
      <li className="doing">
        <MarkOf mark="up" />
        <b>Booting</b>
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
          {doing.kind === "download" ? fetchedWords(operation) : sentence(operation.stage || "installing")} · for {numbers(operation.runs)}
        </span>
      </li>
    )
  }
  const { run, on } = doing
  const fraction = run.progress?.done_fraction
  const going = working(run, on)
  const said = on
    ? `Waits on the ${on.kind} of ${operationName(on)}`
    : waits(run)
      ? (run.waiting ?? (run.queue_position ? `Queued, ${run.queue_position} in line` : "Queued"))
      : workWords(run) || run.waiting
  return (
    <li className="doing" data-doing="run" data-run={run.number}>
      <MarkOf mark={going ? "work" : "idle"} />
      <Link to={run.project_id ? to(`runs/${run.number}`) : `/runs/${run.number}`} title={`Run ${run.number}: ${run.target}`}>
        <b>{runName(run, boxes)}</b>
      </Link>
      <span className="kind">Run {run.number}</span>
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
        <p className="said">Nothing to do</p>
      )}
    </div>
  )
}

/** Each machine, top right, `local` first: what it's doing at a glance, and a click shows each thing with how far it is, and Render here,
 *  which makes it one of the open project's preferred machines, green (Paul 11-12). Kept by the stream. */
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
  const chosen = project?.machines ?? []
  // Every machine of this computer's, a rental still booting too, and any the project prefers.
  const machines = (data?.machines ?? [])
    .filter((machine) => machine.kind !== "rental" || (machine.state !== "ended" && (machine.ordered !== false || chosen.includes(machine.name))))
    .sort((a, b) => Number(b.kind === "local") - Number(a.kind === "local"))
  if (!machines.length) return null
  const runs = [...(projectId ? [...(running.data?.runs ?? []), ...(queued.data?.runs ?? [])] : []), ...(ownRunning.data?.runs ?? []), ...(ownQueued.data?.runs ?? [])]
  const boxes = new Map((project?.assets ?? []).flatMap((asset) => (asset.making ?? []).map((number) => [number, asset.name] as const)))
  const shown = open && machines.find((machine) => machine.name === open.name)
  return (
    <>
      <nav className="mtabs" aria-label="Machines">
        {machines.map((machine) => {
          const doing = doingOf(machine, runs, projectId)
          // Its jobs are function runs, its GPUs' work: an install or a download is listed, not counted (cl-889).
          const total = doing.filter((one) => one.kind === "run" && one.run.kind === "call").length
          return (
            <button
              key={machine.name}
              type="button"
              className={`mtab ${open?.name === machine.name ? "on" : ""} ${chosen.includes(machine.name) ? "render-here" : ""}`}
              data-machine={machine.name}
              data-slow={slowWords(machine).length ? "" : undefined}
              aria-expanded={open?.name === machine.name}
              title={[`${gpusWords(machine)}, ${stateWords(machine, Date.now())}`, ...slowWords(machine)].join("\n")}
              onClick={(event) => {
                const chip = event.currentTarget
                setOpen((now) => (now?.name === machine.name ? null : { name: machine.name, chip }))
              }}
            >
              {marksOf(machine, doing).map((mark) => (
                <MarkOf key={mark} mark={mark} />
              ))}
              <b>{called(machine)}</b>
              {total > 0 && <span className="load">{total}</span>}
            </button>
          )
        })}
      </nav>
      {open && shown && <Activity machine={shown} doing={doingOf(shown, runs, projectId)} boxes={boxes} to={to} chip={open.chip} close={() => setOpen(null)} />}
    </>
  )
}
