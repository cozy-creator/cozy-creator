import { useQuery } from "@tanstack/react-query"
import { useRef } from "react"
import type { Ahead, Machine, Operation, Run } from "../api/calcifer"
import { machinesQuery, runQuery } from "../api/queries"
import { useConnection } from "../app/Connection"
import { useNow } from "../panels/common"
import { modelWords, packageWords, useInstalling, useWaitsOn } from "../panels/machines/activity"
import { size, stageLine } from "../panels/runs/words"
import { useSignal } from "../ui/signal"
import type { ElementView } from "./CanvasStore"
import { useCanvasStore, useElement } from "./context"
import { shownOf } from "./shown"

/**
 * A render on its way, as Paul wrote it (11-12), two plain lines and nothing else:
 *
 *     Jinhyeok - 3s / 2m 5s
 *     Denoising image - Step 7 / 8
 *
 * the machine, then once there's an estimate its time so far over the whole job's; then what it's doing in words, then how far within
 * that (step, frame, tokens, bytes), then its rate where there is one. No box, panel or bar. Cancel is a small control beside them.
 */
export interface Face {
  /** `queued`, `renting`, `install`, `download`, `work`, `result`, `canceling`, `failed`. */
  phase: string
  /** The machine, or what it's waiting to be placed on. */
  machine: string
  /** What it's doing, with how far and how fast. */
  state: string
}

/** A span as Paul writes it: `3s`, `2m 5s`, `5m`, `1h 4m`. */
export function short(s: number): string {
  const all = Math.max(0, Math.round(s))
  if (all < 60) return `${all}s`
  if (all < 3600) return all % 60 ? `${Math.floor(all / 60)}m ${all % 60}s` : `${all / 60}m`
  const m = Math.floor((all % 3600) / 60)
  return m ? `${Math.floor(all / 3600)}h ${m}m` : `${Math.floor(all / 3600)}h`
}

const sentence = (said: string) => said.charAt(0).toUpperCase() + said.slice(1)

/** A rate as Paul writes it: `100 MBps`. */
const rateWords = (rate: number) => size(rate).replace(/([KMGT]?B)$/, " $1ps")

/** Bytes moving: `2.1GB / 7GB - 100 MBps`. */
const moving = (done: number, total: number, rate?: number | null) => `${size(done)}${total ? ` / ${size(total)}` : ""}${rate ? ` - ${rateWords(rate)}` : ""}`

/** The whole job's time left, Calcifer's: once it ended, its result coming here. */
const leftOf = (run: Run) => (run.status === "completed" ? run.arriving?.eta_s : run.progress?.eta_s) ?? undefined

/** What it waits on as its machine says it, without the machine's state its reason repeats, each run it names by its box. */
const reason = (run: Run, boxOf: (run: number) => string | undefined) =>
  run.waiting?.replace(/^(queued|preparing|running): /, "").replace(/\brun (\d+)/g, (said, number: string) => {
    const box = boxOf(Number(number))
    return box ? `<${box}>` : said
  })

/** The run whose take is shown before its file is here, while that file comes: its box says so (Paul, 11-12: no take looks done before
 *  it's here). None for a file a machine keeps. */
export function useArriving(view: ElementView | undefined): number | undefined {
  const { client } = useConnection()
  const store = useCanvasStore()
  const shown = view ? shownOf(view) : null
  const number = shown?.absent ? shown.run : undefined
  const { data } = useQuery({ ...runQuery(client, store.projectId ?? "", number ?? 0), enabled: Boolean(store.projectId && number !== undefined) })
  return data && data.status === "completed" && owed(data) !== undefined ? number : undefined
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
  /** The box it waits for: that one's render is that box's to show, not this one's (cl-905). */
  waits?: string
  /** What its machine prepares for it meanwhile: its code and weights, fetched ahead. */
  preparing?: Ahead
  /** The boxes ahead of it in its project's line. */
  ahead?: string[]
  canceling: boolean
  boxOf: (run: number) => string | undefined
}

/** Its two lines' words, by what is known of it: the most specific first. */
export function faceOf(seen: Seen): Face {
  const { view, run, machine, on, installing, waits, ahead, preparing } = seen
  const rented = machine?.kind === "rental" && machine.state === "booting"
  const card = machine?.rental?.accelerator || machine?.rental?.sku?.toUpperCase()
  const where = run?.machine ? (rented && card ? `${run.machine} (renting ${card})` : run.machine) : "Choosing a machine"
  const face = (phase: string, state: string, on = where): Face => ({ phase, machine: on, state })
  if (seen.canceling || run?.cancel_requested) return face("canceling", "Cancelling…")
  if (run?.status === "failed") return face("failed", `Failed: ${run.reason?.message ?? "its run failed"}`)
  if (waits && !on) return prepared(`Waiting for <${waits}>`, preparing)
  if (!run) {
    if (installing) return face("install", `Installing ${packageWords(installing.subject)}`, "Choosing a machine")
    if (view.queued && (preparing?.stage || preparing?.ready)) return prepared(`Queued: Position ${(ahead?.length ?? 0) + 1}`, preparing)
    if (view.queued) return face("queued", `Queued: Position ${(ahead?.length ?? 0) + 1}`, "Choosing a machine")
    return face("queued", "Queued", "Choosing a machine")
  }
  const waiting = reason(run, seen.boxOf)
  if (!run.machine) {
    const renting = waiting?.match(/^renting (.+)$/)
    return renting ? face("renting", "Ordering it", `Renting ${renting[1]}`) : face("queued", run.queue_position != null ? `Queued: Position ${run.queue_position}` : "Queued")
  }
  if (machine?.state === "booting") return face("renting", machine.boot ? sentence(machine.boot) : "Booting")
  if (waiting?.startsWith("uploading ")) return face("uploading", `Uploading ${waiting.slice("uploading ".length)}`)
  if (on?.kind === "install") return face("install", `Installing ${packageWords(on.subject)}${on.stage ? ` - ${on.stage}` : ""}`)
  if (on?.kind === "download") return face("download", `Downloading weights - ${modelWords(on.subject)} - ${moving(on.bytes_done, on.bytes_total, on.bytes_per_s)}`)
  if (run.queue_position != null) return face("queued", `Queued: Position ${run.queue_position}`)
  if (run.status === "completed") {
    const arriving = run.arriving
    if (arriving?.bytes_total) return face("result", `Downloading result - ${moving(arriving.bytes_done, arriving.bytes_total, arriving.bytes_per_s)}`)
    return face("result", owed(run) ? "Downloading result" : "Saving")
  }
  const progress = run.progress
  if (progress?.stage || progress?.total) {
    const line = stageLine(progress)
    const phase = progress.bytes_total ? "download" : /install/i.test(progress.stage) ? "install" : "work"
    return face(phase, progress.bytes_per_s ? `${line} - ${rateWords(progress.bytes_per_s)}` : line)
  }
  if (waiting) return face("queued", sentence(waiting))
  return face("work", run.machine_state === "preparing" ? "Preparing" : "Starting")
}

/** How far the whole job is, by line 1's numbers: what's passed of what it has taken and has left. None until there's an estimate; it
 *  never goes back while it's the same run. */
function useWhole(run: Run | undefined, since: number | undefined, left: number | undefined): number | undefined {
  const held = useRef<{ run: number; fraction: number } | null>(null)
  if (!run || since === undefined || left === undefined) return undefined
  const fraction = Math.min(0.99, since / Math.max(1, since + left))
  const was = held.current?.run === run.number ? held.current.fraction : 0
  held.current = { run: run.number, fraction: Math.max(was, fraction) }
  return held.current.fraction
}

/** A box waiting its turn, by what its machine prepares for it meanwhile (cl-905): its own install and download, on the machine it will
 *  run on, never another box's render. */
function prepared(state: string, row?: Ahead): Face {
  if (!row) return { phase: "queued", machine: "Choosing a machine", state }
  if (row.held_back) return { phase: "queued", machine: row.machine, state: `${state} - Not prepared: ${row.held_back}` }
  if (row.ready) return { phase: "queued", machine: row.machine, state: `${state} - Ready` }
  if (!row.stage) return { phase: "queued", machine: "Choosing a machine", state }
  const counted = row.total > 0
  const phase = counted || /download/i.test(row.stage) ? "download" : /install/i.test(row.stage) ? "install" : "queued"
  const parts = [state, `Preparing: ${row.stage}`, counted ? moving(row.done, row.total) : "", row.waiting ? sentence(row.waiting) : ""]
  return { phase, machine: row.machine, state: parts.filter(Boolean).join(" - ") }
}

/** A render on its way, two plain lines (Paul, 11-12): its machine and time, then what it's doing with how far and how fast; Cancel
 *  beside them. On the canvas at its box's text size; in Focus, between its name and its picture, the same two lines. */
export function RunFace({ view, name, number, above = false }: { view: ElementView; name: string; number?: number; full?: boolean; above?: boolean }) {
  const { client } = useConnection()
  const store = useCanvasStore()
  const tools = useSignal(store.tools)
  useSignal(store.names)
  const asset = view.asset
  const making = asset?.standing === "making"
  const later = number !== undefined && number !== view.run?.number
  const { data } = useQuery({ ...runQuery(client, store.projectId ?? "", number ?? 0), enabled: Boolean(store.projectId && later) })
  const run = later ? data : view.run
  const on = useWaitsOn(run)
  const { data: machines } = useQuery(machinesQuery(client))
  const machine = run?.machine ? machines?.machines.find((held) => held.name === run.machine) : undefined
  // Waiting for what it reads, it says that, and nothing of a machine's work for another box.
  const onItsWay = making || Boolean(view.queued) || view.asked !== undefined
  const waits = making || !onItsWay ? null : store.waitsFor(name)
  const installsOf = asset?.tool ? tools.get(asset.tool) : undefined
  const installing = useInstalling(run || waits ? undefined : installsOf && (installsOf.installs ?? installsOf.package))
  useElement(store.mentionOf(waits ?? name, "")?.name ?? name)
  useSignal(store.ahead)
  const now = useNow(true)
  const face = faceOf({ view, run, machine, on, installing, waits: waits && !on ? waits : undefined, ahead: store.aheadOf(name), preparing: store.preparing(name), canceling: Boolean(view.canceling), boxOf: (n) => store.boxOfRun(n) })
  const since = run ? (now - run.created_at_ms) / 1000 : undefined
  const left = run ? leftOf(run) : undefined
  const time = since !== undefined && left != null ? ` - ${short(since)} / ${short(since + left)}` : ""
  const cancel = !["canceling", "failed", "result"].includes(face.phase)
  // At rest, a meter of the whole job along its box's bottom edge, only for a true fraction (cl-928); Focus has the lines alone.
  const whole = useWhole(run, since, left ?? undefined)
  const meter = !above && !["canceling", "failed"].includes(face.phase) ? whole : undefined
  return (
    <div className={`prog run-face ${above ? "above" : "lines"} ${face.phase}`} data-phase={face.phase}>
      <p className="face-machine">{`${face.machine}${time}`}</p>
      <p className="face-state">
        <b>{face.state}</b>
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
      </p>
      {meter !== undefined && (
        <span className="face-meter" role="progressbar" aria-label="The whole job" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(meter * 100)}>
          <i style={{ transform: `scaleX(${meter})` }} />
        </span>
      )}
    </div>
  )
}
