// One player for every video in Focus (cl-965): the transport under it (play, where it is of how long, a scrubber), and a track on a time
// ruler with a playhead. An Edit's track holds its clips (VideoEditor); a take's, its segments (Segments).
import type { CSSProperties } from "react"
import { Icon } from "../../ui/Icon"
import { type Signal, signal, useSignal } from "../../ui/signal"
import { marks, rulerLabel, timecode } from "./model"
import "./timeline.css"

/** What a transport drives: a cut played clip after clip (Preview), or one video (VideoPlayback). */
export interface Playback {
  readonly frame: Signal<number>
  readonly playing: Signal<boolean>
  toggle(): void
  seek(frame: number): void
}

/** One `<video>` as a Playback, its frame read each animation frame while it plays. Before it's open, play or a seek asks it open (`start`),
 *  and the frame wanted is shown once it is. */
export class VideoPlayback implements Playback {
  readonly frame = signal(0)
  readonly playing = signal(false)
  /** Opens the player when there's nothing to drive yet. */
  start: () => void = () => {}
  private video: HTMLVideoElement | null = null
  private wanted: number | null = null
  private raf = 0
  private off: (() => void) | null = null

  constructor(public fps: number) {}

  /** Drives `video` from now; true when a seek was waiting for it, which it now shows instead of playing. */
  attach(video: HTMLVideoElement | null): boolean {
    this.off?.()
    this.off = null
    this.video = video
    cancelAnimationFrame(this.raf)
    if (!video) {
      this.playing.set(false)
      return false
    }
    const read = () => this.frame.set(Math.round(video.currentTime * this.fps))
    const tick = () => {
      read()
      this.raf = requestAnimationFrame(tick)
    }
    const played = () => {
      this.playing.set(true)
      cancelAnimationFrame(this.raf)
      this.raf = requestAnimationFrame(tick)
    }
    const stopped = () => {
      this.playing.set(false)
      cancelAnimationFrame(this.raf)
      read()
    }
    const events: Array<[string, () => void]> = [["play", played], ["pause", stopped], ["ended", stopped], ["seeked", read], ["loadedmetadata", read]]
    for (const [name, heard] of events) video.addEventListener(name, heard)
    this.off = () => {
      for (const [name, heard] of events) video.removeEventListener(name, heard)
    }
    const held = this.wanted
    this.wanted = null
    if (held !== null) {
      video.currentTime = held / this.fps
      return true
    }
    if (!video.paused) played()
    return false
  }

  toggle() {
    const video = this.video
    if (!video) return this.start()
    if (video.paused) void video.play().catch(() => {})
    else video.pause()
  }

  seek(frame: number) {
    const at = Math.max(0, frame)
    this.frame.set(at)
    if (!this.video) {
      this.wanted = at
      return this.start()
    }
    this.video.pause()
    this.video.currentTime = at / this.fps
  }
}

/** Under the player, as under any video: play, where it is of how long, and a scrubber across the whole of it. */
export function Transport({ playback, frames, fps }: { playback: Playback; frames: number; fps: number }) {
  const frame = useSignal(playback.frame)
  const playing = useSignal(playback.playing)
  const last = Math.max(0, frames - 1)
  return (
    <div className="tl-transport nodrag nopan">
      <button type="button" className="tb tl-go" aria-label={playing ? "Pause" : "Play"} title={playing ? "Pause (Space)" : "Play (Space)"} disabled={!frames} onClick={() => playback.toggle()}>
        <Icon name={playing ? "pause" : "play"} />
      </button>
      <span className="tl-time" aria-label="Playhead">
        {timecode(Math.min(frame, frames), fps)} <span className="tl-of">/ {timecode(frames, fps)}</span>
      </span>
      <input className="tl-scrub" type="range" aria-label="Scrub" min={0} max={last} step={1} value={Math.min(frame, last)} disabled={!frames} onChange={(event) => playback.seek(Number(event.target.value))} />
    </div>
  )
}

/** Where the playhead is on a track drawn `scale` pixels a frame. */
export function Playhead({ playback, scale }: { playback: Playback; scale: number }) {
  const frame = useSignal(playback.frame)
  return <div className="tl-playhead" data-frame={frame} style={{ transform: `translateX(${frame * scale}px)` }} />
}

/** Time along the top, labelled where the labels have room; only what's in view (`from` to `to`, in pixels) is drawn. */
export function Ruler({ frames, scale, width, fps, from, to }: { frames: number; scale: number; width: number; fps: number; from: number; to: number }) {
  const { major, minor } = marks(scale, fps)
  const first = Math.max(0, Math.floor(from / scale / major) * major)
  const last = Math.min(frames, Math.ceil(to / scale / major) * major)
  const labels: number[] = []
  for (let frame = first; frame <= last; frame += major) labels.push(frame)
  return (
    <div className="tl-ruler" style={{ "--minor": `${minor * scale}px`, "--major": `${major * scale}px`, width } as CSSProperties}>
      {labels.map((frame) => (
        <span key={frame} className="tl-mark" style={{ transform: `translateX(${frame * scale}px)` }}>
          {rulerLabel(frame, fps)}
        </span>
      ))}
    </div>
  )
}
