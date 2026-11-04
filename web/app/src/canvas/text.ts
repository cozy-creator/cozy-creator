// What the page reads of an asset's words. `text` is whatever a person or a pod wrote, so each read takes only the shape it wants and
// nothing here is more than text.

export interface Write {
  image: string
  at?: number
}

export interface Beat {
  duration?: number
  description: string
  writes: Write[]
}

const string = (value: unknown): string | null => (typeof value === "string" && value.trim() ? value : null)
const number = (value: unknown): number | undefined => (typeof value === "number" && Number.isFinite(value) ? value : undefined)

/** How long a beat with no `duration` lasts, as Calcifer reads it. */
const BEAT_S = 5

export function beatsOf(text?: Record<string, unknown>): Beat[] {
  const beats = text?.beats
  if (!Array.isArray(beats)) return []
  return beats.flatMap((beat): Beat[] => {
    if (typeof beat !== "object" || beat === null) return []
    const { duration, description, writes } = beat as Record<string, unknown>
    const written = (Array.isArray(writes) ? writes : []).flatMap((write): Write[] => {
      const image = string((write as Record<string, unknown> | null)?.image)
      return image ? [{ image, at: number((write as Record<string, unknown>).at) }] : []
    })
    return [{ duration: number(duration), description: string(description) ?? "", writes: written }]
  })
}

export const beatLength = (beat: Beat) => beat.duration ?? BEAT_S

/** What a cut holds, in order, as its text says: the names in `<>`. */
export function piecesOf(text?: Record<string, unknown>): string[] {
  const pieces = text?.pieces
  if (!Array.isArray(pieces)) return []
  return pieces.flatMap((piece) => {
    const held = string(piece)?.match(/<([^<>\s]+)>/)
    return held ? [held[1]] : []
  })
}

/** The words a card shows when it has nothing else to show. */
export function lineOf(text?: Record<string, unknown>): string | null {
  return string(text?.instructions) ?? string(text?.summary) ?? string(text?.description) ?? beatsOf(text)[0]?.description ?? null
}

/** The text at a place an asset's `reads` names: `summary`, `beats.0.description`. */
export function textAt(text: Record<string, unknown> | undefined, place: string): string | null {
  let value: unknown = text
  for (const part of place.split(".")) {
    if (typeof value !== "object" || value === null) return null
    value = (value as Record<string, unknown>)[part]
  }
  return string(value)
}

/** `beats.0.description` as a person says it. */
export function placeName(place: string): string {
  const [field, n] = place.split(".")
  return n !== undefined && /^\d+$/.test(n) ? `${field === "beats" ? "beat" : field} ${Number(n) + 1}` : field
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
