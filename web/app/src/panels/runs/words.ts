import type { Progress, Run, Stalled } from "../../api/calcifer"
import { bytes, shown, span, usd } from "../format"
import stages from "./stages.json"

export const settled = (run: Run) => run.status === "completed" || run.status === "failed" || run.status === "canceled"

export const target = (run: Run) => (run.kind === "call" ? shown(run.target) : `${run.kind} ${shown(run.target)}`)

/** `canceling` once a cancel is asked; a waiting run's place in its line. */
export function statusWord(run: Run): string {
  if (run.queue_position !== undefined) return `queue: ${run.queue_position}`
  if (run.cancel_requested && !settled(run)) return "canceling"
  return run.status === "in_progress" ? "running" : run.status
}

export const statusClass = (run: Run) =>
  run.status === "completed" ? "s-current" : run.status === "failed" ? "s-bad" : run.status === "canceled" ? "s-missing" : run.status === "in_progress" ? "s-making" : "s-queued"

/** How long it stopped, where, and what that has cost: by the minute. */
export function stalledWords(stalled: Stalled, now: number): string {
  const minutes = Math.floor(Math.max(0, now - stalled.since_ms) / 60_000)
  const hourly = stalled.usd_micros_per_hour
  const cost = hourly == null ? "" : `, ${usd((hourly * minutes) / 60)} so far at ${usd(hourly)}/h`
  return `No progress for ${span(minutes * 60)} on ${stalled.machine}${cost}`
}

/** The seconds the run has left, as Calcifer estimates them for every client. */
export const secondsLeft = (run: Run): number | undefined => run.progress?.eta_s ?? undefined

/** That nothing like it ran on its machine to tell how long it takes. */
const firstRun = (run: Run) => run.progress?.eta_s === undefined && run.progress?.eta_basis === "first_run"

/** How long is left: `41s left`, `about 3m10s left` while that's little trusted; or that this is its first run here. */
export function eta(run: Run): string | null {
  const progress = run.progress
  if (firstRun(run)) return "first run on this machine"
  if (progress?.eta_s == null) return null
  return `${(progress.eta_confidence ?? 1) < 0.5 ? "about " : ""}${span(progress.eta_s)} left`
}

const sentence = (text: string) => text.charAt(0).toUpperCase() + text.slice(1)

/** What a count counts, as `stages.json` names it: steps only where a stage denoises. */
export type Counts = "step" | "frame" | "token" | "bytes" | "item"

/** One entry of `stages.json`: a stage (or a scope of one) as the Runtime, a package, TensorD or Calcifer names it, how the page says it,
 *  the phase of the job it is, and what a count under it counts. */
interface Entry {
  match: string
  says: string
  phase?: string
  counts?: Counts
  scope?: boolean
}

/** A `match` as a pattern: literal text, `{name}` any text, `{#name}` digits; the whole part, any case. */
const compiled = (stages.stages as unknown as Entry[]).map((entry) => {
  const names: string[] = []
  const source = entry.match.replace(/\{(#?)(\w+)\}|[.*+?^$()|[\]\\]/g, (piece, digits: string | undefined, name: string | undefined) => {
    if (name === undefined) return `\\${piece}`
    names.push(name)
    return digits ? "(\\d+)" : "(.+?)"
  })
  return { entry, names, pattern: new RegExp(`^${source}$`, "i") }
})

/** A part of a stage's path, as the map says it: its words with what it named filled in, its phase and what it counts; none: the map
 *  doesn't know it. The release gate reads the same file (`release-gate/suites/progress`). */
export function stageOf(part: string): { says: string; phase?: string; counts?: Counts; scope?: boolean } | undefined {
  const text = part.trim()
  for (const { entry, names, pattern } of compiled) {
    const found = pattern.exec(text)
    if (!found) continue
    const filled = entry.says.replace(/\{(\w+)\}/g, (said, name: string) => {
      const at = names.indexOf(name)
      return at < 0 ? said : found[at + 1]
    })
    return { says: filled, phase: entry.phase, counts: entry.counts, scope: entry.scope }
  }
  return undefined
}

/** A stage's path as people read it: each part (a `tel.scope`) after a dot, each named as `stages.json` says: `Turn / Segment 1 of 1 /
 *  decode_video` is `Turn · Segment 1 of 1 · Decoding video`. A part the map doesn't know reads as named. A count under no stage is
 *  `Working`. */
export function stageName(stage: string): string {
  const parts = stage.trim().split(/\s+\/\s+/).filter(Boolean)
  if (!parts.length) return "Working"
  return parts.map((part) => stageOf(part)?.says ?? sentence(part.trim())).join(" · ")
}

/** The phase of the job a stage is (`installing`, `downloading`, `loading`, `working`, …), by its path's last part; none: unknown. */
export const phaseOf = (stage: string): string | undefined => stageOf(stage.split(/\s+\/\s+/).at(-1) ?? "")?.phase

/** What a stage's count counts: a decode's tokens or bytes as its progress says, else as its stage is known to count; none: just a count. */
export function countsOf(progress: Progress): Counts | undefined {
  if (progress.unit === "tokens") return "token"
  if (progress.unit === "bytes") return "bytes"
  return stageOf(progress.stage.split(/\s+\/\s+/).at(-1) ?? "")?.counts
}

/** How far a stage is, by what it counts: `step 6 of 8`, `frame 173 of 350`, `65 tokens` (a cap is no total), bytes, `173 of 350` for a
 *  count of what isn't named, a share. */
export function countWords(progress: Progress): string {
  const counts = countsOf(progress)
  if (counts === "token") return `${progress.completed} token${progress.completed === 1 ? "" : "s"}`
  if (progress.bytes_total) return `${bytes(progress.bytes_done)} of ${bytes(progress.bytes_total)}`
  if (progress.total) return `${counts === "step" || counts === "frame" ? `${counts} ` : ""}${progress.completed} of ${progress.total}`
  if (progress.stage_fraction != null) return `${Math.round(progress.stage_fraction * 100)}%`
  return ""
}

/** Its own work, as a person reads it: its stage and what it counts, `Generating the image · step 6 of 8, 3s left`,
 *  `Rewriting the prompt · 65 tokens`. The time is the whole run's, Calcifer's estimate. */
export function workWords(run: Run): string {
  const progress = run.progress
  if (!progress?.stage && !progress?.total) return ""
  const left = secondsLeft(run)
  const after = left !== undefined ? `, ${span(left)} left` : firstRun(run) ? ", first run on this machine" : ""
  const count = countWords(progress)
  return `${stageName(progress.stage)}${count ? ` · ${count}` : ""}${after}`
}

/** Worth saying more than a bar: it has run over 10 s, or has over 10 s to go. */
export function long(run: Run, now: number): boolean {
  const began = run.started_at_ms ?? run.accepted_at_ms ?? run.created_at_ms
  return now - began > 10_000 || (secondsLeft(run) ?? 0) > 10
}

/** Its stage, how far, and how fast bytes move; what it waits on says first. */
export function progressWords(run: Run): string {
  const progress = run.progress
  if (!progress) return run.waiting ?? run.machine_state ?? ""
  const parts = [stageName(progress.stage)]
  parts.push(countWords(progress))
  if (progress.bytes_per_s) parts.push(`${bytes(progress.bytes_per_s)}/s`)
  const left = eta(run)
  if (left) parts.push(left)
  const line = parts.filter(Boolean).join(" · ")
  return run.waiting ? `${run.waiting} · ${line}` : line
}

/** Its own time, never its queue's. */
export function runTime(run: Run, now: number): string {
  if (run.execution_ms != null) return span(run.execution_ms / 1000)
  if (run.started_at_ms != null && !settled(run)) return span((now - run.started_at_ms) / 1000)
  return ""
}
