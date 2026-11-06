import { type KeyboardEvent, type PointerEvent, useRef, useState } from "react"
import { Thumb } from "./Media"
import { timecode } from "./ranges"

const STRIP = 6

/** A video's frames as a strip, with its in and out dragged on it, a frame at a time. What's let go is written; the arrows move a frame. */
export function ClipRange({ sha256, frames, fps, from, to, width, label, onSet }: { sha256: string; frames: number; fps: number; from: number; to: number; width: number; label: string; onSet(from: number, to: number): void }) {
  const bar = useRef<HTMLDivElement>(null)
  const [held, setHeld] = useState<{ from: number; to: number } | null>(null)
  const now = held ?? { from, to }
  const frameAt = (x: number) => {
    const box = bar.current!.getBoundingClientRect()
    return Math.round(Math.max(0, Math.min(1, (x - box.left) / box.width)) * frames)
  }
  const moved = (which: "from" | "to", frame: number) => {
    const next = which === "from" ? { ...now, from: Math.min(frame, now.to - 1) } : { ...now, to: Math.max(frame, now.from + 1) }
    return { from: Math.max(0, next.from), to: Math.min(frames, next.to) }
  }
  const grab = (which: "from" | "to") => ({
    onPointerDown(event: PointerEvent<HTMLElement>) {
      event.stopPropagation()
      event.currentTarget.setPointerCapture(event.pointerId)
      setHeld(now)
    },
    onPointerMove(event: PointerEvent<HTMLElement>) {
      if (held) setHeld(moved(which, frameAt(event.clientX)))
    },
    onPointerUp() {
      if (held && (held.from !== from || held.to !== to)) onSet(held.from, held.to)
      setHeld(null)
    },
    onKeyDown(event: KeyboardEvent) {
      const step = event.key === "ArrowLeft" ? -1 : event.key === "ArrowRight" ? 1 : 0
      if (!step) return
      event.preventDefault()
      const next = moved(which, (which === "from" ? now.from : now.to) + step)
      onSet(next.from, next.to)
    },
  })
  const at = (frame: number) => `${(frame / frames) * 100}%`
  return (
    <div className="clip-range nodrag nopan" ref={bar} role="group" aria-label={label}>
      <Thumb sha256={sha256} cardWidth={width} strip={STRIP} className="clip-strip" />
      <span className="clip-out" style={{ left: 0, width: at(now.from) }} />
      <span className="clip-out" style={{ right: 0, width: `${((frames - now.to) / frames) * 100}%` }} />
      <span className="clip-handle" role="slider" tabIndex={0} aria-label={`${label}: in`} aria-valuemin={0} aria-valuemax={frames} aria-valuenow={now.from} aria-valuetext={timecode(now.from, fps)} style={{ left: at(now.from) }} {...grab("from")} />
      <span className="clip-handle out" role="slider" tabIndex={0} aria-label={`${label}: out`} aria-valuemin={0} aria-valuemax={frames} aria-valuenow={now.to} aria-valuetext={timecode(now.to, fps)} style={{ left: at(now.to) }} {...grab("to")} />
      <span className="clip-tc">
        {timecode(now.from, fps)}–{timecode(now.to, fps)}
      </span>
    </div>
  )
}
