import type { Run, Stalled } from "../../types"
import { bytes, shown, span, usd } from "../format"

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
  const cost = hourly === undefined ? "" : `, ${usd((hourly * minutes) / 60)} so far at ${usd(hourly)}/h`
  return `No progress for ${span(minutes * 60)} on ${stalled.machine}${cost}`
}

/** The seconds the run has left, as Calcifer estimates them for every client. */
export const secondsLeft = (run: Run): number | undefined => run.progress?.eta_s

/** That nothing like it ran on its machine to tell how long it takes. */
const firstRun = (run: Run) => run.progress?.eta_s === undefined && run.progress?.eta_basis === "first_run"

/** How long is left: `41s left`, `about 3m10s left` while that's little trusted; or that this is its first run here. */
export function eta(run: Run): string | null {
  const progress = run.progress
  if (firstRun(run)) return "first run on this machine"
  if (progress?.eta_s === undefined) return null
  return `${(progress.eta_confidence ?? 1) < 0.5 ? "about " : ""}${span(progress.eta_s)} left`
}

const sentence = (text: string) => text.charAt(0).toUpperCase() + text.slice(1)

/** Its own work, as a person reads it, by what its progress carries: steps are `Denoise steps: 6/8, 3s left`, bytes `Downloading`, and any
 *  other stage its own words, short: `Decoding`, `Encoding prompt`. The time is the whole run's, Calcifer's estimate. */
export function workWords(run: Run): string {
  const progress = run.progress
  if (!progress?.stage) return ""
  const left = secondsLeft(run)
  const after = left !== undefined ? `, ${span(left)} left` : firstRun(run) ? ", first run on this machine" : ""
  if (progress.bytes_total) return `Downloading${after}`
  if (progress.total) return `Denoise steps: ${progress.completed ?? 0}/${progress.total}${after}`
  return `${phase(progress.stage)}${after}`
}

/** A stage in a word or two: what it's done to, a picture or a video, goes. `decoding image` is `Decoding`. */
function phase(stage: string): string {
  const [verb, ...rest] = stage.trim().split(/\s+/)
  return sentence(rest.length === 1 && /^(image|images|video|videos|audio|picture|pictures)$/i.test(rest[0]) ? verb : stage.trim())
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
  const parts = [progress.stage]
  if (progress.total) parts.push(`${progress.completed ?? 0}/${progress.total}`)
  if (progress.bytes_per_s) parts.push(`${bytes(progress.bytes_per_s)}/s`)
  const left = eta(run)
  if (left) parts.push(left)
  const line = parts.filter(Boolean).join(" · ")
  return run.waiting ? `${run.waiting} · ${line}` : line
}

/** Its own time, never its queue's. */
export function runTime(run: Run, now: number): string {
  if (run.execution_ms !== undefined) return span(run.execution_ms / 1000)
  if (run.started_at_ms !== undefined && !settled(run)) return span((now - run.started_at_ms) / 1000)
  return ""
}
