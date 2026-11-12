import { useQueries } from "@tanstack/react-query"
import type { Tool } from "../api/calcifer"
import { useConnection } from "../app/Connection"
import type { CanvasStore } from "./CanvasStore"

// A LoRA's triggers (Paul, 11-12): its Hub card's, appended to a box's prompt when it's added, shown on its row beneath the box, and
// drawn in a token colour wherever a prompt of that box says them. They stay plain text, sent as written. Each is one entry, as Civitai's
// trainedWords are (cl-971): a word, a few words or a whole prompt, kept whole.

type Entry = Record<string, unknown>
const entryOf = (value: unknown): Entry => value && typeof value === "object" && !Array.isArray(value) ? value as Entry : {}
const refsIn = (value: unknown): string[] => Array.isArray(value) ? value.flatMap((held) => typeof entryOf(held).ref === "string" ? [entryOf(held).ref as string] : []) : []
const identityOf = (ref: string) => ref.split("@")[0]

/** The LoRAs on box `name`: each slot's and its own. */
export function lorasOn(store: CanvasStore, name: string): string[] {
  const slots = Object.values(entryOf(store.valueOf(name, "models"))).flatMap((held) => refsIn(entryOf(held).lora))
  return [...new Set([...slots, ...refsIn(store.valueOf(name, "lora"))].map(identityOf))]
}

/** The triggers of the LoRAs on box `name`, as their Hub cards say them. */
export function useTriggers(store: CanvasStore, name: string): string[] {
  const { client } = useConnection()
  const refs = lorasOn(store, name)
  const details = useQueries({
    queries: refs.map((ref) => ({ queryKey: ["model", ref], queryFn: ({ signal }: { signal: AbortSignal }) => client.model(ref, signal), staleTime: 60_000, retry: false })),
  })
  return [...new Set(details.flatMap((detail) => detail.data?.triggers ?? []))]
}

const WORD = /[\p{L}\p{N}_]/u
/** Where `word` stands in `text` as itself, not inside a longer word: each [start, end). */
function found(text: string, word: string): [number, number][] {
  const out: [number, number][] = []
  const lower = text.toLowerCase()
  const sought = word.toLowerCase()
  for (let at = lower.indexOf(sought); at >= 0; at = lower.indexOf(sought, at + 1)) {
    const end = at + sought.length
    const before = at > 0 && WORD.test(text[at - 1]) && WORD.test(sought[0])
    const after = end < text.length && WORD.test(text[end]) && WORD.test(sought[sought.length - 1])
    if (!before && !after) out.push([at, end])
  }
  return out
}

/** Whether `text` says `word`, as itself. */
export const says = (text: string, word: string) => found(text, word).length > 0

/** `text` with the triggers it doesn't say yet after it, each once and whole, or null when it says them all. */
export function appended(text: string, triggers: string[]): string | null {
  const missing = triggers.filter((word) => !says(text, word))
  if (!missing.length) return null
  return missing.reduce((said, word) => {
    const kept = said.trimEnd()
    return `${kept}${!kept ? "" : /[.,;:!?]$/.test(kept) ? " " : ", "}${word}`
  }, text)
}

// Nothing marks a negative prompt yet (the SDK's `NegativePrompt` is plain text), so its name stands in for the mark.
const NEGATIVE = /^neg(ative)?(_|$)/i

/** A tool's prompt: its first `prompt` field, else its first words (SDXL's and Ideogram 4's `str`); never a negative prompt. */
export function promptOf(tool: Tool | undefined): string | undefined {
  const fields = tool?.fields.filter((field) => !NEGATIVE.test(field.field)) ?? []
  return (fields.find((field) => field.type === "prompt") ?? fields.find((field) => field.type === "words"))?.field
}

// The highlight: ranges over each prompt's own text nodes (never its chips), one CSS highlight for the page, so nothing a person types moves.

const painted = new Map<HTMLElement, Range[]>()
const HIGHLIGHT = "lora-trigger"

function repaint() {
  const highlights = (globalThis.CSS as { highlights?: Map<string, unknown> } | undefined)?.highlights
  const Made = (globalThis as { Highlight?: new (...ranges: Range[]) => unknown }).Highlight
  if (!highlights || !Made) return
  const all = [...painted.values()].flat()
  if (all.length) highlights.set(HIGHLIGHT, new Made(...all))
  else highlights.delete(HIGHLIGHT)
}

/** Draws `triggers` wherever `el`'s words say them; none, it draws nothing there. */
export function paintTriggers(el: HTMLElement, triggers: string[]) {
  const ranges: Range[] = []
  if (triggers.length) {
    // Runs of text nodes, each read as one text: a browser splits what's typed across nodes.
    const runs: Text[][] = [[]]
    const walk = (parent: Node) => {
      for (const child of parent.childNodes) {
        if (child instanceof Text) runs[runs.length - 1].push(child)
        else if (child instanceof HTMLElement && (child.dataset.mention || child instanceof HTMLBRElement)) runs.push([])
        else walk(child)
      }
    }
    walk(el)
    for (const run of runs.filter((nodes) => nodes.length)) {
      const text = run.map((node) => node.data).join("")
      const at = (offset: number): [Text, number] => {
        let left = offset
        for (const node of run) {
          if (left <= node.data.length) return [node, left]
          left -= node.data.length
        }
        const last = run[run.length - 1]
        return [last, last.data.length]
      }
      for (const word of triggers) {
        for (const [start, end] of found(text, word)) {
          const range = document.createRange()
          range.setStart(...at(start))
          range.setEnd(...at(end))
          ranges.push(range)
        }
      }
    }
  }
  if (ranges.length) painted.set(el, ranges)
  else painted.delete(el)
  repaint()
}

export function unpaint(el: HTMLElement) {
  painted.delete(el)
  repaint()
}

// Words put where a box's prompt's caret is: a trigger clicked on its LoRA's row goes in the prompt of that box last typed in.

const inserters = new Map<string, (text: string) => void>()
const lastTyped = new Map<string, string>()

/** Prompt `place` of box `name` takes words at its caret. */
export function takesWords(name: string, place: string, insert: (text: string) => void): () => void {
  inserters.set(`${name}\u0000${place}`, insert)
  return () => void inserters.delete(`${name}\u0000${place}`)
}

/** Prompt `place` of box `name` was typed in last. */
export const typedIn = (name: string, place: string) => void lastTyped.set(name, place)

/** `text` at the caret of the prompt of box `name` last typed in, else its first. Whether one took it. */
export function insertAtCaret(name: string, text: string): boolean {
  const place = lastTyped.get(name)
  const insert = (place && inserters.get(`${name}\u0000${place}`)) || [...inserters.entries()].find(([key]) => key.startsWith(`${name}\u0000`))?.[1]
  insert?.(text)
  return Boolean(insert)
}
