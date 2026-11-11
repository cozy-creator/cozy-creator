// A video mentioned for a range of its frames, `<walk:48-120>` (Paul, 11-06): `in` to `out` in the source's own frames, `<walk.2:48-120>` of a
// version, and `<walk:48->` to its end. An orbit by angle (Paul, 11-11): `<cat:120deg>` the frame at 120° as a picture, `<cat:90deg-180deg>`
// that part of the turn as a clip. `:` is no name character, so the rest of a mention is what it names.

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

export interface Angled {
  base: string
  from: number
  /** A turn's end; none: the frame at `from`, a picture. */
  to?: number
}

/** `cat.2:120deg` or `cat:90deg-180deg` as what it names and its angles. */
export function angleOf(mention: string): Angled | null {
  const found = /^(.+):(\d+(?:\.\d+)?)deg(?:-(\d+(?:\.\d+)?)deg)?$/.exec(mention)
  return found ? { base: found[1], from: Number(found[2]), to: found[3] === undefined ? undefined : Number(found[3]) } : null
}

/** An orbit's angles as a chip says them: `120°`, `90°–180°`. */
export const degrees = (angled: Angled) => (angled.to === undefined ? `${angled.from}°` : `${angled.from}°–${angled.to}°`)

/** What a mention names, without its range or angle. */
export const baseOf = (mention: string) => rangeOf(mention)?.base ?? angleOf(mention)?.base ?? mention

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
