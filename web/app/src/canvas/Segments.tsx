import { useQuery } from "@tanstack/react-query"
import { type CSSProperties, type PointerEvent as ReactPointerEvent, useCallback, useRef, useState } from "react"
import type { Pickup } from "../api/calcifer"
import { machinesQuery, runResultQuery } from "../api/queries"
import { useConnection } from "../app/Connection"
import { useSignal } from "../ui/signal"
import type { ElementView } from "./CanvasStore"
import { useCanvasStore } from "./context"
import { WalkedFrames } from "./Media"
import { orbitOf } from "./Orbit"
import { cutClock, FPS, framesIn, pickupAt, segmentAt, segmentsOf, segmentWords } from "./ranges"
import { makes, type Shown, shownOf } from "./shown"
import { rowsIn } from "./text"
import { Playhead, Ruler, Transport } from "./timeline/Player"

/** Where each of a take's segments after the first starts, in frames: `seams` in the result of the run that made it (se-130). None
 *  for a take its tool didn't say them of. */
export function useSeams(run: number | undefined): number[] | null {
  const { client } = useConnection()
  const { data: result } = useQuery({ ...runResultQuery(client, run ?? 0), enabled: Boolean(run) })
  const seams = result && typeof result === "object" && !Array.isArray(result) ? (result as Record<string, unknown>).seams : undefined
  return Array.isArray(seams) && seams.every((seam) => Number.isInteger(seam)) ? (seams as number[]) : null
}

/** The machines still held, by name: a take's latents on any other went with it. */
function useHeld(): Set<string> | null {
  const { client } = useConnection()
  const { data } = useQuery(machinesQuery(client))
  return data ? new Set(data.machines.filter((machine) => machine.state !== "ended" && machine.state !== "ending").map((machine) => machine.name)) : null
}

/** A take's mention without its brackets, as the store reads one. */
export const bare = (take: string) => take.replace(/^<|>$/g, "")

/** Under a video looked at in Focus: the one transport every video has (cl-965). */
export function VideoTransport({ name, view }: { name: string; view: ElementView }) {
  const store = useCanvasStore()
  const shown = shownOf(view)
  if (shown?.field !== "video" || !shown.duration_ms || orbitOf(shown)) return null
  const fps = shown.fps ?? FPS
  return <Transport playback={store.playbackOf(name, fps)} frames={framesIn(shown.duration_ms, fps)} fps={fps} />
}

/** Its track (Paul, 11-12; cl-965): the take's segments as spans on the time ruler every video has, the one the playhead is in marked,
 *  and Redo from there. Only whole segments are redone, each continuing from the frames before it, so every later one goes again too.
 *  Segment 1 offers no redo: that's a new take. A take its tool said no seams of is one span. Above it, what the take went on from. */
export function Segments({ name, view }: { name: string; view: ElementView }) {
  const store = useCanvasStore()
  const tools = useSignal(store.tools)
  const shown = shownOf(view)
  const seams = useSeams(shown?.run)
  const fps = shown?.fps ?? FPS
  const playback = store.playbackOf(name, fps)
  const frame = useSignal(playback.frame)
  const [room, setRoom] = useState(0)
  // Its width, measured whenever it's drawn: the track fits it.
  const watching = useRef<ResizeObserver | null>(null)
  const scroller = useCallback((held: HTMLDivElement | null) => {
    watching.current?.disconnect()
    watching.current = null
    if (!held) return
    const watch = new ResizeObserver(() => setRoom(held.clientWidth))
    watch.observe(held)
    watching.current = watch
    setRoom(held.clientWidth)
  }, [])
  if (!makes(view) && !view.asset) return null
  if (shown?.field !== "video" || shown.number === undefined || orbitOf(shown)) return null
  const pickup = tools.get(view.asset?.tool ?? "")?.pickup
  // What this take went on from: another box's take, or its own from a seam.
  const made: Pickup | null | undefined = view.asset?.prefixes?.find((version) => version.number === shown.number)?.pickup
  const of = made ? store.mentionOf(bare(made.take), name) : null
  const own = of?.name === store.resolveName(name)
  const last = shown.duration_ms ? framesIn(shown.duration_ms, fps) : 0
  const blocks = last ? segmentsOf(pickup && seams ? seams : [], last) : []
  const at = blocks.length ? Math.min(blocks.length - 1, segmentAt(frame, pickup && seams ? seams : [])) : 0
  const scale = last && room ? room / last : 0
  const scrub = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!scale || event.button !== 0 || (event.target as Element).closest("button")) return
    const box = event.currentTarget.getBoundingClientRect()
    playback.seek(Math.round(Math.max(0, Math.min(last - 1, (event.clientX - box.left) / scale))))
  }
  return (
    <div className="tl tl-take nodrag nopan nowheel" aria-label="Timeline">
      <div className="tl-bar" role="toolbar" aria-label="Its segments">
        {made && of && (
          <span className="continues" title={made.take}>
            {own ? `Redone from ${cutClock(made.out_frame / fps)}` : `Continues <${of.name}.${of.number}> from ${cutClock(made.out_frame / fps)}`}
          </span>
        )}
        <span className="sp" />
        {pickup && seams && at > 0 && (
          <button
            type="button"
            className="tb redo-segment"
            title={`Pick up from ${cutClock(blocks[at][0] / fps)}: keep ${segmentWords(1, at)} as ${at > 1 ? "they are" : "it is"} and render the rest again`}
            onClick={() => void store.beginVideoPickup(name, blocks[at][0])}
          >
            Redo {segmentWords(at + 1, blocks.length)}
          </button>
        )}
      </div>
      <div ref={scroller} className="tl-scroll">
        <div className="tl-track" style={{ width: room }}>
          <div className="tl-rail" onPointerDown={scrub}>
            {scale > 0 && <Ruler frames={last} scale={scale} width={room} fps={fps} from={0} to={room} />}
          </div>
          <div className="tl-clips tl-spans" onPointerDown={scrub}>
            {scale > 0 &&
              blocks.map(([start, end], i) => (
                <button
                  key={start}
                  type="button"
                  className={`tl-clip tl-span segment ${i === at ? "on here" : ""}`}
                  style={{ width: (end - start) * scale, transform: `translateX(${start * scale}px)` }}
                  aria-pressed={i === at}
                  aria-label={`Segment ${i + 1}, ${cutClock(start / fps)} to ${cutClock(end / fps)}`}
                  title={`Segment ${i + 1}: ${cutClock(start / fps)} to ${cutClock(end / fps)}`}
                  onClick={() => playback.seek(start)}
                >
                  {blocks.length > 1 ? i + 1 : ""}
                </button>
              ))}
          </div>
          {scale > 0 && <Playhead playback={playback} scale={scale} />}
        </div>
      </div>
    </div>
  )
}

/** In the frame of a draft that goes on from a take (cl-951): its kept part's last frame, what's kept and what's made again, the two
 *  on one line, and Cancel, back to a plain draft. */
export function LeadIn({ name, view }: { name: string; view: ElementView }) {
  const store = useCanvasStore()
  const tools = useSignal(store.tools)
  const held = useHeld()
  const going = view.continuing
  const said = going ? store.mentionOf(bare(going.take), name) : null
  const source = said ? store.get(said.name) : undefined
  const version = source?.asset?.prefixes?.find((held) => held.number === said?.number)
  const seams = useSeams(version?.run ?? undefined)
  const file: Shown | null = going ? store.fileOf(bare(going.take), name) : null
  if (!going || !said || !file?.duration_ms) return null
  const fps = view.continuingFps ?? file.fps ?? FPS
  const last = framesIn(file.duration_ms, fps)
  const own = said.name === store.resolveName(name)
  const take = `<${said.name}.${said.number}>`
  const end = pickupAt(last, last, tools.get(view.asset?.tool ?? "")?.pickup ?? undefined) ?? last
  const blocks = seams ? segmentsOf(seams, last) : null
  const at = blocks ? segmentAt(going.out_frame, seams!) : 0
  const words = own
    ? blocks && at > 0
      ? `Keeps ${segmentWords(1, at)} of ${take} · redoes ${at + 1 === blocks.length ? at + 1 : `${at + 1}–${blocks.length}`}`
      : `Redoes ${take} from ${cutClock(going.out_frame / fps)}`
    : going.out_frame >= end ? `Continues ${take} from its end · ${cutClock(going.out_frame / fps)}` : `Continues ${take} from ${cutClock(going.out_frame / fps)}`
  // The new part runs from the beats' clock at the cut to their end.
  const beats = tools.get(view.asset?.tool ?? "")?.fields.find((field) => field.type === "group" && field.list && field.members?.some((member) => member.total))
  const timing = beats?.members?.find((member) => member.total)
  const total = beats && timing ? rowsIn(store.valueOf(name, beats.field)).reduce((sum, row) => sum + (Number(row[timing.field] ?? timing.default) || 0), 0) : 0
  const gone = version?.latents && held && !held.has(version.latents) ? version.latents : null
  const keptSeconds = going.out_frame / fps
  const newSeconds = Math.max(0, total - (going.beats_from ?? keptSeconds))
  return (
    <div className="lead-in nodrag nopan" role="group" aria-label="Continues a video">
      <span className="lead-in-frame" style={{ aspectRatio: file.width && file.height ? file.width / file.height : 16 / 9 }}>
        <WalkedFrames sha256={file.sha256} cardWidth={160} at={Math.min(1, going.out_frame / last)} />
      </span>
      <span className="lead-in-words">
        <b>{words}</b>
        <span className="lead-in-line" style={{ "--kept": keptSeconds, "--new": newSeconds || 1 } as CSSProperties} aria-hidden>
          <span className="kept" />
          <span className="new" />
        </span>
      </span>
      <button type="button" className="tb nodrag" onClick={() => store.stopContinuing(name)}>
        Cancel
      </button>
      {gone && <span className="lead-in-gone">Its latents went with {gone}: the join may show</span>}
    </div>
  )
}

/** On a draft that can go on from a video: pick one of the other boxes' takes to continue from its end (cl-951). A take let go on the
 *  box does the same. */
export function ContinueFrom({ name, view }: { name: string; view: ElementView }) {
  const store = useCanvasStore()
  const tools = useSignal(store.tools)
  useSignal(store.names)
  if (view.continuing || !tools.get(view.asset?.tool ?? "")?.pickup) return null
  const takes = store.videoTakes(name)
  if (!takes.length) return null
  return (
    <label className="slide continue-from">
      <span className="ctl-name">Continue</span>
      <select
        className="field nodrag"
        aria-label="Continue a video from its end"
        value=""
        onChange={(event) => {
          const [from, number] = event.target.value.split("\n")
          if (from && number) store.continueFrom(name, from, Number(number))
        }}
      >
        <option value="">a video from its end…</option>
        {takes.map((take) => (
          <option key={`${take.name}.${take.number}`} value={`${take.name}\n${take.number}`}>
            {`<${take.name}.${take.number}>`}
          </option>
        ))}
      </select>
    </label>
  )
}
