import type { Run, ToolField } from "../api/calcifer"
import type { Row } from "./text"

export interface BeatProgress {
  state: "kept" | "skipped" | "completed" | "in_progress" | "pending"
  completed: number
}

const record = (value: unknown): value is Row => typeof value === "object" && value !== null && !Array.isArray(value)

/** Only a matching frozen request and an actual frame plan can describe these rows. */
export function progressOfBeats(field: ToolField, rows: Row[], run: Run | undefined): BeatProgress[] | null {
  const plan = run?.video_progress
  const input = run?.request?.input
  const frozen = input?.[field.field]
  const timing = field.members?.find((member) => member.total)
  if (!plan?.segments.length || !timing || !Array.isArray(frozen) || frozen.length !== rows.length || !(plan.fps > 0) || !Number.isFinite(plan.fps)) return null
  const durations = frozen.map((row) => {
    const duration = record(row) ? row[timing.field] : undefined
    return typeof duration === "number" ? duration : NaN
  })
  if (durations.some((duration, i) => {
    const current = Number(rows[i][timing.field] ?? timing.default)
    return !Number.isFinite(duration) || duration <= 0 || !Number.isFinite(current) || Math.abs(duration - current) > 1e-6
  })) return null

  // A custom pickup can use a different beat clock. Without its explicit origin,
  // there is no proven alignment between authored beats and absolute shot frames.
  const pickups = Object.values(input ?? {}).filter((value) => record(value) && "take" in value && typeof value.out_frame === "number" && "beats_from" in value)
  if (pickups.length > 1) return null
  const pickup = pickups[0]
  const from = record(pickup) ? pickup.beats_from : undefined
  if ((pickup || plan.resumed_at > 0) && (typeof from !== "number" || !Number.isFinite(from) || from < 0)) return null
  const offset = typeof from === "number" ? plan.resumed_at - Math.round(from * plan.fps) : 0

  let seconds = 0
  const intervals = durations.map((duration) => {
    const start = Math.round(seconds * plan.fps) + offset
    seconds += duration
    return { start, end: Math.round(seconds * plan.fps) + offset }
  })
  if (intervals.some(({ start, end }) => end <= start)) return null
  return intervals.map<BeatProgress>(({ start, end }) => {
    const completed = Math.max(0, Math.min(1, (plan.completed_frames - start) / (end - start)))
    const active = plan.segments.some((segment) => segment.status === "in_progress" && segment.from_frame < end && segment.to_frame > start)
    return { completed, state: end <= plan.resumed_at ? offset === 0 ? "kept" : "skipped" : end <= plan.completed_frames ? "completed" : active ? "in_progress" : "pending" }
  })
}
