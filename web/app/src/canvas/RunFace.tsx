import { useQuery } from "@tanstack/react-query"
import { useRef } from "react"
import type { Ahead, Machine, Operation, Run } from "../api/calcifer"
import { machinesQuery, runQuery } from "../api/queries"
import { useConnection } from "../app/Connection"
import { useNow } from "../panels/common"
import { modelWords, packageWords, useInstalling, useWaitsOn } from "../panels/machines/activity"
import { size, stageParts } from "../panels/runs/words"
import { useSignal } from "../ui/signal"
import type { ElementView } from "./CanvasStore"
import { useCanvasStore, useElement } from "./context"
import { shownOf, watched } from "./shown"

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
  /** What it's doing: the words, which give way to a narrow box. */
  state: string
  /** How far within it and how fast (`Frame 3 / 12`, `2GB / 7GB - 100 MBps`): never cut (cl-953). */
  count?: string
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
  const face = (phase: string, state: string, on = where, count?: string): Face => ({ phase, machine: on, state, ...(count ? { count } : {}) })
  if (seen.canceling || run?.cancel_requested) return face("canceling", "Cancelling…")
  if (run?.status === "failed") return face("failed", `Failed: ${run.reason?.message ?? "its run failed"}`)
  if (waits && !on) return prepared(`Waiting for <${waits}>`, preparing)
  if (!run) {
    if (installing) return face("install", `Installing ${packageWords(installing.subject)}`, "Choosing a machine")
    if (view.queued && (preparing?.stage || preparing?.ready)) return prepared(`Queued: Position ${(ahead?.length ?? 0) + 1}`, preparing)
    if (view.queued) return face("queued", "Queued:", "Choosing a machine", `Position ${(ahead?.length ?? 0) + 1}`)
    return face("queued", "Queued", "Choosing a machine")
  }
  const waiting = reason(run, seen.boxOf)
  if (!run.machine) {
    const renting = waiting?.match(/^renting (.+)$/)
    return renting ? face("renting", "Ordering it", `Renting ${renting[1]}`) : run.queue_position != null ? face("queued", "Queued:", where, `Position ${run.queue_position}`) : face("queued", "Queued")
  }
  if (machine?.state === "booting") return face("renting", machine.boot ? sentence(machine.boot) : "Booting")
  if (waiting?.startsWith("uploading ")) return face("uploading", `Uploading ${waiting.slice("uploading ".length)}`)
  if (on?.kind === "install") return face("install", `Installing ${packageWords(on.subject)}${on.stage ? ` - ${on.stage}` : ""}`)
  if (on?.kind === "download") return face("download", `Downloading weights - ${modelWords(on.subject)} -`, where, moving(on.bytes_done, on.bytes_total, on.bytes_per_s))
  if (run.queue_position != null) return face("queued", "Queued:", where, `Position ${run.queue_position}`)
  if (run.status === "completed") {
    const arriving = run.arriving
    if (arriving?.bytes_total) return face("result", "Downloading result -", where, moving(arriving.bytes_done, arriving.bytes_total, arriving.bytes_per_s))
    return face("result", owed(run) ? "Downloading result" : "Saving")
  }
  const progress = run.progress
  if (progress?.stage || progress?.total) {
    const { words, count } = stageParts(progress)
    const phase = progress.bytes_total ? "download" : /install/i.test(progress.stage) ? "install" : "work"
    const numbers = [count, progress.bytes_per_s ? rateWords(progress.bytes_per_s) : ""].filter(Boolean).join(" - ")
    return face(phase, count || !numbers ? words : `${words} -`, where, numbers || undefined)
  }
  if (waiting) return face("queued", sentence(waiting))
  return face("work", run.machine_state === "preparing" ? "Preparing" : "Starting")
}

/** How far the whole job is, by line 1's numbers: first what's passed of what it has taken and has left, then each moment moves it that
 *  moment's share of the time left over what it has still to go. It never goes back while it's the same run, a longer estimate slows it,
 *  and one that was too short never pins it near its end: it reaches the end as the estimate does. None until there's an estimate; no
 *  time left while it still runs (it ran past what was expected) says nothing, and it holds. */
function useWhole(run: Run | undefined, since: number | undefined, left: number | undefined): number | undefined {
  const held = useRef<{ run: number; fraction: number; since: number } | null>(null)
  if (!run || since === undefined || left === undefined) return undefined
  const was = held.current?.run === run.number ? held.current : null
  if (!was && left <= 0) return undefined
  const passed = was ? Math.max(0, since - was.since) : 0
  const fraction = !was ? Math.max(0, since) / (Math.max(0, since) + left) : left > 0 ? was.fraction + ((1 - was.fraction) * passed) / (passed + left) : was.fraction
  held.current = { run: run.number, fraction: Math.min(0.99, fraction), since }
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
  // Its bytes are its count, which never gives way; with what it waits on after them, all of it is words, as it reads.
  if (counted && !row.waiting) return { phase, machine: row.machine, state: `${state} - Preparing: ${row.stage} -`, count: moving(row.done, row.total) }
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
  const installing = useInstalling(run || waits ? undefined : installsOf && (installsOf.installs ?? installsOf.package), store.projectId)
  useElement(store.mentionOf(waits ?? name, "")?.name ?? name)
  useSignal(store.ahead)
  // Its clock ticks only while it has a time to show.
  const now = useNow(run !== undefined && leftOf(run) !== undefined)
  const face = faceOf({ view, run, machine, on, installing, waits: waits && !on ? waits : undefined, ahead: store.aheadOf(name), preparing: store.preparing(name), canceling: Boolean(view.canceling), boxOf: (n) => store.boxOfRun(n) })
  const since = run ? (now - run.created_at_ms) / 1000 : undefined
  const left = run ? leftOf(run) : undefined
  const time = since !== undefined && left != null ? ` - ${short(since)} / ${short(since + left)}` : ""
  const cancel = !["canceling", "failed", "result"].includes(face.phase)
  return (
    <div className={`prog run-face ${above ? "above" : "lines"} ${face.phase}`} role="status" aria-label={`${name}'s render`} data-phase={face.phase} data-run={number}>
      {/* Its words give way to a narrow box; its numbers never do (cl-953). */}
      <p className="face-machine">
        <span className="face-where">{face.machine}</span>
        {time && <span className="face-time">{time}</span>}
      </p>
      <p className="face-state">
        <b>
          <span className="face-words">{face.count ? `${face.state} ` : face.state}</span>
          {face.count && <span className="face-count">{face.count}</span>}
        </b>
        {cancel && (
          <button
            type="button"
            className="face-cancel nodrag"
            aria-label={`Cancel ${name}'s render`}
            onClick={(event) => {
              // Its box isn't opened by it. The take looked at, by its id, when one is: its others go on (cl-945).
              event.stopPropagation()
              const take = watched(view)
              if (take) store.cancelTake(name, { id: take.id, run: take.run })
              else store.cancelRender(name)
            }}
          >
            Cancel
          </button>
        )}
      </p>
    </div>
  )
}

/** In Focus, once its take has landed, its render's row stays and says so (Paul, 11-12): where it ran, then the take and how long the job
 *  took. The same two lines in the same place, so nothing under them moves. */
export function RunDone({ view, number }: { view: ElementView; number: number }) {
  const { client } = useConnection()
  const project = useCanvasStore().projectId
  const later = number !== view.run?.number
  const { data } = useQuery({ ...runQuery(client, project ?? "", number), enabled: Boolean(project && later) })
  const run = later ? data : view.run
  const made = view.asset?.prefixes?.find((version) => version.run === number)
  const ms = run?.settled_at_ms ? run.settled_at_ms - run.created_at_ms : made?.execution_ms
  return (
    <div className="prog run-face above done" data-phase="done" data-run={number}>
      <p className="face-machine">
        <span className="face-where">{run?.machine ?? ""}</span>
      </p>
      <p className="face-state">
        <b>
          <span className="face-words">{made ? `${view.asset?.noun ?? "take"} ${made.number} done${ms ? ` in ${short(ms / 1000)}` : ""}` : ""}</span>
        </b>
      </p>
    </div>
  )
}

/** At rest, how far the whole job is, as a thin meter along the top of its box (cl-928; Paul, 11-12: at the top): line 1's numbers, drawn
 *  only once there's an estimate. Focus has the lines alone. */
export function RunMeter({ view, number }: { view: ElementView; number?: number }) {
  const { client } = useConnection()
  const store = useCanvasStore()
  const later = number !== undefined && number !== view.run?.number
  const { data } = useQuery({ ...runQuery(client, store.projectId ?? "", number ?? 0), enabled: Boolean(store.projectId && later) })
  const run = later ? data : view.run
  const now = useNow(run !== undefined && leftOf(run) !== undefined)
  const since = run ? (now - run.created_at_ms) / 1000 : undefined
  const whole = useWhole(run, since, run ? leftOf(run) : undefined)
  if (whole === undefined || view.canceling || run?.cancel_requested || run?.status === "failed") return null
  return (
    <span className="tile-meter" role="progressbar" aria-label="The whole job" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(whole * 100)}>
      <i style={{ transform: `scaleX(${whole})` }} />
    </span>
  )
}
