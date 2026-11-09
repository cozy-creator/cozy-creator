// An Edit as the box holds it: clips end to end on V1, every point a whole frame at the edit's rate. No gaps and no overlaps
// can be written, since a clip has no place of its own: where it sits is the sum of what's before it.
import type { Clip, ClipNow } from "../../api/calcifer"
import type { CanvasStore } from "../CanvasStore"
import { shownOf } from "../shown"

/** How long a still lasts until a handle says otherwise, as Calcifer reads one with no `frames`. */
export const STILL_S = 3

export const mention = (name: string) => `<${name}>`
export const named = (clip: Clip) => clip.clip.replace(/^<|>$/g, "")

/** A clip, where it sits and what it shows. Offline, it has no frames, as a render goes on without it. */
export interface Placed {
  clip: ClipNow
  index: number
  at: number
  frames: number
  /** Its source's frames, counting from 0, `out` exclusive, as OTIO and an EDL count. A still's is 0 to its duration. */
  in: number
  out: number
  /** Left to right, in the box's own pixels. */
  x: number
  w: number
}

export interface Layout {
  placed: Placed[]
  frames: number
  width: number
}

/** A clip too short to see still has handles to pull, and an offline one still says so. */
export const NARROWEST = 22
export const OFFLINE = 112

export const isStill = (clip: ClipNow) => clip.field === "image"

export function span(clip: ClipNow, fps: number): { in: number; out: number } {
  if (clip.missing) return { in: 0, out: 0 }
  if (isStill(clip)) return { in: 0, out: Math.max(1, clip.frames ?? Math.round(STILL_S * fps)) }
  const holds = clip.holds ?? Number.POSITIVE_INFINITY
  const first = Math.min(clip.in ?? 0, holds - 1)
  const whole = clip.holds ?? first + Math.round(STILL_S * fps)
  return { in: first, out: Math.max(first + 1, Math.min(clip.out ?? whole, holds)) }
}

export function lay(clips: ClipNow[], fps: number, scale: number): Layout {
  let at = 0
  let x = 0
  const placed = clips.map((clip, index): Placed => {
    const held = span(clip, fps)
    const frames = held.out - held.in
    const w = clip.missing ? OFFLINE : Math.max(NARROWEST, frames * scale)
    const place = { clip, index, at, frames, ...held, x, w }
    at += frames
    x += w
    return place
  })
  return { placed, frames: at, width: x }
}

/** The same cut at another rate: a clip's points are times, so their frames move with it. What no clip says stays unsaid. */
export function retimed(timeline: { tracks: { clips: Clip[] }[] }, from: number, to: number) {
  const at = (frame?: number | null) => (frame == null ? undefined : Math.round((frame * to) / from))
  return { ...timeline, tracks: timeline.tracks.map((track) => ({ ...track, clips: track.clips.map((clip) => ({ ...clip, in: at(clip.in), out: at(clip.out), frames: at(clip.frames) })) })) }
}

/** The clip showing frame `frame`, the last for one past the end. */
export function clipAt(layout: Layout, frame: number): Placed | undefined {
  const showing = layout.placed.filter((place) => place.frames > 0)
  return showing.find((place) => frame < place.at + place.frames) ?? showing.at(-1)
}

export const xOf = (layout: Layout, frame: number): number => {
  const place = clipAt(layout, frame)
  if (!place) return 0
  return place.x + (Math.min(frame - place.at, place.frames) / place.frames) * place.w
}

export function frameAt(layout: Layout, x: number): number {
  const place = layout.placed.find((held) => x < held.x + held.w && held.frames > 0) ?? layout.placed.filter((held) => held.frames > 0).at(-1)
  if (!place) return 0
  const share = Math.max(0, Math.min(1, (x - place.x) / place.w))
  return Math.min(place.at + Math.floor(share * place.frames), Math.max(0, layout.frames - 1))
}

/** Where a clip dropped or dragged at `x` goes: before the first clip whose middle is past it. */
export function indexAt(layout: Layout, x: number): number {
  const after = layout.placed.find((place) => x < place.x + place.w / 2)
  return after ? after.index : layout.placed.length
}

export function moved<T>(items: T[], from: number, to: number): T[] {
  const held = [...items]
  const [item] = held.splice(from, 1)
  held.splice(to > from ? to - 1 : to, 0, item)
  return held
}

/** The in point moved by `by`, the out point staying. A still has no in point to pull, so its duration gives. */
export function trimmedIn(clip: ClipNow, fps: number, by: number): ClipNow {
  const held = span(clip, fps)
  if (isStill(clip)) return { ...clip, frames: Math.max(1, held.out - by) }
  return { ...clip, in: Math.max(0, Math.min(held.out - 1, held.in + by)), out: held.out }
}

/** The out point moved by `by`, within the source. Pulled all the way out, it's the whole source again. */
export function trimmedOut(clip: ClipNow, fps: number, by: number): ClipNow {
  const held = span(clip, fps)
  const to = Math.max(held.in + 1, held.out + by)
  if (isStill(clip)) return { ...clip, frames: to }
  return { ...clip, in: held.in, out: to >= (clip.holds ?? Number.POSITIVE_INFINITY) ? undefined : to }
}

/** What `PATCH` writes: `<name>`, and only what says which part of it. Its place in the list is its place on the track. */
export const wire = (clips: ClipNow[]) => ({
  tracks: [
    {
      clips: clips.map((clip): Clip => {
        if (isStill(clip)) return { clip: clip.clip, ...(clip.frames === undefined ? {} : { frames: clip.frames }) }
        return { clip: clip.clip, ...(clip.in ? { in: clip.in } : {}), ...(clip.out === undefined ? {} : { out: clip.out }) }
      }),
    },
  ],
})

/** SMPTE timecode, non-drop: `HH:MM:SS:FF`, where people read time. A trim is read in frames. */
export function timecode(frame: number, fps: number): string {
  const rate = Math.max(1, Math.round(fps))
  const seconds = Math.floor(frame / rate)
  const pad = (value: number) => String(value).padStart(2, "0")
  return `${pad(Math.floor(seconds / 3600))}:${pad(Math.floor(seconds / 60) % 60)}:${pad(seconds % 60)}:${pad(frame % rate)}`
}

/** What's known of an asset dropped in, until Calcifer has read the Edit that holds it. */
export function clipOf(store: CanvasStore, name: string, fps: number): ClipNow | null {
  const view = store.get(name)
  const shown = view && shownOf(view)
  const field = shown?.field ?? view?.asset?.kind
  if (field !== "video" && field !== "image") return null
  return {
    clip: mention(view?.name ?? name),
    name: view?.name ?? name,
    field,
    frames: field === "image" ? Math.round(STILL_S * fps) : undefined,
    sha256: shown?.sha256,
    path: shown?.path,
    width: shown?.width,
    height: shown?.height,
    holds: field === "video" && shown?.duration_ms ? Math.floor((shown.duration_ms * fps) / 1000) : undefined,
    missing: shown ? undefined : "no version yet",
  }
}
