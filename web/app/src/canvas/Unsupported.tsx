import type { ToolField, Unused } from "../api/calcifer"
import "./Unsupported.css"

const FILES = ["image", "video", "audio"]

/** Whether a box's `field` can use a mention of a box of `kind`, by the rule Calcifer strikes the rest through by (binder.rs `unusable`):
 *  words take a text box's words only; a prompt those and the files it declares (all of them when `takes` is unset), every file when it
 *  takes `file`. Words of no tool's (a Text box's) take anything. What's offered to mention is only what's usable (Paul, 11-16). */
export function usable(field: ToolField | undefined, kind: string | undefined): boolean {
  if (!field || !kind || kind === "text") return true
  if (field.type !== "prompt") return false
  return !field.takes || field.takes.includes(kind) || (field.takes.includes("file") && FILES.includes(kind))
}

/** The mark on whatever a box's words or inputs hold that its tool can't use (cl-944): struck through in red, Calcifer's reason on hover, and
 *  never sent. Every such case is this one mark (Paul, 11-16: "that is the general syntax for any unsupported issue"). */
export const UNSUPPORTED = "unsupported"

/** The reasons, each once, as one line under what they're written in. */
export function UnsupportedNote({ unused }: { unused: Unused[] }) {
  const reasons = [...new Set(unused.map((one) => one.why))]
  return reasons.length ? <p className="unsupported-note" role="note">{reasons.join(". ")}</p> : null
}
