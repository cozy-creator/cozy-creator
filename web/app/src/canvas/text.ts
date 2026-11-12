// What the page reads of an asset's words. `text` is whatever a person or a pod wrote, so each read takes only the shape it wants and
// nothing here is more than text.

import type { ToolField } from "../api/calcifer"

export type Row = Record<string, unknown>

const string = (value: unknown): string | null => (typeof value === "string" && value.trim() ? value : null)

/** What a group holds: only its rows. */
export const rowsIn = (value: unknown): Row[] => (Array.isArray(value) ? value.filter((row): row is Row => typeof row === "object" && row !== null) : [])

/** What an Edit holds, in order, as its `timeline` says: the names in `<>`. */
export function piecesOf(text?: Record<string, unknown>): string[] {
  const clips = (text?.timeline as { tracks?: { clips?: { clip?: unknown }[] }[] } | undefined)?.tracks?.[0]?.clips
  if (!Array.isArray(clips)) return []
  return clips.flatMap((clip) => {
    const held = string(clip?.clip)?.match(/<([^<>\s]+)>/)
    return held ? [held[1]] : []
  })
}

/** The words a card shows when it has nothing else to show: its first prompt's, at the top or in its rows. Never its description, which is off
 *  for now (cl-900). */
export function lineOf(text: Record<string, unknown> | undefined, fields: ToolField[]): string | null {
  const isText = (field: ToolField) => field.type === "prompt" || field.type === "words"
  const said = fields.flatMap((field) =>
    field.type === "group" ? rowsIn(text?.[field.field]).flatMap((row) => (field.members ?? []).filter(isText).map((member) => row[member.field])) : isText(field) ? [text?.[field.field]] : [],
  )
  return said.map(string).find((words) => words !== null) ?? null
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

/** A mention as a chip shows it: in its brackets, first letter up, `<Self.2>`. The file keeps the name as typed (names ignore case). */
export const chipWords = (name: string) => `<${name.charAt(0).toUpperCase()}${name.slice(1)}>`

/** The names `<like this>` in `text`, once each, in order. */
export const mentionsIn = (text: string): string[] => [...new Set(withMentions(text).filter((part) => part.mention).map((part) => part.text.slice(1, -1)))]

/** Every text under `value`, where Calcifer finds mentions. */
export const textsIn = (value: unknown): string[] => (typeof value === "string" ? [value] : typeof value === "object" && value !== null ? Object.values(value).flatMap(textsIn) : [])

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
