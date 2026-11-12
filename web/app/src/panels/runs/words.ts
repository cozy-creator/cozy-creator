import type { Progress, Run, Stalled } from "../../api/calcifer"
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
  const cost = hourly == null ? "" : `, ${usd((hourly * minutes) / 60)} so far at ${usd(hourly)}/h`
  return `No progress for ${span(minutes * 60)} on ${stalled.machine}${cost}`
}

/** The seconds the run has left, as Calcifer estimates them for every client. */
export const secondsLeft = (run: Run): number | undefined => run.progress?.eta_s ?? undefined

/** That nothing like it has run anywhere this computer knows, and it has no step of its own yet, to tell how long it takes. */
const firstRun = (run: Run) => run.progress?.eta_s === undefined && run.progress?.eta_basis === "first_run"

/** How long is left: `41s left`, `about 3m10s left` while that's little trusted; or that it isn't known yet. */
export function eta(run: Run): string | null {
  const progress = run.progress
  if (firstRun(run)) return "time left not known yet: nothing like it has run"
  if (progress?.eta_s == null) return null
  return `${(progress.eta_confidence ?? 1) < 0.5 ? "about " : ""}${span(progress.eta_s)} left`
}

const sentence = (text: string) => text.charAt(0).toUpperCase() + text.slice(1)

/**
 * The stages the Runtime and the packages name (packages and cozy-runtime rc/0.25.0, cl-898's audit), as the page says them, and what a
 * count under each counts: the one place a stage's words are made readable. A stage not here reads as named, and a count under it is only
 * a count: nothing is called a step that isn't one. A count of tokens is a decode's: its total is only its cap.
 */
const STAGES: Record<string, { says: string; counts?: "step" | "frame" | "token" }> = {
  // The Runtime's: `ctx.progress` with no stage, a decode's answer (`vlm.ask`).
  denoise: { says: "Denoising", counts: "step" },
  answering: { says: "Answering", counts: "token" },
  // A picture made: its prompt rewritten or thought through, encoded, its denoising steps, decoded, saved.
  "upsampling prompt": { says: "Rewriting the prompt", counts: "token" },
  thinking: { says: "Thinking", counts: "token" },
  "encoding prompt": { says: "Encoding the prompt" },
  tokenize: { says: "Reading the prompt" },
  "encoding reference images": { says: "Encoding the pictures" },
  "encoding pictures": { says: "Encoding the pictures" },
  "generating image": { says: "Generating the image", counts: "step" },
  generate: { says: "Generating the image", counts: "step" },
  "decoding image": { says: "Decoding the image" },
  // Anima's.
  conditioning: { says: "Encoding the inputs" },
  decoding: { says: "Decoding the image" },
  "saving image": { says: "Saving the image" },
  encode_webp: { says: "Saving the image" },
  // A video made (MiniMax H3): conditioned, denoised and refined by steps, decoded and encoded by frames.
  prepare: { says: "Preparing" },
  condition: { says: "Encoding the inputs" },
  condition_text: { says: "Encoding the prompt" },
  condition_media: { says: "Encoding the pictures" },
  read_video: { says: "Reading the video" },
  upscale_latent: { says: "Upscaling the latents" },
  refine: { says: "Refining", counts: "step" },
  decode_video: { says: "Decoding video", counts: "frame" },
  encode_video: { says: "Encoding video", counts: "frame" },
  decode_audio: { says: "Decoding audio" },
  // A video upscaled (RelayVSR) by its frames; restored (SeedVR2).
  "upscaling frames": { says: "Upscaling frames", counts: "frame" },
  "seedvr2 vae encode": { says: "Encoding the video" },
  "seedvr2 denoise": { says: "Denoising" },
  "seedvr2 vae decode": { says: "Decoding the video" },
}

const stageOf = (part: string) => STAGES[part.trim().toLowerCase()]

/** A stage's path as people read it: each part (a `tel.scope`) after a dot, each named as above: `Turn / Segment 1 of 1 / decode_video` is
 *  `Turn · Segment 1 of 1 · Decoding video`. A count under no stage is `Working`. */
export function stageName(stage: string): string {
  const parts = stage.trim().split(/\s+\/\s+/).filter(Boolean)
  if (!parts.length) return "Working"
  return parts.map((part) => stageOf(part)?.says ?? sentence(part)).join(" · ")
}

/** What a stage's count counts: a decode's tokens as its progress says, else as its stage is known to count; none: just a count. */
export function countsOf(progress: Progress): "step" | "frame" | "token" | undefined {
  if (progress.unit === "tokens") return "token"
  return stageOf(progress.stage.split(/\s+\/\s+/).at(-1) ?? "")?.counts
}

/** How far a stage is, by what it counts: `step 6 of 8`, `frame 173 of 350`, `65 tokens` (a cap is no total), bytes, `173 of 350` for a
 *  count of what isn't known, a share. */
export function countWords(progress: Progress): string {
  const counts = countsOf(progress)
  if (counts === "token") return `${progress.completed} token${progress.completed === 1 ? "" : "s"}`
  if (progress.bytes_total) return `${bytes(progress.bytes_done)} of ${bytes(progress.bytes_total)}`
  if (progress.total) return `${counts ? `${counts} ` : ""}${progress.completed} of ${progress.total}`
  if (progress.stage_fraction != null) return `${Math.round(progress.stage_fraction * 100)}%`
  return ""
}

/** Its own work, as a person reads it: its stage and what it counts, `Generating the image · step 6 of 8, 3s left`,
 *  `Rewriting the prompt · 65 tokens`. The time is the whole run's, Calcifer's estimate. */
export function workWords(run: Run): string {
  const progress = run.progress
  if (!progress?.stage && !progress?.total) return ""
  const left = secondsLeft(run)
  const after = left !== undefined ? `, ${span(left)} left` : firstRun(run) ? ", time left not known yet" : ""
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
