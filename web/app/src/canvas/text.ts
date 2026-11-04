// What the page reads of an asset's words. `text` is whatever a person or a pod wrote, so each read takes only the shape it wants and
// nothing here is more than text.

export interface Beat {
  duration?: number
  description: string
}

const string = (value: unknown): string | null => (typeof value === "string" && value.trim() ? value : null)
const number = (value: unknown): number | undefined => (typeof value === "number" && Number.isFinite(value) ? value : undefined)

/** How long a beat with no `duration` lasts, as Calcifer reads it. */
export const BEAT_S = 5

export function beatsOf(text?: Record<string, unknown>): Beat[] {
  const beats = text?.beats
  if (!Array.isArray(beats)) return []
  return beats.flatMap((beat): Beat[] => {
    if (typeof beat !== "object" || beat === null) return []
    const { duration, description } = beat as Record<string, unknown>
    return [{ duration: number(duration), description: string(description) ?? "" }]
  })
}

export const beatLength = (beat: Beat) => beat.duration ?? BEAT_S

/** What an Edit holds, in order, as its `timeline` says: the names in `<>`. */
export function piecesOf(text?: Record<string, unknown>): string[] {
  const clips = (text?.timeline as { tracks?: { clips?: { clip?: unknown }[] }[] } | undefined)?.tracks?.[0]?.clips
  if (!Array.isArray(clips)) return []
  return clips.flatMap((clip) => {
    const held = string(clip?.clip)?.match(/<([^<>\s]+)>/)
    return held ? [held[1]] : []
  })
}

/** The words a card shows when it has nothing else to show. */
export function lineOf(text?: Record<string, unknown>): string | null {
  return string(text?.instructions) ?? string(text?.summary) ?? string(text?.description) ?? beatsOf(text)[0]?.description ?? null
}

/** Text with its mentions told apart, for showing, never as HTML. */
export function withMentions(text: string): { text: string; mention: boolean }[] {
  const parts: { text: string; mention: boolean }[] = []
  let at = 0
  for (const found of text.matchAll(/<[^<>\s]+>/g)) {
    if (found.index > at) parts.push({ text: text.slice(at, found.index), mention: false })
    parts.push({ text: found[0], mention: true })
    at = found.index + found[0].length
  }
  if (at < text.length) parts.push({ text: text.slice(at), mention: false })
  return parts
}

/** The names `<like this>` in `text`, once each, in order. */
export const mentionsIn = (text: string): string[] => [...new Set(withMentions(text).filter((part) => part.mention).map((part) => part.text.slice(1, -1)))]

/** What Calcifer said, for a page that never shows a hash. */
export const noHash = (said: string) => said.replace(/\b(?:sha256:)?[0-9a-f]{12,64}\b/g, "a file")

export function clock(ms: number): string {
  const seconds = Math.max(0, Math.round(ms / 1000))
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`
}

export function took(ms: number): string {
  const seconds = Math.round(ms / 1000)
  if (seconds < 90) return `${seconds} s`
  const minutes = Math.round(seconds / 60)
  return minutes < 90 ? `${minutes} min` : `${(minutes / 60).toFixed(1)} h`
}
