import { type KeyboardEvent, type PointerEvent, useEffect, useRef, useState } from "react"
import { holdMedia } from "../api/events"
import type { Playback } from "../api/transport"
import { useConnection } from "../app/Connection"
import type { Shown } from "./shown"

/** What a file's `orbit` metadata says (design/file-metadata.md): how far one pass turns, which way, and whether its last frame meets its first. */
export interface Orbit {
  degrees: number
  direction: "cw" | "ccw"
  loops: boolean
}

/** What turning orbit viewing on gives any video: one whole turn, clockwise, looping. */
export const ORBIT: Orbit = { degrees: 360, direction: "cw", loops: true }

/** A video's `orbit`, read as found: what's missing or malformed takes the whole turn's. None: it plays. */
export function orbitOf(shown: { metadata?: Record<string, unknown> } | null | undefined): Orbit | null {
  const said = shown?.metadata?.orbit
  if (!said || typeof said !== "object") return null
  const held = said as Record<string, unknown>
  const degrees = typeof held.degrees === "number" && held.degrees > 0 ? held.degrees : ORBIT.degrees
  return { degrees, direction: held.direction === "ccw" ? "ccw" : "cw", loops: held.loops !== false }
}

/** The frame an angle shows: `floor(angle / degrees × frames) mod frames`, held at its ends when it doesn't loop. */
export function frameAt(angle: number, orbit: Orbit, frames: number): number {
  const at = Math.floor((angle / orbit.degrees) * frames)
  return orbit.loops ? ((at % frames) + frames) % frames : Math.min(frames - 1, Math.max(0, at))
}

/** A drag one stage wide turns a whole circle, as the demo the owner approved does; an arrow key 5°. */
const PER_WIDTH = 360
const STEP = 5
const R = 38

/** The angle as it's shown: within one pass when it loops, held at its ends when it doesn't. */
const within = (angle: number, orbit: Orbit) => (orbit.loops ? ((angle % orbit.degrees) + orbit.degrees) % orbit.degrees : Math.max(0, Math.min(orbit.degrees, angle)))

/** Where on the dial an angle is: 0° at the top, clockwise. */
const onDial = (angle: number, radius = R) => {
  const rad = ((angle - 90) * Math.PI) / 180
  return { x: 50 + radius * Math.cos(rad), y: 50 + radius * Math.sin(rad) }
}

function Dial({ angle }: { angle: number }) {
  const dot = onDial(angle)
  const arc = angle < 0.5 ? "" : `M 50 12 A ${R} ${R} 0 ${angle % 360 > 180 ? 1 : 0} 1 ${dot.x} ${dot.y}`
  const ticks = Array.from({ length: 24 }, (_, i) => i * 15)
  return (
    <svg className="orbit-dial" viewBox="0 0 100 100" aria-hidden="true">
      <circle cx={50} cy={50} r={R} className="ring" />
      {ticks.map((tick) => {
        const from = onDial(tick, tick % 90 ? 34 : 31)
        const to = onDial(tick)
        return <line key={tick} x1={from.x} y1={from.y} x2={to.x} y2={to.y} className={tick % 90 ? "tick" : "tick major"} />
      })}
      {arc && <path d={arc} className="arc" />}
      <circle cx={dot.x} cy={dot.y} r={4.5} className="dot" />
    </svg>
  )
}

/**
 * A video that turns, not plays (Paul, 11-11): a drag across it turns it, wrapping at 0/360 when it loops; arrows step it; Spin plays it
 * round at its own speed. Every frame of an orbit is a keyframe, so a seek shows at once; one seek is in flight at a time, so a drag never
 * queues a backlog. The demo viewer in outputs/orb360-demo-20261111 is its reference.
 */
export function OrbitView({ path, shown, orbit }: { path: string; shown: Shown; orbit: Orbit }) {
  const { client } = useConnection()
  const video = useRef<HTMLVideoElement>(null)
  const [angle, setAngle] = useState(0)
  const [ready, setReady] = useState(false)
  const [spinning, setSpinning] = useState(false)
  const [used, setUsed] = useState(false)
  const at = useRef(0)
  const seek = useRef<{ pending: number | null; busy: boolean }>({ pending: null, busy: false })
  const drag = useRef<{ x: number; from: number; last: number; t: number; v: number } | null>(null)
  const frameId = useRef(0)
  const fps = shown.fps ?? 24
  const frames = Math.max(1, shown.frames ?? (Math.round(((shown.duration_ms ?? 0) / 1000) * fps) || 1))
  const sign = orbit.direction === "ccw" ? -1 : 1
  const shownAngle = within(angle, orbit)
  const frame = frameAt(shownAngle, orbit, frames)

  // One of the two live videos across tabs, held while the box is open.
  useEffect(() => {
    const waiting = new AbortController()
    let release: (() => void) | null = null
    let playback: Playback | null = null
    holdMedia(waiting.signal)
      .then((held) => {
        release = held
        const el = video.current
        if (!el || waiting.signal.aborted) return held()
        playback = client.transport.play(path, el)
      })
      .catch(() => {})
    return () => {
      waiting.abort()
      cancelAnimationFrame(frameId.current)
      playback?.stop()
      release?.()
    }
  }, [client, path])

  const flush = () => {
    const el = video.current
    const held = seek.current
    if (!el || held.pending === null || !el.duration) return
    held.busy = true
    el.currentTime = Math.min(held.pending, el.duration - 0.001)
    held.pending = null
  }
  const turn = (to: number) => {
    const next = orbit.loops ? to : Math.max(0, Math.min(orbit.degrees, to))
    at.current = next
    setAngle(next)
    seek.current.pending = (frameAt(within(next, orbit), orbit, frames) + 0.5) / fps
    if (!seek.current.busy) flush()
  }
  const stopSpin = () => {
    const el = video.current
    if (el && spinning) {
      el.pause()
      el.loop = false
    }
    setSpinning(false)
  }
  const spin = () => {
    const el = video.current
    if (!el) return
    if (spinning) return stopSpin()
    setUsed(true)
    setSpinning(true)
    el.loop = orbit.loops
    el.play().catch(() => setSpinning(false))
    const track = () => {
      if (el.paused) return setSpinning(false)
      const now = el.duration ? (el.currentTime / el.duration) * orbit.degrees : 0
      at.current = now
      setAngle(now)
      frameId.current = requestAnimationFrame(track)
    }
    frameId.current = requestAnimationFrame(track)
  }

  const down = (event: PointerEvent<HTMLDivElement>) => {
    if ((event.target as HTMLElement).closest("button")) return
    stopSpin()
    cancelAnimationFrame(frameId.current)
    setUsed(true)
    drag.current = { x: event.clientX, from: at.current, last: event.clientX, t: performance.now(), v: 0 }
    event.currentTarget.setPointerCapture(event.pointerId)
  }
  const move = (event: PointerEvent<HTMLDivElement>) => {
    const held = drag.current
    if (!held) return
    const width = event.currentTarget.clientWidth || 1
    const now = performance.now()
    turn(held.from + ((sign * (event.clientX - held.x)) / width) * PER_WIDTH)
    held.v = (((sign * (event.clientX - held.last)) / width) * PER_WIDTH) / Math.max(1, now - held.t)
    held.last = event.clientX
    held.t = now
  }
  // Let go moving, it coasts to a stop, unless motion is to be kept down.
  const up = () => {
    const held = drag.current
    drag.current = null
    if (!held || matchMedia("(prefers-reduced-motion: reduce)").matches) return
    let v = held.v
    const glide = () => {
      if (Math.abs(v) < 0.004) return
      turn(at.current + v * 16)
      v *= 0.92
      frameId.current = requestAnimationFrame(glide)
    }
    frameId.current = requestAnimationFrame(glide)
  }
  const key = (event: KeyboardEvent<HTMLDivElement>) => {
    const by = event.key === "ArrowRight" ? STEP : event.key === "ArrowLeft" ? -STEP : 0
    if (!by) return
    event.preventDefault()
    stopSpin()
    setUsed(true)
    turn(at.current + sign * by)
  }

  return (
    <div
      className={`orbit nodrag nopan nowheel ${ready ? "ready" : ""}`}
      role="slider"
      tabIndex={0}
      aria-label="Turn"
      aria-valuemin={0}
      aria-valuemax={orbit.degrees}
      aria-valuenow={Math.round(shownAngle)}
      aria-valuetext={`${Math.round(shownAngle)}°, frame ${frame + 1} of ${frames}`}
      data-angle={shownAngle.toFixed(2)}
      data-frame={frame}
      data-frames={frames}
      onPointerDown={down}
      onPointerMove={move}
      onPointerUp={up}
      onPointerCancel={up}
      onKeyDown={key}
    >
      <video
        ref={video}
        className="orbit-video"
        muted
        playsInline
        preload="auto"
        onLoadedData={() => {
          setReady(true)
          turn(at.current)
        }}
        onSeeked={() => {
          seek.current.busy = false
          flush()
        }}
        onEnded={() => setSpinning(false)}
      />
      {!used && <span className="orbit-hint">Drag to turn</span>}
      <div className="orbit-bar">
        <Dial angle={shownAngle} />
        <span className="orbit-deg">{Math.round(shownAngle) % 360}°</span>
        <button type="button" className="orbit-act nodrag" aria-pressed={spinning} onClick={spin}>
          {spinning ? "Stop" : "Spin"}
        </button>
        <button
          type="button"
          className="orbit-act nodrag"
          onClick={() => {
            stopSpin()
            turn(0)
          }}
        >
          Front
        </button>
      </div>
    </div>
  )
}
