import { useQueries } from "@tanstack/react-query"
import type { Chosen, Tool } from "../api/calcifer"
import { useConnection } from "../app/Connection"
import type { CanvasStore } from "./CanvasStore"
import { mainText } from "./controls"

// A LoRA's triggers (Paul, 11-12): its Hub card's, shown on its row beneath the box and drawn in a token colour wherever a prompt of that
// box says them; the first is appended to a box's prompt when it's added. They stay plain text, sent as written. Each is one entry, as
// Civitai's trainedWords are (cl-971): a word, a few words or a whole prompt, kept whole.

/** What adding a LoRA puts after the prompt: its first trigger. Paul, 11-19: "you can have loras with many trigger words, sure. Just include
 *  one though when you add it to the user's prompt in the webui" (cl-1287). The rest are one click away on its row. */
export const added = (triggers: string[]) => triggers.slice(0, 1)

type Entry = Record<string, unknown>
const entryOf = (value: unknown): Entry => value && typeof value === "object" && !Array.isArray(value) ? value as Entry : {}
const refsIn = (value: unknown): string[] => Array.isArray(value) ? value.flatMap((held) => typeof entryOf(held).ref === "string" ? [entryOf(held).ref as string] : []) : []
const identityOf = (ref: string) => ref.split("@")[0]

/** The LoRAs on box `name`: each slot's, its own, and what its tool's rules chose for it (`chosen`, cl-1281: a base model's card has
 *  no triggers). */
export function lorasOn(store: CanvasStore, name: string): string[] {
  const slots = Object.values(entryOf(store.valueOf(name, "models"))).flatMap((held) => refsIn(entryOf(held).lora))
  const chosen = store.get(name)?.asset?.chosen?.map((held) => held.model) ?? []
  return [...new Set([...slots, ...refsIn(store.valueOf(name, "lora")), ...chosen].map(identityOf))]
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

/** `text` without the `triggers` it ends with as `appended` put them, each after the comma or space it went after, or null when it ends
 *  with none of them so: one typed over or typed after is the person's. */
export function unappended(text: string, triggers: string[]): string | null {
  let said = text.trimEnd()
  for (let word = triggers.find((held) => held && said.endsWith(held)); word; word = triggers.find((held) => held && said.endsWith(held))) {
    const kept = said.slice(0, -word.length)
    if (kept && !kept.endsWith(", ") && !(kept.endsWith(" ") && /[.,;:!?]$/.test(kept.slice(0, -1)))) break
    said = kept.endsWith(", ") ? kept.slice(0, -2) : kept.trimEnd()
  }
  return said === text.trimEnd() ? null : said
}

/** A tool's prompt: its main text (`mainText`). */
export const promptOf = (tool: Tool | undefined): string | undefined => mainText(tool?.fields ?? [])?.field

// A LoRA a tool's rule chose (cl-1281; Paul, 11-18: "when you add in lora-A in the webui, it modifies your prompt to append the trigger word
// for that lora … which you as an author (the end user) can remove"): the change this page made that brings one in (an edit's type, a tool
// put on, a box made) puts its first trigger after the prompt as adding a LoRA does, and takes the one it replaced off where the page put it. A
// load, a reload or another writer's change brings nothing, so a trigger deleted stays deleted.

const seen = new WeakMap<CanvasStore, Map<string, string[]>>()
const turns = new WeakMap<CanvasStore, Map<string, Promise<void>>>()
const chosenOf = (chosen: Chosen[]) => [...new Set(chosen.map((held) => identityOf(held.model)))]

/** Box `name` read with `now` chosen, its view having held `was` (none: not known); `mine`, a read of this page's own write. */
export function chose(store: CanvasStore, name: string, was: Chosen[] | null | undefined, now: Chosen[] | null | undefined, mine: boolean) {
  let held = seen.get(store)
  if (!held) seen.set(store, (held = new Map()))
  // What it last held as this page knew it: a read that didn't know says nothing of it.
  const key = `${store.projectId}\u0000${name}`
  const before = was ? chosenOf(was) : held.get(key)
  if (!now) return void (before && held.set(key, before))
  const after = chosenOf(now)
  held.set(key, after)
  if (!mine || !before) return
  const brought = after.filter((model) => !before.includes(model))
  const left = before.filter((model) => !after.includes(model))
  if (!brought.length && !left.length) return
  let queue = turns.get(store)
  if (!queue) turns.set(store, (queue = new Map()))
  queue.set(name, (queue.get(name) ?? Promise.resolve()).then(() => retrigger(store, name, brought, left)).catch(() => undefined))
}

async function retrigger(store: CanvasStore, name: string, brought: string[], left: string[]) {
  const triggersOf = async (models: string[]) => (await Promise.all(models.map((model) => store.modelDetail(model).then((card) => card.kind === "lora" ? card.triggers ?? [] : [], () => [])))).flat()
  const [cards, staying] = await Promise.all([Promise.all([...brought, ...left].map(async (model) => [model, await triggersOf([model])] as const)), triggersOf(lorasOn(store, name))])
  // As the box stands once its cards are read: it may have moved on, and the next change says the rest.
  const now = chosenOf(store.get(name)?.asset?.chosen ?? [])
  const adding = cards.filter(([model]) => brought.includes(model) && now.includes(model)).flatMap(([, words]) => added(words))
  const removing = cards.filter(([model]) => left.includes(model) && !now.includes(model)).flatMap(([, words]) => words)
  const field = promptOf(store.tools.get().get(store.get(name)?.asset?.tool ?? ""))
  if (!field) return
  const said = store.valueOf(name, field)
  const text = typeof said === "string" ? said : ""
  const without = unappended(text, removing.filter((word) => !adding.includes(word) && !staying.includes(word))) ?? text
  const next = appended(without, adding) ?? without
  if (next !== text) store.putOn(name, { [field]: next })
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
