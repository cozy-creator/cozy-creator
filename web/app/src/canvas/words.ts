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
