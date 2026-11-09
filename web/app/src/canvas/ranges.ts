// A video mentioned for a range of its frames, `<walk:48-120>` (Paul, 11-06): `in` to `out` in the source's own frames, `<walk.2:48-120>` of a
// version, and `<walk:48->` to its end. `:` is no name character, so the rest of a mention is what it names.

/** A video's frames a second when its file doesn't say. */
export const FPS = 24

export interface Ranged {
  base: string
  from: number
  to?: number
}

/** `walk.2:48-120` as what it names and its frames. */
export function rangeOf(mention: string): Ranged | null {
  const found = /^(.+):(\d+)-(\d*)$/.exec(mention)
  return found ? { base: found[1], from: Number(found[2]), to: found[3] ? Number(found[3]) : undefined } : null
}

/** What a mention names, without its range. */
export const baseOf = (mention: string) => rangeOf(mention)?.base ?? mention

/** A mention of frames `from` to `to` of a video of `last` frames: the whole of it says no range. */
export function ranged(base: string, from: number, to: number, last: number): string {
  if (from <= 0 && to >= last) return base
  return `${base}:${Math.max(0, from)}-${to >= last ? "" : to}`
}

/** A frame as timecode: minutes, seconds and the frame within that second. */
export function timecode(frame: number, fps: number): string {
  const seconds = Math.floor(frame / fps)
  const within = Math.round(frame - seconds * fps)
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}.${String(within).padStart(2, "0")}`
}

/** How many frames a video of `ms` holds. */
export const framesIn = (ms: number | null | undefined, fps: number) => Math.max(1, Math.round(((ms ?? 0) / 1000) * fps))

/** `<old>` written as `<now>` wherever the text says it. */
export const retext = (text: string, old: string, now: string) => text.split(`<${old}>`).join(`<${now}>`)
