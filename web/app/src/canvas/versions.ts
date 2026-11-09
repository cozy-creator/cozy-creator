import type { Asset, VersionPrefix } from "../api/calcifer"
import { nameKey } from "./names"

// How a text says one version, the same rule as `version_of` (`crates/calcifer/src/project/graph.rs`): `<abr>` moves with the box, and `<abr.2>` is that version for good.

/** What a box calls itself in its own text. */
export const SELF = "self"

/** `abr.2` as the name it's a version of, and that number. */
export function versionOf(mention: string): { name: string; number: number } | null {
  const found = /^(.+)\.(\d+)$/.exec(mention)
  return found ? { name: found[1], number: Number(found[2]) } : null
}

/** What `<self>` is told. */
export const NO_BARE_SELF = "<self> alone would change with every render: write <self.1>, the version to start from"

export const isSelf = (mention: string) => nameKey(mention) === SELF

/** The versions a box has: a file with none yet would be its first. */
export function numbersOf(asset: Asset | undefined): number[] {
  const held = (asset?.prefixes ?? []).map((version) => version.number)
  return held.length || !asset?.sha256 ? held : [1]
}

/** The version `number` of a box, as its row holds it. */
export const versionIn = (asset: Asset | undefined, number: number): VersionPrefix | undefined => asset?.prefixes?.find((held) => held.number === number)

/** What a mention of a version of `name` says in `asset`'s text. */
export const mentionFrom = (asset: string, name: string, number: number) => `${nameKey(asset) === nameKey(name) ? SELF : name}.${number}`
