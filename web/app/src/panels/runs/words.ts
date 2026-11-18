import type { Progress, Retry, Run, Stalled } from "../../api/calcifer"
import { ordinal, shown, span, usd } from "../format"
import stages from "./stages.json"

export const settled = (run: Run) => run.status === "completed" || run.status === "failed" || run.status === "canceled"

/** What names it anywhere (cl-987): `SE-5`, its project's key and its number there. */
export const runName = (run: Run) => run.name || String(run.number)

/** A job's target says what it does (`install …`, `remove …`, a download's `file → model`), but an update's names only its parts. */
export const target = (run: Run) => (run.kind === "update" ? `update ${shown(run.target)}` : shown(run.target))

/** Sent again, its machine lost or its GPU failed (cl-1173): `attempt 2 of 3: stand-r shut down`. */
export const attemptWords = (retry: Retry) => `attempt ${retry.attempt} of ${retry.attempts}${retry.why ? `: ${retry.why}` : ""}`

/** `Canceling` once a cancel is asked; a waiting run's place in its line. */
export function statusWord(run: Run): string {
  if (run.queue_position != null) return `Queued, ${ordinal(run.queue_position)} in line`
  if (run.cancel_requested && !settled(run)) return "Canceling"
  const word = run.status === "in_progress" ? "running" : run.status
  return word.charAt(0).toUpperCase() + word.slice(1)
}

export const statusClass = (run: Run) =>
  run.status === "completed" ? "s-current" : run.status === "failed" ? "s-bad" : run.status === "canceled" ? "s-missing" : run.status === "in_progress" ? "s-making" : "s-queued"

/** How long it stopped, where, and what that has cost: by the minute. */
export function stalledWords(stalled: Stalled, now: number): string {
  const minutes = Math.floor(Math.max(0, now - stalled.since_ms) / 60_000)
  const hourly = stalled.usd_micros_per_hour
  const cost = hourly == null ? "" : ` (${usd((hourly * minutes) / 60)} so far at ${usd(hourly)}/h)`
  return `No progress for ${span(minutes * 60)} on ${stalled.machine}${cost}`
}

/** The seconds the run has left, as Calcifer estimates them for every client. */
export const secondsLeft = (run: Run): number | undefined => run.progress?.eta_s ?? undefined

/** That nothing like it has run anywhere this computer knows, and it has no step of its own yet, to tell how long it takes. */
const firstRun = (run: Run) => run.progress?.eta_s === undefined && run.progress?.eta_basis === "first_run"

/** How long is left: `41s left`, `about 3m10s left` while that's little trusted; or that it isn't known yet. */
export function eta(run: Run): string | null {
  const progress = run.progress
  if (firstRun(run)) return "time left unknown: this is its first run"
  if (progress?.eta_s == null) return null
  // Only the bytes it moves are known: at least that.
  if (progress.eta_basis === "bytes") return `at least ${span(progress.eta_s)} left`
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

/** A `match` as a pattern: literal text, `{name}` any text, `{#name}` digits; any case, the whole text, or with `start` its start. */
function matcher(match: string, start = false) {
  const names: string[] = []
  const source = match.replace(/\{(#?)(\w+)\}|[.*+?^$()|[\]\\]/g, (piece, digits: string | undefined, name: string | undefined) => {
    if (name === undefined) return `\\${piece}`
    names.push(name)
    return digits ? "(\\d+)" : "(.+?)"
  })
  return { names, pattern: new RegExp(`^${source}${start ? "" : "$"}`, "i") }
}

/** `says` with what a `matcher` found filled in. */
const filled = (says: string, names: string[], found: RegExpExecArray) =>
  says.replace(/\{(\w+)\}/g, (said, name: string) => {
    const at = names.indexOf(name)
    return at < 0 ? said : found[at + 1]
  })

const compiled = (stages.stages as unknown as Entry[]).map((entry) => ({ entry, ...matcher(entry.match) }))

/** One of `stages.json`'s `waits`: a reason TensorD or Calcifer gives for a wait, by its start, and what a box's line says of it, which
 *  holds about 25 characters beside Cancel (cl-1197: `Waiting for another project's run to finish with this machine's CPU` was
 *  `Waiting for another…`). */
interface Wait {
  match: string
  box: string
}
const boxed = (stages.waits as Wait[]).map((entry) => ({ entry, ...matcher(entry.match, true) }))

/** A part of a stage's path, as the map says it: its words with what it named filled in, its phase and what it counts; none: the map
 *  doesn't know it. The release gate reads the same file (`release-gate/suites/progress`). */
export function stageOf(part: string): { says: string; phase?: string; counts?: Counts; scope?: boolean } | undefined {
  const text = part.trim()
  for (const { entry, names, pattern } of compiled) {
    const found = pattern.exec(text)
    if (!found) continue
    return { says: filled(entry.says, names, found), phase: entry.phase, counts: entry.counts, scope: entry.scope }
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

/** What a stage's bytes are (Paul, 11-16, cl-1074): weights arriving here (`download`), or loaded into the GPU (`load`); none when it moves
 *  none, or they're a package's or a result's. Bytes under a stage that doesn't say so (`preparing model adapters…`) are weights arriving. */
export function bytesKind(progress: Progress): "download" | "load" | undefined {
  if (!progress.bytes_total && !progress.bytes_done) return undefined
  const phase = phaseOf(progress.stage)
  if (phase === "loading") return "load"
  return phase && ["installing", "uploading", "saving", "result"].includes(phase) ? undefined : "download"
}

/** A stage's words, saying what its bytes are where they don't: `Downloading weights · Preparing the adapters for lora`, `Loading
 *  transformer into the GPU`. */
function saysBytes(progress: Progress, words: string): string {
  const kind = bytesKind(progress)
  if (kind === "load") return `${words} into the GPU`
  return kind === "download" && phaseOf(progress.stage) !== "downloading" ? `Downloading weights · ${words}` : words
}

/** A stage's name, and what its bytes are. */
export const stageWords = (progress: Progress) => saysBytes(progress, stageName(progress.stage))

/** A rate, in bytes a second (Paul, 11-16: never `MBps`, which reads as megabits): `117MB/s`, `1.2GB/s`. */
export const rateWords = (rate: number) => `${size(rate)}/s`

/** Bytes as Paul writes them: `2.1GB`, `340MB`. */
export function size(n: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"]
  let value = n
  let unit = 0
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000
    unit += 1
  }
  return `${unit === 0 ? value : value < 10 ? value.toFixed(1) : value.toFixed(0)}${units[unit]}`
}

/** How far a stage is, by what it counts (cl-1084): `step 7 of 8`, `frame 173 of 350`, a decode's `65 tokens` (a cap is no total),
 *  `2.1GB of 7GB`, `173 of 350` for a count of anything else (`stages.json`'s items), a share. */
export function countWords(progress: Progress): string {
  const counts = countsOf(progress)
  if (counts === "token") return `${progress.completed} token${progress.completed === 1 ? "" : "s"}`
  if (progress.bytes_total) return `${size(progress.bytes_done)} of ${size(progress.bytes_total)}`
  if (progress.total) return `${counts === "step" ? "step " : counts === "frame" ? "frame " : ""}${progress.completed} of ${progress.total}`
  if (progress.stage_fraction != null) return `${Math.round(progress.stage_fraction * 100)}%`
  return ""
}

/** A stage with its count (cl-1084): `Rendering, step 7 of 8`, `Rewriting the prompt, 65 tokens`, `Segment 2 of 3: Decoding the video,
 *  frame 173 of 350`. */
export function stageLine(progress: Progress): string {
  const { words, count } = stageParts(progress)
  return count ? `${words} ${count}` : words
}

/** `stageLine`'s two halves: its words, `Segment 2 of 2: Decoding the video,`, which give way to a narrow box, and how far within it,
 *  `frame 3 of 12`, which never does (cl-953). A scope of one (`Segment 1 of 1`) says nothing. */
export function stageParts(progress: Progress): { words: string; count?: string } {
  const parts = progress.stage.trim().split(/\s+\/\s+/).filter(Boolean)
  const own = parts.length ? stageName(parts.at(-1)!) : "Working"
  const scope = parts.slice(0, -1).filter((part) => !/\b1 of 1$/i.test(part.trim())).map((part) => stageName(part)).join(", ")
  const words = saysBytes(progress, scope ? `${scope}: ${own}` : own)
  const count = countWords(progress)
  if (!count) return { words }
  return { words: `${words},`, count }
}

/** Why a run waits, as Calcifer says it (`waiting for 2 GPUs, held by run PRO6-60, about 2 min`: cl-1187), less the machine state its
 *  reason repeats (`queued: `), each run `boxOf` knows by its bare number named by its box (`<hero>`). The one wording for the drawer, the
 *  run list, the canvas face and the machine tabs (cl-1197); none while it waits for nothing. */
export function waitReason(waiting: string | null | undefined, boxOf?: (run: number) => string | undefined): string {
  const said = waiting?.replace(/^(queued|preparing|running): /, "") ?? ""
  return boxOf ? said.replace(/\brun (\d+)/g, (whole, number: string) => { const box = boxOf(Number(number)); return box ? `<${box}>` : whole }) : said
}

/** `waitReason` as a line. */
export const waitWords = (waiting: string | null | undefined, boxOf?: (run: number) => string | undefined) => sentence(waitReason(waiting, boxOf))

/** What it waits for alone, a box's line, which has room for no more: `Waiting for 2 GPUs`, `Waiting for the CPU`, as `stages.json`'s
 *  `waits` say its reason, else its reason up to its first comma (as cl-1205's `Waiting for download`: a cut-off line lost its key word).
 *  Who holds it and about how long, its machine's tab, its queue, its drawer and the run list say whole (`waitWords`). */
export function waitFor(waiting: string | null | undefined): string {
  const reason = waitReason(waiting).trim()
  for (const { entry, names, pattern } of boxed) {
    const found = pattern.exec(reason)
    if (found) return filled(entry.box, names, found)
  }
  return sentence(reason.split(", ")[0])
}

/** It waits on downloads its progress names (cl-1205's typed `waiting_on`): its box and tile say `Waiting for download`; its rows, its
 *  drawer and the run list its stage, `Waiting for Sdxl to download`, and never its machine's reason over it, whose bytes are the
 *  download's to show (Paul: the jobs waiting "don't need to independently report the progress"), so a download wait reads one way. */
export const downloadWait = (run: Run) => Boolean(run.progress?.waiting_on?.length)

/** What a waiting run waits for, whole, as a row says it: a download wait its stage (`downloadWait`), any other its reason
 *  (`waitWords`). None while it waits for nothing. */
export function waitLine(run: Run): string {
  if (!downloadWait(run)) return waitWords(run.waiting)
  const own = run.progress?.stage.split(/\s+\/\s+/).at(-1)?.trim()
  return own ? stageName(own) : "Waiting for download"
}

/** What it's doing, as a person reads it: what it waits for while it waits (cl-1197; Paul read a 96 s wait for GPUs, shown as its stage
 *  "Preparing, time left unknown", as stuck), else its stage and what it counts, `Rendering, step 6 of 8, 3s left`, `Rewriting the
 *  prompt, 65 tokens`. The time is the whole run's, Calcifer's estimate. */
export function workWords(run: Run): string {
  const wait = downloadWait(run) ? "" : waitWords(run.waiting)
  if (wait) return wait
  const progress = run.progress
  if (!progress?.stage && !progress?.total) return ""
  const left = secondsLeft(run)
  const after = left !== undefined ? `, ${span(left)} left` : firstRun(run) ? ", time left unknown" : ""
  return `${stageLine(progress)}${after}`
}

/** Worth saying more than a bar: it has run over 10 s, or has over 10 s to go. */
export function long(run: Run, now: number): boolean {
  const began = run.started_at_ms ?? run.accepted_at_ms ?? run.created_at_ms
  return now - began > 10_000 || (secondsLeft(run) ?? 0) > 10
}

/** Its stage, how far, and how fast bytes move; what it waits on says first, a download wait by its stage alone (`downloadWait`). */
export function progressWords(run: Run): string {
  const progress = run.progress
  const wait = downloadWait(run) ? "" : waitWords(run.waiting)
  if (!progress) return wait || sentence(run.machine_state ?? "")
  const parts = [stageWords(progress)]
  parts.push(countWords(progress))
  if (progress.bytes_per_s) parts.push(rateWords(progress.bytes_per_s))
  const left = eta(run)
  if (left) parts.push(left)
  const line = parts.filter(Boolean).join(" · ")
  return wait ? `${wait} · ${line}` : line
}

/** Its own time, never its queue's. */
export function runTime(run: Run, now: number): string {
  if (run.execution_ms != null) return span(run.execution_ms / 1000)
  if (run.started_at_ms != null && !settled(run)) return span((now - run.started_at_ms) / 1000)
  return ""
}
