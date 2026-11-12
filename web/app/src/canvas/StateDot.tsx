import type { Asset } from "../api/calcifer"
import type { ElementView } from "./CanvasStore"
import { faceOf } from "./shown"

/** What a node is doing, as its dot says it (Paul, 11-12): grey queued, blue running, green made and not looked at, red failed. */
export type NodeState = "queued" | "running" | "unseen" | "failed"

/** A node's state, the most pressing first: a failure, its render waiting its turn, then running, then a take not looked at; else none.
 *  `waiting`: its render is asked and not running yet, by this page or in its project's or machine's line. `failureShown`: what failed
 *  is what it shows (an approved box looked at shows nothing wrong). */
export function nodeState(asset: Asset | undefined, { waiting = false, failureShown = true }: { waiting?: boolean; failureShown?: boolean } = {}): NodeState | null {
  if (!asset) return waiting ? "queued" : null
  if (asset.failure && failureShown) return "failed"
  if (waiting) return "queued"
  if (asset.standing === "making") return "running"
  if (asset.unseen?.length) return "unseen"
  return null
}

/** A box's state on this page: asked and not answered, in its project's line, or on its machine and not started is waiting; an approved
 *  box looked at shows nothing wrong. */
export function viewState(view: ElementView): NodeState | null {
  const asset = view.asset
  if (view.placing) return null
  const run = view.run
  // One rendered here (an Edit's cut) has no machine to wait for.
  const notStarted = asset?.standing === "making" && asset.rendering == null && (!run?.machine || run.queue_position != null || run.machine_state === "queued")
  const waiting = Boolean(view.queued) || view.asked !== undefined || notStarted
  return nodeState(asset, { waiting, failureShown: !(asset?.pinned && faceOf(view) === "media") })
}

const SAID: Record<NodeState, string> = { queued: "Queued", running: "Rendering", unseen: "New, not looked at", failed: "Failed" }

/** A node's state, left of its name, wherever it's shown: its tile, its Focus header, a list of them. Nothing when there's nothing to say. */
export function StateDot({ state, unseen = 0, noun = "take" }: { state: NodeState | null; unseen?: number; noun?: string }) {
  if (!state) return null
  const said = state === "unseen" ? `${unseen} new ${noun}${unseen === 1 ? "" : "s"} you haven't looked at` : SAID[state]
  return <span className={`state-dot ${state}`} data-state={state} role="img" aria-label={said} title={said} />
}
