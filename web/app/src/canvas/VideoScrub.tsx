import { useSignal } from "../ui/signal"
import type { ElementView } from "./CanvasStore"
import { useCanvasStore } from "./context"
import { orbitOf } from "./Orbit"
import { FPS, framesIn, timecode } from "./ranges"
import { makes, shownOf } from "./shown"

/** A video looked at in Focus: a scrubber under it, where it is in time, and, for a tool that picks up, Pick up from here at that frame
 *  (Paul, 11-12). An orbit turns by its own control instead. */
export function VideoScrub({ name, view }: { name: string; view: ElementView }) {
  const store = useCanvasStore()
  const tools = useSignal(store.tools)
  const shown = shownOf(view)
  if (!shown || shown.field !== "video" || !shown.duration_ms || orbitOf(shown)) return null
  const fps = shown.fps ?? FPS
  const last = framesIn(shown.duration_ms, fps)
  const frame = Math.min(last, Math.max(0, Math.round((view.scrub ?? 0) * fps)))
  const go = (to: number) => {
    const at = Math.max(0, Math.min(last, to)) / fps
    store.patch(name, { scrub: at })
    store.seek(name, at, true)
  }
  const picks = makes(view) && Boolean(tools.get(view.asset?.tool ?? "")?.pickup) && shown.number !== undefined
  return (
    <div className="video-scrub nodrag nopan" role="group" aria-label="Scrub the video">
      <input type="range" aria-label="Where in the video" min={0} max={last} step={1} value={frame} onChange={(event) => go(Number(event.target.value))} />
      <div className="row">
        <button type="button" className="iconb" aria-label="A frame back" title="A frame back" disabled={frame <= 0} onClick={() => go(frame - 1)}>
          ‹
        </button>
        <span className="tc-inline" aria-label="Where it is">
          {timecode(frame, fps)} / {timecode(last, fps)}
        </span>
        <button type="button" className="iconb" aria-label="A frame on" title="A frame on" disabled={frame >= last} onClick={() => go(frame + 1)}>
          ›
        </button>
        <span className="sp" />
        {picks && (
          <button type="button" className="tb pick-up" onClick={() => void store.beginVideoPickup(name, frame / fps)}>
            Pick up from here
          </button>
        )}
      </div>
    </div>
  )
}
