import { type CSSProperties, type DragEvent, type KeyboardEvent, type PointerEvent as ReactPointerEvent, memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react"
import { useConnection } from "../../app/Connection"
import type { ClipNow } from "../../api/calcifer"
import { Icon } from "../../ui/Icon"
import { say } from "../../ui/notes"
import { useSignal } from "../../ui/signal"
import type { DropTarget } from "../CanvasStore"
import { LET_GO, type LetGo } from "../carry"
import { useCanvasStore, useElement } from "../context"
import { FocusFrame } from "../FocusFrame"
import { Thumb } from "../Media"
import { valuesOf } from "../shown"
import { clipOf, frameAt, indexAt, isStill, lay, type Layout, marks, moved, named, type Placed, rulerLabel, split, timecode, trimmedIn, trimmedOut, wire, xOf } from "./model"
import { Preview } from "./Preview"
import "./timeline.css"

const NONE: ClipNow[] = []
/** Room past the last clip, to drop after it. */
const ROOM_AFTER = 48
const PICTURE_HEIGHT = 66
const STRIP_FRAMES = [1, 2, 4, 8, 16, 32]
/** The clips part this far for what's being dragged in. */
const GAP = 64
const MOST_ZOOM = 64
const POSTER_FRAMES = 16
/** A cut changed this long ago, and nothing since, is joined into its next version. */
const JOIN_MS = 1200

/** Follows a pointer, wherever it goes, until it lets go. */
function follow(event: ReactPointerEvent, moving: (event: PointerEvent) => void, ended: () => void) {
  event.preventDefault()
  event.stopPropagation()
  const pointer = event.pointerId
  // Off the timeline the pointer is still this gesture's.
  event.currentTarget.closest(".tl")?.setPointerCapture(pointer)
  const move = (next: PointerEvent) => {
    if (next.pointerId === pointer) moving(next)
  }
  const up = (next: PointerEvent) => {
    if (next.pointerId !== pointer) return
    window.removeEventListener("pointermove", move)
    window.removeEventListener("pointerup", up)
    window.removeEventListener("pointercancel", up)
    ended()
  }
  window.addEventListener("pointermove", move)
  window.addEventListener("pointerup", up)
  window.addEventListener("pointercancel", up)
}

/** Its frames side by side, each where it falls in the source: a trim only moves the window on them. */
function Strip({ place, scale }: { place: Placed; scale: number }) {
  const { clip } = place
  const sha256 = clip.sha256
  if (!sha256) return null
  const tile = PICTURE_HEIGHT * (clip.width && clip.height ? clip.width / clip.height : 16 / 9)
  if (isStill(clip)) {
    return (
      <div className="tl-tiles">
        {Array.from({ length: Math.min(32, Math.max(1, Math.ceil(place.w / tile))) }, (_, n) => (
          <div key={n} className="tl-tile" style={{ width: tile }}>
            <Thumb sha256={sha256} cardWidth={128} />
          </div>
        ))}
      </div>
    )
  }
  const whole = (clip.holds ?? place.frames) * scale
  const strip = STRIP_FRAMES.find((frames) => frames >= Math.round(whole / tile)) ?? 32
  return (
    <div className="tl-frames" style={{ left: -place.in * scale, width: whole }}>
      <Thumb sha256={sha256} cardWidth={128} strip={strip} exact />
    </div>
  )
}

/** Its sound, as a waveform of the whole source under the same window as its frames. A still has none. */
function Sound({ place, scale }: { place: Placed; scale: number }) {
  const { clip } = place
  if (!clip.sha256 || isStill(clip)) return null
  return (
    <div className="tl-wave" style={{ left: -place.in * scale, width: (clip.holds ?? place.frames) * scale }}>
      <Thumb sha256={clip.sha256} cardWidth={1024} wave exact />
    </div>
  )
}

const Clip = memo(function Clip({
  place,
  scale,
  x,
  fps,
  selected,
  lifted,
  onBody,
  onHandle,
}: {
  place: Placed
  scale: number
  x: number
  fps: number
  selected: boolean
  lifted: boolean
  onBody: (event: ReactPointerEvent, index: number) => void
  onHandle: (event: ReactPointerEvent, index: number, side: "in" | "out") => void
}) {
  const { clip, index } = place
  const name = clip.name ?? named(clip)
  const common = {
    "data-clip": index,
    "data-name": name,
    "data-in": place.in,
    "data-out": place.out,
    "data-frames": place.frames,
    onPointerDown: (event: ReactPointerEvent) => onBody(event, index),
  }
  // Offline it lasts nothing, as its film goes on without it: a flag where it stands, which still says why and can still be taken out.
  if (clip.missing) {
    return (
      <div className={`tl-clip offline ${selected ? "on" : ""}`} style={{ transform: `translateX(${x}px)` }} data-missing={clip.missing} role="img" aria-label={`${name}: media offline, ${clip.missing}`} title={`${name}: media offline, ${clip.missing}. The film goes on without it.`} {...common}>
        !
      </div>
    )
  }
  return (
    <div className={`tl-clip ${selected ? "on" : ""} ${lifted ? "lifted" : ""}`} style={{ transform: `translateX(${x}px)`, width: place.w }} title={`${name} · ${timecode(place.frames, fps)}`} {...common}>
      <div className="tl-picture">
        <Strip place={place} scale={scale} />
        <span className="tl-name">{name}</span>
      </div>
      <div className="tl-sound">
        <Sound place={place} scale={scale} />
      </div>
      <i className="tl-handle in" aria-label={`Trim the start of ${name}`} onPointerDown={(event) => onHandle(event, index, "in")} />
      <i className="tl-handle out" aria-label={`Trim the end of ${name}`} onPointerDown={(event) => onHandle(event, index, "out")} />
    </div>
  )
})

function Playhead({ preview, layout }: { preview: Preview; layout: Layout }) {
  const frame = useSignal(preview.frame)
  return <div className="tl-playhead" data-frame={frame} style={{ transform: `translateX(${xOf(layout, frame)}px)` }} />
}

/** Time along the top, labelled where the labels have room; only what's in view is drawn. */
function Ruler({ layout, fps, from, to }: { layout: Layout; fps: number; from: number; to: number }) {
  const { major, minor } = marks(layout.scale, fps)
  const first = Math.max(0, Math.floor(from / layout.scale / major) * major)
  const last = Math.min(layout.frames, Math.ceil(to / layout.scale / major) * major)
  const labels: number[] = []
  for (let frame = first; frame <= last; frame += major) labels.push(frame)
  return (
    <div className="tl-ruler" style={{ "--minor": `${minor * layout.scale}px`, "--major": `${major * layout.scale}px`, width: layout.width } as CSSProperties}>
      {labels.map((frame) => (
        <span key={frame} className="tl-mark" style={{ transform: `translateX(${frame * layout.scale}px)` }}>
          {rulerLabel(frame, fps)}
        </span>
      ))}
    </div>
  )
}

/** What's at the playhead, filling the frame center-cropped as the film is, exact once it's been sought. A click plays or pauses it. */
function Monitor({ preview, layout, empty }: { preview: Preview; layout: Layout; empty: boolean }) {
  const store = useCanvasStore()
  const holder = useRef<HTMLDivElement>(null)
  const still = useRef<HTMLCanvasElement>(null)
  const showing = useSignal(preview.showing)
  const live = useSignal(preview.live)
  const ready = useSignal(preview.ready)
  const kept = useSignal(preview.kept)
  const failed = useSignal(preview.failed)
  const frame = useSignal(preview.frame)
  useLayoutEffect(() => {
    preview.attach(holder.current, still.current)
    return () => preview.attach(null, null)
  }, [preview])
  const place = layout.placed[showing ?? -1]
  const clip = place?.clip
  const moving = clip && !isStill(clip) && clip.path
  const drawn = Boolean(kept && (!live || !ready))
  const videoReady = moving && live && ready
  const pictureReady = () => {
    const image = holder.current?.parentElement?.querySelector("img")
    if (place && clip?.sha256 && image) preview.picture(place.index, clip.sha256, image)
  }
  // Until it's sought, the nearest of a strip's frames: a trimmed clip doesn't open on its take's first.
  const tile = place && clip?.holds ? Math.min(POSTER_FRAMES - 1, Math.floor(((place.in + (frame - place.at)) / clip.holds) * POSTER_FRAMES)) : 0
  return (
    <div className="tl-monitor" data-clip={clip ? (clip.name ?? named(clip)) : undefined} data-live={live ? "" : undefined} onClick={() => !empty && preview.toggle()}>
      {clip?.sha256 && !clip.missing && (isStill(clip) || !(videoReady || drawn)) && store.projectId && (
        moving && tile > 0 ? <Thumb sha256={clip.sha256} cardWidth={512} strip={POSTER_FRAMES} exact frame={tile} className="tl-poster" /> : <Thumb sha256={clip.sha256} cardWidth={1024} onSize={pictureReady} />
      )}
      <canvas ref={still} className={drawn ? "" : "off"} />
      {/* One for the box, there only while it holds its slot: what plays is whatever it holds now. */}
      <div ref={holder} className={`tl-video ${videoReady ? "" : "off"}`} />
      {empty && <p className="tl-empty">Drop videos and pictures on the timeline</p>}
      {failed && <p className="tl-empty">{failed}</p>}
    </div>
  )
}

/** Under the player, as under any video: play, where it is of how long, and a scrubber across the whole cut. */
function Transport({ preview, layout, fps }: { preview: Preview; layout: Layout; fps: number }) {
  const frame = useSignal(preview.frame)
  const playing = useSignal(preview.playing)
  const last = Math.max(0, layout.frames - 1)
  return (
    <div className="tl-transport nodrag nopan">
      <button type="button" className="tb tl-go" aria-label={playing ? "Pause" : "Play"} title={playing ? "Pause (Space)" : "Play (Space)"} disabled={!layout.frames} onClick={() => preview.toggle()}>
        <Icon name={playing ? "pause" : "play"} />
      </button>
      <span className="tl-time" aria-label="Playhead">
        {timecode(frame, fps)} <span className="tl-of">/ {timecode(layout.frames, fps)}</span>
      </span>
      <input className="tl-scrub" type="range" aria-label="Scrub" min={0} max={last} step={1} value={Math.min(frame, last)} disabled={!layout.frames} onChange={(event) => preview.seek(Number(event.target.value))} />
    </div>
  )
}

/** Split, there when the playhead is inside a clip. */
function SplitButton({ preview, clips, fps, onSplit }: { preview: Preview; clips: ClipNow[]; fps: number; onSplit: () => void }) {
  const frame = useSignal(preview.frame)
  return (
    <button type="button" className="tb" aria-label="Split at playhead" title="Split at playhead (S)" disabled={!split(clips, fps, frame)} onClick={onSplit}>
      <Icon name="split" />
      Split
    </button>
  )
}

/** Writes the newest edit, one at a time: what's asked while one is going goes next. */
class Saver {
  etag: string | undefined
  private flying = false
  private next: Record<string, unknown> | null = null
  private write: (fields: Record<string, unknown>, etag: string | undefined) => Promise<{ etag: string; warnings?: string[] }>
  private settled: (etag: string | null) => void

  constructor(write: Saver["write"], settled: Saver["settled"]) {
    this.write = write
    this.settled = settled
  }

  save(fields: Record<string, unknown>) {
    this.next = { ...this.next, ...fields }
    void this.send()
  }

  private async send() {
    if (this.flying || !this.next) return
    this.flying = true
    const fields = this.next
    this.next = null
    let answered: string | null = null
    try {
      const answer = await this.write(fields, this.etag)
      answered = this.etag = answer.etag
      for (const warning of answer.warnings ?? []) say(warning)
    } catch (error) {
      say("That edit didn't save", error)
      this.etag = undefined
      this.next = null
    }
    this.flying = false
    if (this.next) void this.send()
    else this.settled(answered)
  }
}

/**
 * The Edit in Focus, as any video editor has it (Paul, 11-12, cl-957): the cut playing full size in the frame every box has, and under it one
 * track of clips on a time ruler, each as long as it lasts, its frames and its sound, snapped end to end. Drag to reorder, pull an end to trim,
 * S splits at the playhead, Delete takes a clip out with Undo. It plays in the page with no render; a moment after the cut changes it's joined
 * here, free, into the box's next version. What it writes is `fields.timeline`; a version's `timeline` is what made it, which an edit carries on from.
 */
export function VideoEditor({ name }: { name: string }) {
  const store = useCanvasStore()
  const { client } = useConnection()
  const view = useElement(name)
  const asset = view?.asset
  const viewed = view?.viewed === undefined ? undefined : asset?.prefixes?.find((version) => version.number === view.viewed)?.timeline
  const reading = viewed ?? asset?.timeline
  const fps = reading?.fps ?? 24
  const written = reading?.tracks[0]?.clips ?? NONE
  const [draft, setDraft] = useState<ClipNow[] | null>(null)
  const clips = draft ?? written
  const [selected, setSelected] = useState<number | null>(null)
  const [zoom, setZoom] = useState(1)
  const [room, setRoom] = useState(0)
  const [seen, setSeen] = useState({ from: 0, to: 0 })
  const [caret, setCaret] = useState<number | null>(null)
  const [frozen, setFrozen] = useState<number | null>(null)
  const [lift, setLift] = useState<{ from: number; to: number; x: number } | null>(null)
  const [trim, setTrim] = useState<{ index: number; side: "in" | "out" } | null>(null)
  const [preview] = useState(() => new Preview(client, store.projectId))
  const scroller = useRef<HTMLDivElement>(null)
  const track = useRef<HTMLDivElement>(null)
  const box = useRef<HTMLDivElement>(null)
  // The timeline shows its draft from a gesture's start until a read holds what that wrote.
  const gesture = useRef(false)
  const awaited = useRef<string | null>(null)
  const armed = useRef<{ index: number; side: "in" | "out" } | null>(null)
  const [saver] = useState(
    () =>
      new Saver(
        (fields) => store.patchInTurn(name, fields),
        (etag) => {
          awaited.current = etag
          if (!etag && !gesture.current) setDraft(null)
        },
      ),
  )
  useEffect(() => {
    if (draft === null) saver.etag = asset?.etag ?? undefined
    else if (!gesture.current && awaited.current && asset?.etag === awaited.current) {
      awaited.current = null
      setDraft(null)
    }
  }, [asset, draft, saver])

  // The cut as the project holds it: once that changes and settles, it's joined into the next version. Leaving joins what's waiting at once.
  const fields = asset?.fields
  const cut = JSON.stringify([fields?.timeline ?? fields?.tracks ?? null, fields?.fps ?? null, fields?.aspect_ratio ?? null, fields?.resolution ?? null])
  const opened = useRef(cut)
  const joining = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)
  useEffect(() => {
    if (cut === opened.current) return
    opened.current = cut
    clearTimeout(joining.current)
    joining.current = setTimeout(() => {
      joining.current = undefined
      // An empty cut has no film to make.
      if (current.current.length) store.join(name)
    }, JOIN_MS)
  }, [cut, name, store])
  useEffect(
    () => () => {
      if (joining.current === undefined) return
      clearTimeout(joining.current)
      if (current.current.length) store.join(name)
    },
    [name, store],
  )

  const total = useMemo(() => lay(clips, fps, 1).frames, [clips, fps])
  const fit = total > 0 ? Math.max(1, room - ROOM_AFTER) / total : 1
  const scale = frozen ?? fit * zoom
  const shown = useMemo(() => (lift ? moved(clips, lift.from, lift.to) : clips), [clips, lift])
  const layout = useMemo(() => lay(shown, fps, scale), [shown, fps, scale])
  // A new scale (a zoom, a fit, a cut longer or shorter) moves every clip with the ruler at once; only a reorder slides them.
  const drawnAt = useRef(scale)
  const rescaled = drawnAt.current !== scale
  drawnAt.current = scale
  useEffect(() => preview.edit(layout, fps), [preview, layout, fps])
  useEffect(() => () => preview.stop(), [preview])

  const held = valuesOf(view).aspect_ratio
  const ratio = typeof held === "string" ? held : ""
  const first = layout.placed.find((place) => place.clip.width && place.clip.height)?.clip
  const [w, h] = ratio.split(":").map(Number)
  const aspect = w && h ? w / h : first ? first.width! / first.height! : 16 / 9

  useLayoutEffect(() => {
    const held = scroller.current
    if (!held) return
    const measure = () => {
      setRoom(held.clientWidth)
      setSeen({ from: held.scrollLeft, to: held.scrollLeft + held.clientWidth })
    }
    measure()
    const watch = new ResizeObserver(measure)
    watch.observe(held)
    held.addEventListener("scroll", measure, { passive: true })
    return () => {
      watch.disconnect()
      held.removeEventListener("scroll", measure)
    }
  }, [])

  const commit = useCallback(
    (next: ClipNow[]) => {
      gesture.current = false
      awaited.current = null
      setDraft(next)
      saver.save({ timeline: wire(next) })
      // What's on the screen is what the edit is now, whichever version it carried on from.
      store.view(name)
    },
    [name, saver, store],
  )
  const current = useRef(clips)
  current.current = clips
  const place = useRef(layout)
  place.current = layout
  const pixels = useRef(scale)
  pixels.current = scale

  // A pointer's place on the track, in its own pixels.
  const xAt = useCallback((clientX: number) => {
    const held = track.current?.getBoundingClientRect()
    return held && held.width ? ((clientX - held.left) / held.width) * track.current!.offsetWidth : 0
  }, [])

  /** Zoomed to `next` times the fit, around `anchor` (a client x), else the playhead. */
  const zoomTo = useCallback(
    (next: number, anchor?: number) => {
      const held = scroller.current
      const to = Math.max(1, Math.min(MOST_ZOOM, next))
      if (!held) return setZoom(to)
      // Around the pointer, else the playhead where it's in view, else the middle of what is.
      let at = anchor === undefined ? xOf(place.current, preview.frame.get()) : xAt(anchor)
      if (anchor === undefined && (at < held.scrollLeft || at > held.scrollLeft + held.clientWidth)) at = held.scrollLeft + held.clientWidth / 2
      const onScreen = at - held.scrollLeft
      setZoom((was) => {
        requestAnimationFrame(() => {
          held.scrollLeft = (at * to) / was - onScreen
        })
        return to
      })
    },
    [preview, xAt],
  )

  // Pinching, or Ctrl and the wheel, zooms around the pointer: down to a frame you can take hold of.
  useEffect(() => {
    const held = scroller.current
    if (!held) return
    const wheel = (event: WheelEvent) => {
      if (!event.ctrlKey && !event.metaKey) return
      event.preventDefault()
      setZoom((was) => {
        const next = Math.max(1, Math.min(MOST_ZOOM, was * Math.exp(-event.deltaY * 0.01)))
        const at = xAt(event.clientX)
        const onScreen = at - held.scrollLeft
        requestAnimationFrame(() => {
          held.scrollLeft = (at * next) / was - onScreen
        })
        return next
      })
    }
    held.addEventListener("wheel", wheel, { passive: false })
    return () => held.removeEventListener("wheel", wheel)
  }, [xAt])

  const scrub = useCallback(
    (event: ReactPointerEvent) => {
      box.current?.focus({ preventScroll: true })
      armed.current = null
      setSelected(null)
      preview.seek(frameAt(place.current, xAt(event.clientX)))
      follow(event, (next) => preview.seek(frameAt(place.current, xAt(next.clientX))), () => {})
    },
    [preview, xAt],
  )

  const onBody = useCallback(
    (event: ReactPointerEvent, index: number) => {
      if (event.button !== 0) return
      box.current?.focus({ preventScroll: true })
      armed.current = null
      setSelected(index)
      const pressed = xAt(event.clientX)
      const held = place.current.placed[index]
      const down = frameAt(place.current, pressed)
      let to: number | null = null
      setFrozen(pixels.current)
      follow(
        event,
        (next) => {
          const x = xAt(next.clientX)
          if (to === null && Math.abs(x - pressed) < 4) return
          // Where its middle falls among the others, as they stand without it.
          const others = lay(current.current.filter((_, n) => n !== index), fps, pixels.current)
          const among = indexAt(others, x - pressed + held.x + held.w / 2)
          to = among >= index ? among + 1 : among
          setLift({ from: index, to, x: x - pressed + held.x })
        },
        () => {
          setFrozen(null)
          setLift(null)
          if (to === null) return preview.seek(down)
          if (to !== index && to !== index + 1) {
            commit(moved(current.current, index, to))
            setSelected(to > index ? to - 1 : to)
          } else {
            setSelected(index)
          }
        },
      )
    },
    [commit, fps, preview, xAt],
  )

  /** A clip with one end moved by `by` frames, the picture on the frame it's cut on. */
  const pull = useCallback(
    (clips: ClipNow[], index: number, side: "in" | "out", by: number): ClipNow[] => {
      const change = side === "in" ? trimmedIn(clips[index], fps, by) : trimmedOut(clips[index], fps, by)
      const next = clips.map((clip, n) => (n === index ? change : clip))
      const held = lay(next, fps, 1).placed[index]
      preview.seek(side === "in" ? held.at : held.at + held.frames - 1)
      return next
    },
    [fps, preview],
  )

  const onHandle = useCallback(
    (event: ReactPointerEvent, index: number, side: "in" | "out") => {
      if (event.button !== 0) return
      box.current?.focus({ preventScroll: true })
      setSelected(index)
      armed.current = { index, side }
      const pressed = xAt(event.clientX)
      const before = current.current
      const frozenScale = pixels.current
      setFrozen(frozenScale)
      setTrim({ index, side })
      gesture.current = true
      let latest: ClipNow[] | null = null
      follow(
        event,
        (next) => {
          latest = pull(before, index, side, Math.round((xAt(next.clientX) - pressed) / frozenScale))
          setDraft(latest)
        },
        () => {
          setFrozen(null)
          setTrim(null)
          gesture.current = false
          if (latest) commit(latest)
        },
      )
    },
    [commit, pull, xAt],
  )

  /** Out of the cut, with Undo: the cut as it was goes back. */
  const remove = useCallback(
    (index: number) => {
      const before = current.current
      const gone = before[index]
      if (!gone) return
      setSelected(null)
      armed.current = null
      commit(before.filter((_, n) => n !== index))
      say(`Took ${gone.name ?? named(gone)} out of the cut`, undefined, { action: { label: "Undo", run: () => commit(before) }, ms: 8_000 })
    },
    [commit],
  )

  /** The clip under the playhead in two, there: the second piece is selected. */
  const cutHere = useCallback(() => {
    const done = split(current.current, fps, preview.frame.get())
    if (!done) return
    armed.current = null
    commit(done.clips)
    setSelected(done.index)
  }, [commit, fps, preview])

  /** Names put in at the place `x` (a client x) falls among the clips. */
  const putIn = useCallback(
    (names: string[], x: number) => {
      setCaret(null)
      const asked = names.filter((held) => store.resolveName(held) !== store.resolveName(name))
      const taken = asked.flatMap((held) => clipOf(store, held, fps) ?? [])
      const refused = asked.filter((held) => !clipOf(store, held, fps))
      if (refused.length) say(`Only videos and pictures go on the timeline: ${refused.join(", ")} stayed out`)
      if (!taken.length) return
      const at = indexAt(place.current, xAt(x))
      commit([...current.current.slice(0, at), ...taken, ...current.current.slice(at)])
      setSelected(at)
    },
    [commit, fps, name, store, xAt],
  )

  const target = useMemo<DropTarget>(() => ({ over: (_, x) => setCaret(indexAt(place.current, xAt(x))), drop: putIn }), [putIn, xAt])
  useEffect(() => {
    const held = box.current
    if (!held) return
    store.dropTargets.set(held, target)
    // Carried in from Files, as a box from the canvas is.
    const heard = (event: Event) => {
      const { load, x } = (event as LetGo).detail
      if (!load.name) return
      event.stopPropagation()
      putIn([load.name], x)
    }
    held.addEventListener(LET_GO, heard)
    return () => {
      store.dropTargets.delete(held)
      held.removeEventListener(LET_GO, heard)
    }
  }, [store, target, putIn])
  // Nothing's being dragged over it any more.
  const hot = Boolean(view?.hot)
  useEffect(() => {
    if (!hot) setCaret(null)
  }, [hot])

  // Files from the computer: each a box of its own on the canvas, and a clip where they were let go.
  const dropFiles = (event: DragEvent) => {
    if (!event.dataTransfer.types.includes("Files")) return
    event.preventDefault()
    event.stopPropagation()
    setCaret(null)
    const files = [...event.dataTransfer.files].filter((file) => /^(video|image)\//.test(file.type))
    if (!files.length) return void say("Only videos and pictures go on the timeline")
    let at = indexAt(place.current, xAt(event.clientX))
    store.placeFiles(files, (made) => {
      const clip = clipOf(store, made, fps)
      if (!clip) return
      const now = current.current
      at = Math.min(at, now.length)
      commit([...now.slice(0, at), clip, ...now.slice(at)])
      at += 1
    }, name)
  }

  const key = (event: KeyboardEvent) => {
    if ((event.target as Element).closest("input")) return
    const held = () => {
      event.preventDefault()
      event.stopPropagation()
    }
    const letter = event.key.toLowerCase()
    const command = event.metaKey || event.ctrlKey
    if (event.key === " ") {
      held()
      preview.toggle()
    } else if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
      held()
      const by = event.key === "ArrowLeft" ? -1 : 1
      // The end just pulled, a frame at a time. Otherwise the playhead.
      if (armed.current && clips[armed.current.index]) {
        const { index, side } = armed.current
        setTrim({ index, side })
        commit(pull(clips, index, side, by))
      } else {
        preview.seek(preview.frame.get() + by)
      }
    } else if (letter === "s" && !command) {
      held()
      cutHere()
    } else if ((event.key === "Delete" || event.key === "Backspace") && !command) {
      held()
      if (selected !== null && clips[selected]) remove(selected)
    } else if (!command && (event.key === "=" || event.key === "+")) {
      held()
      zoomTo(zoom * 1.5)
    } else if (!command && (event.key === "-" || event.key === "_")) {
      held()
      zoomTo(zoom / 1.5)
    }
  }

  const liftedTo = lift ? (lift.to > lift.from ? lift.to - 1 : lift.to) : null
  const pulled = trim ? layout.placed[trim.index] : undefined
  return (
    <>
      <FocusFrame enabled aspect={aspect} zooms={false} under={<Transport preview={preview} layout={layout} fps={fps} />}>
        <div className="tl-player" style={{ aspectRatio: aspect }} tabIndex={-1} onKeyDown={key}>
          <Monitor preview={preview} layout={layout} empty={!clips.length} />
          {asset?.rendering != null && (
            <div className="tl-joining" role="progressbar" aria-label="Joining" aria-valuenow={Math.round(asset.rendering * 100)}>
              <i style={{ transform: `translateX(${Math.round(asset.rendering * 100) - 100}%)` }} />
            </div>
          )}
        </div>
      </FocusFrame>
      <div ref={box} className="tl nodrag nopan nowheel" data-drop-target="" tabIndex={0} aria-label="Timeline" onKeyDown={key} onKeyUp={() => setTrim(null)} onDragOver={(event) => event.dataTransfer.types.includes("Files") && event.preventDefault()} onDrop={dropFiles}>
        <div className="tl-bar" role="toolbar" aria-label="Timeline tools">
          <SplitButton preview={preview} clips={clips} fps={fps} onSplit={cutHere} />
          <button type="button" className="tb" aria-label="Delete clip" title="Delete clip (Delete)" disabled={selected === null || !clips[selected]} onClick={() => selected !== null && remove(selected)}>
            <Icon name="trash" />
            Delete
          </button>
          <span className="sp" />
          <button type="button" className="tb tl-zoom-step" aria-label="Zoom out timeline" title="Zoom out (−)" disabled={zoom <= 1} onClick={() => zoomTo(zoom / 1.5)}>
            <Icon name="minus" />
          </button>
          <input className="tl-zoom" type="range" aria-label="Timeline zoom" min={0} max={Math.log(MOST_ZOOM)} step={0.01} value={Math.log(zoom)} onChange={(event) => zoomTo(Math.exp(Number(event.target.value)))} />
          <button type="button" className="tb tl-zoom-step" aria-label="Zoom in timeline" title="Zoom in (+)" disabled={zoom >= MOST_ZOOM} onClick={() => zoomTo(zoom * 1.5)}>
            <Icon name="plus" />
          </button>
          <button type="button" className="tb" aria-label="Fit timeline" title="Fit the whole cut" disabled={zoom === 1} onClick={() => zoomTo(1)}>
            Fit
          </button>
        </div>
        <div ref={scroller} className="tl-scroll">
          <div ref={track} className="tl-track" style={{ width: Math.max(room, layout.width + ROOM_AFTER + (caret === null ? 0 : GAP)) }}>
            <div className="tl-rail" onPointerDown={scrub}>
              <Ruler layout={layout} fps={fps} from={seen.from} to={seen.to} />
            </div>
            <div className={`tl-clips ${trim || rescaled ? "pulling" : ""}`} onPointerDown={scrub}>
              {layout.placed.map((held) => (
                <Clip
                  key={`${named(held.clip)}#${shown.slice(0, held.index).filter((other) => other.clip === held.clip.clip).length}`}
                  place={held}
                  scale={scale}
                  fps={fps}
                  x={held.index === liftedTo ? lift!.x : held.x + (caret !== null && held.index >= caret ? GAP : 0)}
                  selected={selected === held.index}
                  lifted={held.index === liftedTo}
                  onBody={onBody}
                  onHandle={onHandle}
                />
              ))}
              {!clips.length && <p className="tl-hint">Drop videos and pictures here</p>}
              {caret !== null && <i className="tl-caret" style={{ transform: `translateX(${caret < layout.placed.length ? layout.placed[caret].x : layout.width}px)` }} />}
            </div>
            {pulled && trim && (
              <span className="tl-bubble" style={{ transform: `translateX(${trim.side === "in" ? pulled.x : pulled.x + pulled.w}px) translateX(-50%)` }}>
                {isStill(pulled.clip) ? `${timecode(pulled.frames, fps)}, ${pulled.frames} f` : `${trim.side === "in" ? "In" : "Out"} ${timecode(trim.side === "in" ? pulled.in : pulled.out, fps)}, ${pulled.frames} f`}
              </span>
            )}
            {layout.frames > 0 && <Playhead preview={preview} layout={layout} />}
          </div>
        </div>
      </div>
    </>
  )
}
