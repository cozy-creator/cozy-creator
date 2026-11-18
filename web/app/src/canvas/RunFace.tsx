import { useQuery } from "@tanstack/react-query"
import { useEffect, useRef, useState } from "react"
import { Link } from "react-router-dom"
import type { Ahead, Machine, Operation, Run } from "../api/calcifer"
import { machinesQuery, runQuery } from "../api/queries"
import { useConnection } from "../app/Connection"
import { useTo } from "../app/project"
import { useNow } from "../panels/common"
import { anchorOf, packageWords, useDownloadsOf, useInstalling, useWaitsOn } from "../panels/machines/activity"
import { ordinal } from "../panels/format"
import { attemptWords, bytesKind, rateWords, size, stageParts, waitFor, waitReason, waitWords } from "../panels/runs/words"
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
  /** How far within it and how fast (`frame 3 of 12`, `2GB of 7GB, 100MB/s`): never cut (cl-953). */
  count?: string
  /** The download it waits on, on its machine, which its words lead to: that alone shows how far it is (cl-1205). */
  leads?: { machine: string; download: string }
}

/** Where a box's "Waiting for … to download" leads: its machine's drawer, at that download's row. */
export const leadsTo = (leads: NonNullable<Face["leads"]>) => `machines/${encodeURIComponent(leads.machine)}#${anchorOf(leads.download)}`

/** A span as Paul writes it: `3s`, `2m 5s`, `5m`, `1h 4m`. */
export function short(s: number): string {
  const all = Math.max(0, Math.round(s))
  if (all < 60) return `${all}s`
  if (all < 3600) return all % 60 ? `${Math.floor(all / 60)}m ${all % 60}s` : `${all / 60}m`
  const m = Math.floor((all % 3600) / 60)
  return m ? `${Math.floor(all / 3600)}h ${m}m` : `${Math.floor(all / 3600)}h`
}

const sentence = (said: string) => said.charAt(0).toUpperCase() + said.slice(1)

/** Bytes moving: `2.1GB of 7GB, 100MB/s`. */
const moving = (done: number, total: number, rate?: number | null) => `${size(done)}${total ? ` of ${size(total)}` : ""}${rate ? `, ${rateWords(rate)}` : ""}`

/** The whole job's time left, Calcifer's: once it ended, its result coming here. */
const leftOf = (run: Run) => (run.status === "completed" ? run.arriving?.eta_s : run.progress?.eta_s) ?? undefined

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
  /** The downloads its run waits on there, each its machine's one (cl-1205). */
  downloads?: Operation[]
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
  /** Which face this is, when it isn't its box's own (a take's run, in Focus): each is steadied by itself. */
  slot?: string
}

/** How long a box's face waits before it says a new phase: one that comes and goes faster, weights already here, an environment already
 *  installed, a GPU free at once, never flashes by (cl-1197; Paul, 11-18: "those are just phases it's skipping"). 400 ms, as a spinner is
 *  held back: React reveals a fallback no sooner than 300 ms after the last (FALLBACK_THROTTLE_MS), and NN/g puts under a second as one
 *  moment, no indicator owed. A real download or install still shows, with its progress, within half a second. */
export const STEADY_MS = 400
const ENDS = ["canceling", "failed", "result"]
/** What each face shows now, by its box's name (`Seen.slot`): the run it's of, and what would replace it once it has lasted. */
const steady = new Map<string, { face: Face; key: string; run?: number; next?: { key: string; since: number } }>()

/** Its two lines' words, by what is known of it: the most specific first. Past its first attempt, line 1 says which and why (cl-1173). A
 *  new phase shows once it has lasted STEADY_MS; its own count, and an end, show at once, and so does what this page asked itself
 *  (queued, no run yet). The first this page sees of a run under way (it just opened, or the run came from elsewhere) has a phase
 *  whose age is its machine's to say: one it last saw move STEADY_MS ago or more shows at once, and any other waits as a new phase
 *  does, its machine and Cancel shown meanwhile, so a phase gone in a blink is never said first and left standing (e2e 44244383401
 *  read "Blinking once", 200 ms long, for 900 ms). */
export function faceOf(seen: Seen, now = Date.now()): Face {
  const said = faceNow(seen)
  const retry = seen.run?.retry
  const face = retry && !ENDS.includes(said.phase) ? { ...said, machine: `${said.machine} · ${attemptWords(retry)}` } : said
  const slot = seen.slot ?? seen.view.name
  const run = seen.run?.number
  // Its words and where, not its count: a count moving is the same phase going on.
  const key = `${face.phase}\0${face.state}\0${face.machine}`
  const was = steady.get(slot)
  const show = () => {
    steady.set(slot, { face, key, run })
    return face
  }
  // Nothing of this render is shown yet: none here, the one before it ended, or what's here is another run's.
  const fresh = !was || ENDS.includes(was.face.phase) || (was.run !== undefined && was.run !== run)
  if (ENDS.includes(face.phase) || (!fresh && was.key === key)) return show()
  const moved = seen.run?.progress?.at_ms
  if (run !== undefined && (fresh || was.run === undefined) && moved && now - moved >= STEADY_MS) return show()
  if (fresh) {
    if (run === undefined) return show()
    const held: Face = { phase: "queued", machine: face.machine, state: "" }
    steady.set(slot, { face: held, key: "", run, next: { key, since: now } })
    return held
  }
  was.run = run
  if (was.next?.key !== key) was.next = { key, since: now }
  if (now - was.next.since < STEADY_MS) return was.face
  return show()
}

/** When a face will say the phase it's holding back, if one: the moment to draw it again. */
export function faceSettles(slot: string): number | null {
  const next = steady.get(slot)?.next
  return next ? next.since + STEADY_MS : null
}

function faceNow(seen: Seen): Face {
  const { view, run, machine, on, installing, waits, ahead, preparing, downloads } = seen
  const rented = machine?.kind === "rental" && machine.state === "booting"
  const card = machine?.rental?.accelerator || machine?.rental?.sku?.toUpperCase()
  const where = run?.machine ? (rented && card ? `${run.machine} (renting ${card})` : run.machine) : "Choosing a machine"
  const face = (phase: string, state: string, on = where, count?: string): Face => ({ phase, machine: on, state, ...(count ? { count } : {}) })
  if (seen.canceling || run?.cancel_requested) return face("canceling", "Canceling…")
  if (run?.status === "failed") return face("failed", `Failed: ${run.reason?.message ?? "the run failed"}`)
  if (waits && !on) return prepared(`Waiting for <${waits}>`, preparing)
  if (!run) {
    // None of its machines has room on its disk for its models (cl-1175): it waits, nothing of them downloaded.
    if (view.noRoom) return face("queued", "Needs", "Waiting for disk space", `${view.noRoom.short} more`)
    // Its machines have no GPU, and nothing measured says it runs without one (cl-1279): it waits for a machine that can run it.
    if (view.noGpu) return face("queued", "Needs", "Waiting for a machine", "a GPU")
    if (installing) return face("install", `Installing ${packageWords(installing.subject)}`, "Choosing a machine")
    if (view.queued && (preparing?.stage || preparing?.ready)) return prepared(`Queued, ${ordinal((ahead?.length ?? 0) + 1)} in line`, preparing)
    if (view.queued) return face("queued", "Queued,", "Choosing a machine", `${ordinal((ahead?.length ?? 0) + 1)} in line`)
    return face("queued", "Queued", "Choosing a machine")
  }
  const waiting = waitReason(run.waiting, seen.boxOf)
  if (!run.machine) {
    const renting = waiting?.match(/^renting (.+)$/)
    return renting ? face("renting", "Placing the order", `Renting ${renting[1]}`) : run.queue_position != null ? face("queued", "Queued,", where, `${ordinal(run.queue_position)} in line`) : face("queued", "Queued")
  }
  if (machine?.state === "booting") return face("renting", machine.boot ? sentence(machine.boot) : "Starting up")
  if (waiting?.startsWith("uploading ")) return face("uploading", `Uploading ${waiting.slice("uploading ".length)}`)
  if (on?.kind === "install") return face("install", `Installing ${packageWords(on.subject)}${on.stage ? `: ${on.stage}` : ""}`)
  // Waiting on downloads it says so, with no bytes of its own: each is its machine's, shown once there, where its words lead (cl-1205;
  // Paul: "the 3 jobs that are like 'waiting for download...'"). Which models, its machine's rows say: a box's line has no room for them.
  if (on?.kind === "download") return { ...face("download", "Waiting for download"), leads: { machine: run.machine, download: (downloads?.[0] ?? on).id } }
  if (run.queue_position != null) return face("queued", "Queued,", where, `${ordinal(run.queue_position)} in line`)
  if (run.status === "completed") {
    const arriving = run.arriving
    if (arriving?.bytes_total) return face("result", "Downloading the result,", where, moving(arriving.bytes_done, arriving.bytes_total, arriving.bytes_per_s))
    return face("result", owed(run) ? "Downloading the result" : "Saving")
  }
  const progress = run.progress
  if (progress?.waiting_on?.length) return { ...face("download", "Waiting for download"), leads: { machine: run.machine, download: progress.waiting_on[0] } }
  // Waiting on anything else, it says what for, not the stage it waits in (cl-1197: "Preparing, time left unknown" read as stuck
  // through a 96 s wait for 2 GPUs): `Waiting for 2 GPUs`, whole on a box's line. Who holds them and about how long, its machine's rows
  // and its drawer say.
  if (waiting) return face("queued", waitFor(run.waiting))
  if (progress?.stage || progress?.total) {
    const { words, count } = stageParts(progress)
    const phase = bytesKind(progress) === "download" ? "download" : /install/i.test(progress.stage) ? "install" : "work"
    const numbers = [count, progress.bytes_per_s ? rateWords(progress.bytes_per_s) : ""].filter(Boolean).join(", ")
    return face(phase, count || !numbers ? words : `${words},`, where, numbers || undefined)
  }
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
  if (row.held_back) return { phase: "queued", machine: row.machine, state: `${state}. Not prepared yet: ${row.held_back}` }
  if (row.ready) return { phase: "queued", machine: row.machine, state: `${state}. Ready to start` }
  if (!row.stage) return { phase: "queued", machine: "Choosing a machine", state }
  const counted = row.total > 0
  const phase = counted || /download/i.test(row.stage) ? "download" : /install/i.test(row.stage) ? "install" : "queued"
  // Its bytes are its count, which never gives way; with what it waits on after them, all of it is words, as it reads.
  if (counted && !row.waiting) return { phase, machine: row.machine, state: `${state}. Getting ready: ${row.stage},`, count: moving(row.done, row.total) }
  const ready = `Getting ready: ${row.stage}${counted ? `, ${moving(row.done, row.total)}` : ""}`
  return { phase, machine: row.machine, state: [state, ready, waitWords(row.waiting)].filter(Boolean).join(". ") }
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
  const downloads = useDownloadsOf(run)
  const to = useTo()
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
  const slot = later ? `${name}\0${number}` : name
  const face = faceOf({ view, run, machine, on, downloads, installing, waits: waits && !on ? waits : undefined, ahead: store.aheadOf(name), preparing: store.preparing(name), canceling: Boolean(view.canceling), boxOf: (n) => store.boxOfRun(n), slot })
  // A phase held back is drawn once it has lasted, though nothing else changes by then.
  const settles = faceSettles(slot)
  const [, wake] = useState(0)
  useEffect(() => {
    if (settles === null) return
    const timer = setTimeout(() => wake((n) => n + 1), Math.max(0, settles - Date.now()) + 1)
    return () => clearTimeout(timer)
  }, [settles])
  const since = run ? (now - run.created_at_ms) / 1000 : undefined
  const left = run ? leftOf(run) : undefined
  const time = since !== undefined && left != null ? ` · ${short(since)} of ${short(since + left)}` : ""
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
          {face.leads ? (
            <Link className="face-words face-leads" to={to(leadsTo(face.leads))} title="See its download on its machine" onClick={(event) => event.stopPropagation()}>
              {face.state}
            </Link>
          ) : (
            <span className="face-words">{face.count ? `${face.state} ` : face.state}</span>
          )}
          {face.count && <span className="face-count">{face.count}</span>}
        </b>
        {cancel && (
          <button
            type="button"
            className="face-cancel canvas-control"
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
          <span className="face-words">{made ? `${sentence(view.asset?.noun ?? "take")} ${made.number} finished${ms ? ` in ${short(ms / 1000)}` : ""}` : ""}</span>
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
    <span className="tile-meter" role="progressbar" aria-label="Overall progress" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(whole * 100)}>
      <i style={{ transform: `scaleX(${whole})` }} />
    </span>
  )
}
