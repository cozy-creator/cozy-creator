import type { Run, Stalled } from "../../types"
import { bytes, leftWords, shown, span, usd } from "../format"

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

/** The seconds left of its stage: its bytes' rate, else its steps' pace. */
export function secondsLeft(run: Run): number | undefined {
  const progress = run.progress
  if (!progress) return undefined
  if (progress.eta_s !== undefined) return progress.eta_s
  const pace = progress.step_avg_ms || progress.step_ms
  const left = (progress.total ?? 0) - (progress.completed ?? 0)
  return pace && left > 0 ? (pace * left) / 1000 : undefined
}

/** How long is left: its bytes' rate, else its steps' pace. */
export function eta(run: Run): string | null {
  const left = secondsLeft(run)
  if (left === undefined) return null
  return run.progress?.eta_s !== undefined ? `${span(left)} left` : `about ${span(left)} left`
}

const sentence = (text: string) => text.charAt(0).toUpperCase() + text.slice(1)

/** Its own work, as a person reads it: `Generating image 3 / 8, 25 s left`. */
export function workWords(run: Run): string {
  const progress = run.progress
  if (!progress?.stage) return ""
  const steps = progress.total ? ` ${progress.completed ?? 0} / ${progress.total}` : ""
  const left = secondsLeft(run)
  return `${sentence(progress.stage)}${steps}${left === undefined ? "" : `, ${leftWords(left)}`}`
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
