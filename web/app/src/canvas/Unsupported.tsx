import type { Unused } from "../api/calcifer"
import "./Unsupported.css"

/** The mark on whatever a box's words or inputs hold that its tool can't use (cl-944): struck through in red, Calcifer's reason on hover, and
 *  never sent. Every such case is this one mark (Paul, 11-16: "that is the general syntax for any unsupported issue"). */
export const UNSUPPORTED = "unsupported"

/** The reasons, each once, as one line under what they're written in. */
export function UnsupportedNote({ unused }: { unused: Unused[] }) {
  const reasons = [...new Set(unused.map((one) => one.why))]
  return reasons.length ? <p className="unsupported-note" role="note">{reasons.join(". ")}</p> : null
}
