import { useSignal } from "../ui/signal"
import type { ElementView } from "./CanvasStore"
import { useCanvasStore } from "./context"
import { orbitOf } from "./Orbit"
import { FPS, framesIn, pickupAt } from "./ranges"
import { makes, shownOf } from "./shown"

/** Seconds as "0:12.96". */
const clock = (seconds: number) => `${Math.floor(seconds / 60)}:${(seconds % 60).toFixed(2).padStart(5, "0")}`

/** Under a video looked at in Focus, at its bottom right: Pick up from where the player is (Paul, 11-12). Its label is where the cut lands:
 *  the tool's grid point at or before the player's position (H3's `17k + 5`), following the player as it plays or is scrubbed. */
export function PickUpHere({ name, view }: { name: string; view: ElementView }) {
  const store = useCanvasStore()
  const tools = useSignal(store.tools)
  const shown = shownOf(view)
  const pickup = tools.get(view.asset?.tool ?? "")?.pickup
  if (!pickup || !makes(view) || !shown || shown.field !== "video" || !shown.duration_ms || shown.number === undefined || orbitOf(shown)) return null
  const fps = shown.fps ?? FPS
  const frame = Math.round((view.scrub ?? 0) * fps)
  const from = pickupAt(frame, framesIn(shown.duration_ms, fps), pickup)
  return (
    <button
      type="button"
      className="tb pick-up nodrag nopan"
      disabled={from === null}
      title={from === null ? `This tool picks up from ${clock(pickup.first / fps)} on` : `Keep up to ${clock(from / fps)} and render the rest again`}
      onClick={() => from !== null && void store.beginVideoPickup(name, from / fps)}
    >
      Pick up from {clock((from ?? frame) / fps)}
    </button>
  )
}
