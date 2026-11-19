// A video that grows as its render goes (cl-1210; Paul, 11-18: "we could already see segments 1 and 2, but then they suddenly
// disappear"). Each newer revision loads behind the one shown, at its playhead, and takes its place once it has its frame there, playing
// on if it was: nothing blanks between revisions, and the take that lands takes over the same way (`handoff`).
import { type MouseEvent, useEffect, useLayoutEffect, useRef, useState } from "react"

/** How far ahead of a playing one its successor opens, so it's ready as the playing one gets there and the clock never steps back. */
const LEAD_S = 0.4

function heard(el: HTMLMediaElement, events: string[], signal?: AbortSignal): Promise<void> {
  return new Promise((done, failed) => {
    const off = () => {
      for (const name of events) el.removeEventListener(name, on)
      signal?.removeEventListener("abort", aborted)
    }
    const on = () => {
      off()
      done()
    }
    const aborted = () => {
      off()
      failed(signal?.reason)
    }
    for (const name of events) el.addEventListener(name, on)
    signal?.addEventListener("abort", aborted)
  })
}

/** `el` with its frame drawn at the time it was sent to. */
async function shows(el: HTMLMediaElement, signal?: AbortSignal) {
  while (el.seeking || el.readyState < HTMLMediaElement.HAVE_CURRENT_DATA) await heard(el, ["seeked", "loadeddata", "canplay"], signal)
}

const frame = () => new Promise((done) => requestAnimationFrame(done))

/** Marks `el` let go by the one that showed it, before it is paused and emptied: its pause is nobody's, and its clock reads 0 from here on. */
export function letGo(el: HTMLMediaElement) {
  el.dataset.gone = ""
  el.pause()
  el.removeAttribute("src")
  el.load()
}

/** Whether `el` was let go (`letGo`): nothing of it is a playhead any more. */
export const wasLetGo = (el: HTMLMediaElement) => el.dataset.gone !== undefined

/**
 * `to` takes over from `from`: opened where `from` is (a moment ahead while it plays), it waits with its frame drawn until `from` gets
 * there, then plays on if `from` was playing, so the clock runs on with no step back and no frame missing. True once `to` is the one to
 * show, `from` left paused for its owner to drop. False when `from` was let go meanwhile (its owner put another in its place): `to` is
 * left as it is, for its caller to hand it what took `from`'s place. Two takeovers can share a `from`, a newer revision's and its take's.
 */
export async function handoff(from: HTMLMediaElement, to: HTMLMediaElement, signal?: AbortSignal): Promise<boolean> {
  if (to.readyState < HTMLMediaElement.HAVE_METADATA) await heard(to, ["loadedmetadata"], signal)
  const end = Number.isFinite(to.duration) ? to.duration : Infinity
  for (let tries = 0; tries < 4; tries++) {
    if (wasLetGo(from)) return false
    const playing = !from.paused && !from.ended
    const at = Math.min(from.currentTime + (playing ? LEAD_S : 0), end)
    to.currentTime = at
    await shows(to, signal)
    if (wasLetGo(from)) return false
    if (!playing) {
      if (from.paused && Math.abs(from.currentTime - at) < 0.02) break
      continue // it moved meanwhile: open where it is now
    }
    // Played up to where `to` waits; one that loops and came round, or was paused or sought meanwhile, opens again.
    while (!wasLetGo(from) && !from.paused && from.currentTime < at && from.currentTime > at - LEAD_S - 0.5) await frame()
    signal?.throwIfAborted()
    if (wasLetGo(from)) return false
    if (!from.paused && from.currentTime >= at && from.currentTime < at + 0.25) {
      void to.play().catch(() => {})
      break
    }
  }
  from.pause()
  return true
}

/**
 * One video, its newest source shown. A new `path`'s bytes are held first (`media`, the page's transport: cl-1215), then it loads behind
 * the shown one and takes over (`handoff`) once it has its frame at the playhead: the one shown stays until then, however long the bytes
 * take. `onShown` hears each element as it becomes the one shown (null when the video goes), `onTime` the shown one's clock.
 */
export function GrowingVideo({ path, media, className, videoClassName, muted, loop, autoPlay, controls, onShown, onTime, onClick }: {
  path?: string
  media(path: string): Promise<{ url: string; release(): void }>
  /** The holder's, and the shown element's own. */
  className?: string
  videoClassName?: string
  muted: boolean
  loop: boolean
  autoPlay: boolean
  controls?: boolean
  onShown?(el: HTMLVideoElement | null): void
  onTime?(el: HTMLVideoElement): void
  onClick?(event: MouseEvent<HTMLVideoElement>): void
}) {
  const [layers, setLayers] = useState<{ id: number; path: string; src: string }[]>([])
  const [shownId, setShownId] = useState<number | null>(null)
  const shown = useRef<number | null>(null)
  // Each layer's element as last mounted, and the hold on its bytes. Both go when its layer does (`drop`), never as its ref detaches:
  // React's development build detaches and attaches every ref once more as it mounts (StrictMode).
  const elements = useRef(new Map<number, HTMLVideoElement>())
  const holds = useRef(new Map<number, () => void>())
  // One ref callback a layer, kept: an inline one is called again at every render.
  const refs = useRef(new Map<number, (el: HTMLVideoElement | null) => void>())
  const refOf = (id: number) => {
    let ref = refs.current.get(id)
    if (!ref) {
      ref = (el) => {
        if (el) elements.current.set(id, el)
      }
      refs.current.set(id, ref)
    }
    return ref
  }
  // A layer gone lets go at once: its element's source, then its bytes.
  const drop = (id: number) => {
    const el = elements.current.get(id)
    if (el) letGo(el)
    elements.current.delete(id)
    refs.current.delete(id)
    holds.current.get(id)?.()
    holds.current.delete(id)
  }
  const next = useRef(0)
  const told = useRef(onShown)
  told.current = onShown
  useEffect(() => {
    if (!path) return
    let live = true
    void media(path).then((held) => {
      if (!live) return held.release()
      const id = next.current++
      holds.current.set(id, held.release)
      // Behind the one shown; one still on its way to being shown gives way to this newer one.
      setLayers((now) => [...now.filter((layer) => layer.id === shown.current), { id, path, src: held.url }])
    }, () => {})
    return () => {
      live = false
    }
  }, [path, media])
  useLayoutEffect(() => {
    for (const id of [...holds.current.keys()]) if (!layers.some((layer) => layer.id === id)) drop(id)
  }, [layers])
  // Gone for good, every layer lets go. A development build's rehearsal of it mounts again at once, and nothing is let go.
  const mounted = useRef(false)
  useLayoutEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
      queueMicrotask(() => {
        if (mounted.current) return
        for (const id of [...holds.current.keys()]) drop(id)
        told.current?.(null)
      })
    }
  }, [])
  const incoming = layers.at(-1)
  useEffect(() => {
    if (!incoming || incoming.id === shownId) return
    const el = elements.current.get(incoming.id)
    if (!el) return
    const before = shownId === null ? undefined : elements.current.get(shownId)
    const taking = new AbortController()
    void (async () => {
      if (before) await handoff(before, el, taking.signal)
      else {
        await shows(el, taking.signal)
        if (autoPlay) void el.play().catch(() => {})
      }
      shown.current = incoming.id
      setShownId(incoming.id)
      setLayers((now) => now.filter((layer) => layer.id >= incoming.id))
      told.current?.(el)
    })().catch(() => {})
    return () => taking.abort()
  }, [incoming?.id, shownId])
  return (
    <div className={`growing-video ${className ?? ""}`}>
      {layers.map((layer) => {
        const front = layer.id === shownId
        return (
          <video
            key={layer.id}
            ref={refOf(layer.id)}
            src={layer.src}
            className={front ? `shown ${videoClassName ?? ""}` : "incoming"}
            muted={muted}
            loop={loop}
            controls={Boolean(controls) && front}
            playsInline
            preload="auto"
            onClick={onClick}
            onTimeUpdate={front ? (event) => onTime?.(event.currentTarget) : undefined}
            onPause={front ? (event) => onTime?.(event.currentTarget) : undefined}
            onSeeked={front ? (event) => onTime?.(event.currentTarget) : undefined}
          />
        )
      })}
    </div>
  )
}
