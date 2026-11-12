import { useQuery } from "@tanstack/react-query"
import { useRef } from "react"
import type { Machine, Operation, Run } from "../api/calcifer"
import { machinesQuery, runQuery, runsQuery } from "../api/queries"
import { useConnection } from "../app/Connection"
import { bytes, leftWords } from "../panels/format"
import { useNow } from "../panels/common"
import { modelWords, packageWords, useInstalling, useWaitsOn } from "../panels/machines/activity"
import { countsOf, countWords, stageName, workWords } from "../panels/runs/words"
import { useSignal } from "../ui/signal"
import type { ElementView } from "./CanvasStore"
import { useCanvasStore, useElement } from "./context"

/** Where a render is, for its box's face: what it's doing in words, how far the whole of it is and how long it has left, where it runs,
 *  the detail of what it's doing with its numbers, and what it warned of. Every phase says something: nothing on it is wordless. */
export interface Face {
  /** `queued`, `renting`, `uploading`, `install`, `download`, `work`, `result`, `canceling`, `failed`: how its bar is drawn. */
  phase: string
  says: string
  /** How far the whole render is, 0 to 1, by its time left; else how far what it's doing now is; none: it doesn't say. */
  fraction?: number
  /** The stage it's in, within its parts (`Segment 2 of 3 · Decoding video · frame 173 of 350`), and how far that stage is. */
  stage?: string
  stageFraction?: number
  left?: string
  where?: string
  detail?: string
  warnings: string[]
}

const sentence = (said: string) => said.charAt(0).toUpperCase() + said.slice(1)

/** A GPU as people say it: `NVIDIA GeForce RTX 4090` is `RTX 4090`. */
const gpuWords = (name: string) => name.replace(/^NVIDIA\s+/i, "").replace(/^GeForce\s+/i, "").replace(/\s+Laptop GPU$/i, " Laptop")

/** A machine and its GPUs: `local · RTX 4070 Laptop`, `kushina · 2× RTX 4090`. */
function machineWords(machine: Machine): string {
  const gpus = machine.status?.gpus ?? []
  const card = gpus[0]?.name ?? machine.rental?.accelerator ?? machine.rental?.sku
  if (!card) return machine.name
  const count = gpus.length || machine.rental?.gpus || 1
  return `${machine.name} · ${count > 1 ? `${count}× ` : ""}${gpuWords(card)}`
}

/** A rental's card, as it's rented: `an RTX 4090`, `2× H100`. */
function rentedWords(machine: Machine): string {
  const card = gpuWords(machine.rental?.accelerator || machine.rental?.sku?.toUpperCase() || "GPU machine")
  const count = machine.rental?.gpus ?? 1
  // As it's said: an RTX 4090, an H100, an A100, a B200.
  return count > 1 ? `${count}× ${card}` : `${/^([AEFHILMNORSX8]|1[18])/.test(card) ? "an" : "a"} ${card}`
}

/** Its time left, as Calcifer estimates the whole run's: `about 3 min left` while that's little trusted. */
function runLeft(run: Run): string | undefined {
  const progress = run.progress
  if (progress?.eta_s != null) return `${(progress.eta_confidence ?? 1) < 0.5 ? "about " : ""}${leftWords(progress.eta_s)}`
  if (progress?.eta_basis === "first_run") return "first run on this machine"
  return undefined
}

/** What it waits on as its machine says it, without the machine's state its reason repeats (`preparing: installing x`), each run it names
 *  by its box where this project has one: `waiting for <Kyogre> to end`. */
const reason = (run: Run, boxOf: (run: number) => string | undefined) =>
  run.waiting?.replace(/^(queued|preparing|running): /, "").replace(/\brun (\d+)/g, (said, number: string) => {
    const box = boxOf(Number(number))
    return box ? `<${box}>` : said
  })

/** A download's numbers: `Qwen Image 2.1 · 3.1 GiB of 12 GiB · 40 MiB/s`. */
function fetched(operation: Operation): string {
  const parts = [modelWords(operation.subject), operation.bytes_total ? `${bytes(operation.bytes_done)} of ${bytes(operation.bytes_total)}` : bytes(operation.bytes_done)]
  if (operation.bytes_per_s) parts.push(`${bytes(operation.bytes_per_s)}/s`)
  if (operation.bytes_here) parts.push(`${bytes(operation.bytes_here)} already here`)
  return parts.join(" · ")
}

/** Its results not here yet: their bytes. */
function owed(run: Run): number | undefined {
  const items = run.output.filter((item) => !item.stays && !item.checkpoint && !item.url)
  return items.length ? items.reduce((sum, item) => sum + item.length, 0) : undefined
}

export interface Seen {
  view: ElementView
  run?: Run
  machine?: Machine
  /** What its run waits on there: its package's install or its weights. */
  on?: Operation
  /** Its package installing before its run is sent. */
  installing?: Operation
  /** The box it waits for, and that one's run. */
  waits?: string
  theirs?: Run
  /** The boxes ahead of it in its project's line. */
  ahead?: string[]
  /** A machine being made ready before anything is sent to it. */
  readying?: string
  canceling: boolean
  /** The box a run of this project makes. */
  boxOf: (run: number) => string | undefined
}

/** Its face, by what is known of it: the most specific first. */
export function faceOf(seen: Seen): Face {
  const { view, run, machine, on, installing, waits, theirs, ahead, readying } = seen
  const warnings = [...(run?.warnings ?? []).map((warning) => warning.message)]
  if (run?.stalled) warnings.push(`No progress for ${Math.max(1, Math.round((Date.now() - run.stalled.since_ms) / 60_000))} min on ${run.stalled.machine}`)
  const where = machine ? (machine.state === "booting" && machine.kind === "rental" ? `Renting ${rentedWords(machine)}… · ${machine.name}` : machineWords(machine)) : undefined
  const face = (phase: string, says: string, more: Partial<Face> = {}): Face => ({ phase, says, where, warnings, left: run && runLeft(run), ...more })
  if (seen.canceling || run?.cancel_requested) return face("canceling", "Cancelling…")
  if (run?.status === "failed") return face("failed", "Failed", { detail: run.reason?.message, left: undefined })
  if (waits && !on) {
    const words = theirs && theirs.status !== "completed" ? workWords(theirs) : ""
    return face("queued", `Waits for <${waits}>`, { detail: words || "until it's made", fraction: theirs?.progress?.done_fraction ?? undefined })
  }
  if (!run) {
    if (installing) return face("install", `Installing ${packageWords(installing.subject)}…`, { detail: sentence(installing.stage || "installing") })
    if (readying) return face("queued", `Getting ${readying} ready`, { detail: "before anything is sent to it" })
    if (view.queued) return face("queued", "Queued", { detail: ahead?.length ? `${ahead.length} ahead: ${ahead.slice(0, 3).join(", ")}${ahead.length > 3 ? "…" : ""}` : "next" })
    return face("queued", "Queued", { detail: "sending it to a machine" })
  }
  const waiting = reason(run, seen.boxOf)
  if (!run.machine) {
    const renting = waiting?.match(/^renting (.+)$/)
    if (renting) return face("renting", `Renting ${renting[1]}…`, { detail: "ordering it" })
    return face("queued", "Choosing a machine", { detail: waiting ? sentence(waiting) : undefined, left: undefined })
  }
  if (machine?.state === "booting") return face("renting", machine.kind === "rental" ? `Renting ${rentedWords(machine)}…` : `Starting ${machine.name}`, { detail: machine.boot ? sentence(machine.boot) : "booting" })
  if (waiting?.startsWith("uploading ")) return face("uploading", "Uploading inputs", { detail: sentence(waiting.slice("uploading ".length)) })
  if (on?.kind === "install") return face("install", `Installing ${packageWords(on.subject)}`, { detail: sentence(on.stage || "installing") })
  if (on?.kind === "download") {
    const fraction = on.bytes_total ? on.bytes_done / on.bytes_total : undefined
    const own = runLeft(run)
    return face("download", "Downloading weights", { detail: fetched(on), fraction, left: own ?? (on.eta_s != null ? `at least ${leftWords(on.eta_s)}` : undefined) })
  }
  if (run.queue_position != null) return face("queued", `Queued on ${run.machine}`, { detail: [`number ${run.queue_position} in line`, waiting].filter(Boolean).join(" · ") })
  if (run.status === "completed") {
    const left = owed(run)
    return face("result", left ? "Downloading the result" : "Saving", { detail: left ? bytes(left) : undefined })
  }
  const progress = run.progress
  if (progress?.stage || progress?.total) {
    const count = countWords(progress)
    const compiling = /compil/i.test(progress.stage)
    const phase = progress.bytes_total ? "download" : /install/i.test(progress.stage) ? "install" : "work"
    const detail = [progress.bytes_per_s ? `${bytes(progress.bytes_per_s)}/s` : "", compiling ? "first run on this machine" : "", waiting ? sentence(waiting) : ""].filter(Boolean).join(" · ")
    // Its bar is the whole render's (Calcifer's, its parts in); the stage's own goes under it. A decode's tokens are no share of their cap.
    const tokens = countsOf(progress) === "token"
    const own = progress.stage_fraction ?? (progress.total && !tokens ? progress.completed / progress.total : progress.bytes_total ? progress.bytes_done / progress.bytes_total : undefined)
    const stage = `${stageName(progress.stage)}${count ? ` · ${count}` : ""}`
    return face(phase, stageName(progress.stage.split(" / ").at(-1) ?? ""), { detail: detail || undefined, fraction: progress.done_fraction ?? undefined, stage, stageFraction: tokens ? undefined : own })
  }
  if (waiting) return face("queued", sentence(waiting))
  return face("work", run.machine_state === "preparing" ? "Preparing" : "Starting", { detail: `on ${run.machine}` })
}

/** How far its bar is: the whole render by its time, what's passed of what it has taken and has left; else what it's doing now, by its own
 *  count. Neither goes back while it stands on the same. */
function useBar(run: Run | undefined, face: Face, now: number): number | undefined {
  const held = useRef<{ on: string; fraction: number } | null>(null)
  const left = run?.progress?.eta_s
  const since = run ? now - run.created_at_ms : 0
  const whole = run && left != null ? Math.min(0.99, since / Math.max(1, since + left * 1000)) : undefined
  const fraction = whole ?? face.fraction
  if (fraction === undefined) return undefined
  // Its stages' bar is the whole render's too (Calcifer's `done_fraction`, its parts in): one source while it works.
  const on = whole !== undefined ? `${run!.number}` : face.phase === "work" ? `${run?.number}:work` : `${run?.number}:${face.phase}:${face.says}`
  const was = held.current?.on === on ? held.current.fraction : 0
  held.current = { on, fraction: Math.max(was, fraction) }
  return held.current.fraction
}

/** A render on its way, on its box's face: its state in words, one bar for the whole of it and its time left, Cancel; then where it runs,
 *  what it's doing with its numbers, and what it warned of. Over the take being looked at, or a glimpse of what it's made, only its first
 *  lines, along the bottom. The take looked at may be one asked after the first going: it's that one's. */
export function RunFace({ view, name, number, full, above = false }: { view: ElementView; name: string; number?: number; full: boolean; above?: boolean }) {
  const { client } = useConnection()
  const store = useCanvasStore()
  const tools = useSignal(store.tools)
  useSignal(store.names)
  const asset = view.asset
  const making = asset?.standing === "making"
  const later = number !== undefined && number !== view.run?.number
  const { data } = useQuery({ ...runQuery(client, number ?? 0), enabled: later })
  const run = later ? data : view.run
  const on = useWaitsOn(run)
  const { data: machines } = useQuery(machinesQuery(client))
  const machine = run?.machine ? machines?.machines.find((held) => held.name === run.machine) : undefined
  // Waiting for what it reads, it says that, and nothing of a machine's work for another box.
  const onItsWay = making || Boolean(view.queued) || view.asked !== undefined
  const waits = making || !onItsWay ? null : store.waitsFor(name)
  const installsOf = asset?.tool ? tools.get(asset.tool) : undefined
  const installing = useInstalling(run || waits ? undefined : installsOf && (installsOf.installs ?? installsOf.package))
  const read = useElement(store.mentionOf(waits ?? name, "")?.name ?? name)
  const going = useQuery({ ...runsQuery(client, { status: "in_progress" }, 100), enabled: !run })
  const lined = useQuery({ ...runsQuery(client, { status: "queued" }, 100), enabled: !run })
  const settingUp = (other: Run) => other.kind === "update" || other.target.startsWith("install ")
  const readying = run ? undefined : ([...(going.data?.runs ?? []), ...(lined.data?.runs ?? [])].find(settingUp)?.machine ?? undefined)
  const now = useNow(true)
  const face = faceOf({
    view,
    run,
    machine,
    on,
    installing,
    waits: waits && !on ? waits : undefined,
    theirs: waits && read?.asset?.making?.length ? read.run : undefined,
    ahead: store.aheadOf(name),
    readying,
    canceling: Boolean(view.canceling),
    boxOf: (number) => store.boxOfRun(number),
  })
  const fraction = useBar(run, face, now)
  // Made, its result on its way here, there's nothing left to cancel.
  const cancel = !["canceling", "failed", "result"].includes(face.phase)
  return (
    <div
      className={`prog run-face ${above ? "above" : full ? "full" : "strip"} ${face.phase} ${fraction === undefined ? "unknown" : ""}`}
      data-phase={face.phase}
    >
      <div className="face">
        <div className="face-state">
          <b>{face.says}</b>
          {cancel && (
            <button
              type="button"
              className="face-cancel nodrag"
              aria-label={`Cancel ${name}'s render`}
              onClick={(event) => {
                // Its box isn't opened by it.
                event.stopPropagation()
                store.cancelRender(name)
              }}
            >
              Cancel
            </button>
          )}
        </div>
        <div className="face-bar">
          <span className="face-track" role="progressbar" aria-label={face.says} aria-valuenow={fraction === undefined ? undefined : Math.round(fraction * 100)}>
            <i style={fraction === undefined ? undefined : { transform: `translateX(${Math.round(fraction * 1000) / 10 - 100}%)` }} />
          </span>
          {face.left && <span className="face-left">{face.left}</span>}
        </div>
        {(full || above) && face.stage && (
          <div className="face-part">
            <p className="face-stage" title={face.stage}>
              {face.stage}
            </p>
            {face.stageFraction !== undefined && (
              <span className="face-track thin">
                <i style={{ transform: `translateX(${Math.round(face.stageFraction * 1000) / 10 - 100}%)` }} />
              </span>
            )}
          </div>
        )}
        {(full || above) && face.where && <p className="face-where">{face.where}</p>}
        {(full || above) && face.detail && (
          <p className="face-detail" title={face.detail}>
            {face.detail}
          </p>
        )}
        {(full || above) && face.warnings.map((warning) => (
          <p key={warning} className="face-warning" title={warning}>
            {warning}
          </p>
        ))}
      </div>
    </div>
  )
}
