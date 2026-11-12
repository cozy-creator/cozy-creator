import type { Splice } from "../api/calcifer"

/** What turns `from` into `to`, in Unicode code points, as Calcifer's Automerge text counts: one splice where they differ. */
export function spliceOf(from: string, to: string): Splice {
  const [a, b] = [Array.from(from), Array.from(to)]
  let prefix = 0
  while (prefix < a.length && prefix < b.length && a[prefix] === b[prefix]) prefix++
  let suffix = 0
  while (suffix < a.length - prefix && suffix < b.length - prefix && a[a.length - 1 - suffix] === b[b.length - 1 - suffix]) suffix++
  return { at: prefix, delete: a.length - prefix - suffix, insert: b.slice(prefix, b.length - suffix).join("") }
}

const apply = (points: string[], splice: Splice): string[] => [...points.slice(0, splice.at), ...Array.from(splice.insert), ...points.slice(splice.at + splice.delete)]

/** `ours` and `theirs`, each made from `base`, as one: both deletions and both insertions, as Automerge merges text. */
export function merge(base: string, ours: string, theirs: string): string {
  if (ours === base) return theirs
  if (theirs === base || theirs === ours) return ours
  const points = Array.from(base)
  const [mine, other] = [spliceOf(base, ours), spliceOf(base, theirs)]
  if (mine.at + mine.delete <= other.at) return apply(apply(points, other), mine).join("")
  if (other.at + other.delete <= mine.at) return apply(apply(points, mine), other).join("")
  const start = Math.min(mine.at, other.at)
  const end = Math.max(mine.at + mine.delete, other.at + other.delete)
  const inserts = mine.at <= other.at ? mine.insert + other.insert : other.insert + mine.insert
  return [...points.slice(0, start), ...Array.from(inserts), ...points.slice(end)].join("")
}

const isMap = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value)

/** The same value however its keys were ordered. */
export function same(a: unknown, b: unknown): boolean {
  if (Array.isArray(a) && Array.isArray(b)) return a.length === b.length && a.every((held, i) => same(held, b[i]))
  if (isMap(a) && isMap(b)) {
    const keys = Object.keys(a)
    return keys.length === Object.keys(b).length && keys.every((key) => key in b && same(a[key], b[key]))
  }
  return (a ?? null) === (b ?? null)
}

/** Each place `to`'s words differ from `from`'s, by path (`0.description`, "" for words alone), when only words differ; else null. */
export function wordsChanged(from: unknown, to: unknown, at = ""): { path: string; from: string; to: string }[] | null {
  const under = (key: string | number) => (at ? `${at}.${key}` : String(key))
  if (typeof from === "string" && typeof to === "string") return from === to ? [] : [{ path: at, from, to }]
  if (Array.isArray(from) && Array.isArray(to)) {
    if (from.length !== to.length) return null
    const each = from.map((held, i) => wordsChanged(held, to[i], under(i)))
    return each.some((one) => one === null) ? null : each.flatMap((one) => one!)
  }
  if (isMap(from) && isMap(to)) {
    if (!same(Object.keys(from).sort(), Object.keys(to).sort())) return null
    const each = Object.keys(from).map((key) => wordsChanged(from[key], to[key], under(key)))
    return each.some((one) => one === null) ? null : each.flatMap((one) => one!)
  }
  return same(from, to) ? [] : null
}

/** `ours` and `theirs`, each made from `base`, as one: words merged by character wherever all three are the same shape; elsewhere ours, unless
 *  ours is still `base`. */
export function mergeValue(base: unknown, ours: unknown, theirs: unknown): unknown {
  if (same(ours, base)) return theirs
  if (same(theirs, base) || same(theirs, ours)) return ours
  if (typeof base === "string" && typeof ours === "string" && typeof theirs === "string") return merge(base, ours, theirs)
  if (Array.isArray(base) && Array.isArray(ours) && Array.isArray(theirs) && base.length === ours.length && base.length === theirs.length) {
    return base.map((held, i) => mergeValue(held, ours[i], theirs[i]))
  }
  if (isMap(base) && isMap(ours) && isMap(theirs)) {
    const keys = [...new Set([...Object.keys(ours), ...Object.keys(theirs)])]
    return Object.fromEntries(keys.flatMap((key) => {
      const merged = mergeValue(base[key], ours[key], theirs[key])
      return merged === undefined ? [] : [[key, merged]]
    }))
  }
  return ours
}
