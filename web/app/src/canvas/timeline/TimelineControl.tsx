import { type KeyboardEvent, type PointerEvent as ReactPointerEvent, memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react"
import { useConnection } from "../../app/Connection"
import type { ClipNow } from "../../api/calcifer"
import { say } from "../../ui/notes"
import { useSignal } from "../../ui/signal"
import type { DropTarget } from "../CanvasStore"
import { useCanvasStore, useElement } from "../context"
import { Thumb } from "../Media"
import { valuesOf, widthFor } from "../shown"
import { frameAt, indexAt, isStill, lay, type Layout, moved, named, type Placed, timecode, trimmedIn, trimmedOut, wire, xOf, clipOf } from "./model"
import { Preview } from "./Preview"
import "./timeline.css"

const NONE: ClipNow[] = []
const ROOM_AFTER = 40
const STRIP_HEIGHT = 64
const STRIP_FRAMES = [1, 2, 4, 8, 16, 32]
/** The clips part this far for what's being dragged in. */
const GAP = 64
const MOST_ZOOM = 64
const WIDEST = 1280
const POSTER_FRAMES = 16

/** Follows a pointer, wherever it goes, until it lets go. */
function follow(event: ReactPointerEvent, moving: (event: PointerEvent) => void, ended: () => void) {
  event.preventDefault()
  event.stopPropagation()
  const pointer = event.pointerId
  // Off the box the pointer is still this gesture's, and the box stays open under it.
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
  const tile = STRIP_HEIGHT * (clip.width && clip.height ? clip.width / clip.height : 16 / 9)
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

const Clip = memo(function Clip({
  place,
  scale,
  x,
  selected,
  lifted,
  onBody,
  onHandle,
  onRemove,
}: {
  place: Placed
  scale: number
  x: number
  selected: boolean
  lifted: boolean
  onBody: (event: ReactPointerEvent, index: number) => void
  onHandle: (event: ReactPointerEvent, index: number, side: "in" | "out") => void
  onRemove: (index: number) => void
}) {
  const { clip, index } = place
  const name = clip.name ?? named(clip)
  return (
    <div
      className={`tl-clip ${selected ? "on" : ""} ${lifted ? "lifted" : ""} ${clip.missing ? "offline" : ""}`}
      style={{ transform: `translateX(${x}px)`, width: place.w }}
      data-clip={index}
      data-name={name}
      data-in={place.in}
      data-out={place.out}
      data-frames={place.frames}
      data-missing={clip.missing ?? undefined}
      title={clip.missing ? `${name}: media offline, ${clip.missing}. A render goes on without it.` : name}
      onPointerDown={(event) => onBody(event, index)}
    >
      {clip.missing ? (
        <>
          <b>{name}</b>
          <span>{clip.missing}</span>
        </>
      ) : (
        <>
          <Strip place={place} scale={scale} />
          <i className="tl-handle in" aria-label={`Trim the start of ${name}`} onPointerDown={(event) => onHandle(event, index, "in")} />
          <i className="tl-handle out" aria-label={`Trim the end of ${name}`} onPointerDown={(event) => onHandle(event, index, "out")} />
        </>
      )}
      {selected && (
        <button
          type="button"
          className="tl-remove"
          aria-label={`Remove ${name}`}
          onPointerDown={(event) => event.stopPropagation()}
          onClick={(event) => {
            event.stopPropagation()
            onRemove(index)
          }}
        >
          ×
        </button>
      )}
    </div>
  )
})

function Playhead({ preview, layout }: { preview: Preview; layout: Layout }) {
  const frame = useSignal(preview.frame)
  return <div className="tl-playhead" data-frame={frame} style={{ transform: `translateX(${xOf(layout, frame)}px)` }} />
}

/** What's at the playhead, center-cropped as a render is, exact once it's been sought. */
function Monitor({ preview, layout, aspect, empty }: { preview: Preview; layout: Layout; aspect: number; empty: boolean }) {
  const store = useCanvasStore()
  const holder = useRef<HTMLDivElement>(null)
  const still = useRef<HTMLCanvasElement>(null)
  const showing = useSignal(preview.showing)
  const live = useSignal(preview.live)
  const ready = useSignal(preview.ready)
  const kept = useSignal(preview.kept)
  const failed = useSignal(preview.failed)
  const playing = useSignal(preview.playing)
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
    <div className="tl-monitor" style={{ aspectRatio: aspect }} data-clip={clip ? (clip.name ?? named(clip)) : undefined} data-live={live ? "" : undefined}>
      {clip?.sha256 && !clip.missing && (isStill(clip) || !(videoReady || drawn)) && store.projectId && (
        moving && tile > 0 ? <Thumb sha256={clip.sha256} cardWidth={512} strip={POSTER_FRAMES} exact frame={tile} className="tl-poster" /> : <Thumb sha256={clip.sha256} cardWidth={1024} onSize={pictureReady} />
      )}
      <canvas ref={still} className={drawn ? "" : "off"} />
      {/* One for the box, there only while it holds its slot: what plays is whatever it holds now. */}
      <div ref={holder} className={`tl-video ${videoReady ? "" : "off"}`} />
      {empty && <p className="tl-empty">Drag videos and stills here</p>}
      {failed && <p className="tl-empty">{failed}</p>}
      {!empty && (
        <button type="button" className={`tl-play nodrag ${playing ? "going" : ""}`} aria-label={playing ? "Pause" : "Play"} onClick={() => preview.toggle()}>
          {playing ? "❚❚" : "▶"}
        </button>
      )}
    </div>
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
 * The Edit, as a phone's editor has it: a picture over a filmstrip of clips you drag around and pull the ends of, snapped together,
 * every point an exact frame. It plays in the page with no render. What it writes is `fields.timeline`; the asset's `timeline` is the
 * same clips as Calcifer reads them, and a version's is what made that version, which an edit carries on from.
 */
export function TimelineControl({ name }: { name: string }) {
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
  const [caret, setCaret] = useState<number | null>(null)
  const [frozen, setFrozen] = useState<number | null>(null)
  const [lift, setLift] = useState<{ from: number; to: number; x: number } | null>(null)
  const [trim, setTrim] = useState<{ index: number; side: "in" | "out" } | null>(null)
  const [preview] = useState(() => new Preview(client, store.projectId))
  const scroller = useRef<HTMLDivElement>(null)
  const track = useRef<HTMLDivElement>(null)
  const box = useRef<HTMLDivElement>(null)
  // The box shows its draft from a gesture's start until a read holds what that wrote.
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

  const total = useMemo(() => lay(clips, fps, 1).frames, [clips, fps])
  const fit = total > 0 ? Math.max(1, room - ROOM_AFTER) / total : 1
  const scale = frozen ?? fit * zoom
  const shown = useMemo(() => (lift ? moved(clips, lift.from, lift.to) : clips), [clips, lift])
  const layout = useMemo(() => lay(shown, fps, scale), [shown, fps, scale])
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
    const measure = () => setRoom(held.clientWidth)
    measure()
    const watch = new ResizeObserver(measure)
    watch.observe(held)
    return () => watch.disconnect()
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

  // A pointer's place on the filmstrip, in the box's own pixels, whatever the canvas's zoom.
  const xAt = useCallback((clientX: number) => {
    const held = track.current?.getBoundingClientRect()
    return held && held.width ? ((clientX - held.left) / held.width) * track.current!.offsetWidth : 0
  }, [])

  // Pinching, or Ctrl and the wheel, zooms into the filmstrip around the pointer: down to a frame you can take hold of.
  useEffect(() => {
    const held = scroller.current
    if (!held) return
    const wheel = (event: WheelEvent) => {
      if (!event.ctrlKey && !event.metaKey) return
      event.preventDefault()
      const anchor = xAt(event.clientX) - held.scrollLeft
      const at = xAt(event.clientX)
      setZoom((was) => {
        const next = Math.max(1, Math.min(MOST_ZOOM, was * Math.exp(-event.deltaY * 0.01)))
        requestAnimationFrame(() => {
          held.scrollLeft = (at * next) / was - anchor
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
      const held = lay(next, fps, pixels.current).placed[index]
      preview.seek(side === "in" ? held.at : held.at + held.frames - 1)
      return next
    },
    [fps, preview],
  )

  const onHandle = useCallback(
    (event: ReactPointerEvent, index: number, side: "in" | "out") => {
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

  const remove = useCallback(
    (index: number) => {
      setSelected(null)
      armed.current = null
      commit(current.current.filter((_, n) => n !== index))
    },
    [commit],
  )

  const target = useMemo<DropTarget>(
    () => ({
      over: (_, x) => setCaret(indexAt(place.current, xAt(x))),
      drop(names, x) {
        setCaret(null)
        const asked = names.filter((held) => held !== name)
        const taken = asked.flatMap((held) => clipOf(store, held, fps) ?? [])
        const refused = asked.filter((held) => !clipOf(store, held, fps))
        if (refused.length) say(`Only video and stills go in an Edit for now: ${refused.join(", ")} stayed out`)
        if (!taken.length) return
        const at = indexAt(place.current, xAt(x))
        commit([...current.current.slice(0, at), ...taken, ...current.current.slice(at)])
        setSelected(at)
      },
    }),
    [commit, fps, name, store, xAt],
  )
  useEffect(() => {
    const held = box.current
    if (!held) return
    store.dropTargets.set(held, target)
    return () => void store.dropTargets.delete(held)
  }, [store, target])
  // Nothing's being dragged over it any more.
  const hot = Boolean(view?.hot)
  useEffect(() => {
    if (!hot) setCaret(null)
  }, [hot])
  // It grows to fit, and stays wide.
  useEffect(() => {
    const wanted = Math.min(WIDEST, Math.max(widthFor(view), 120 + clips.length * 140))
    if ((store.position(name)?.w ?? 0) < wanted) store.setWidth(name, wanted)
  }, [clips.length, name, store, view])

  const key = (event: KeyboardEvent) => {
    const held = () => {
      event.preventDefault()
      event.stopPropagation()
    }
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
    } else if ((event.key === "Delete" || event.key === "Backspace") && selected !== null && clips[selected]) {
      held()
      remove(selected)
    }
  }

  const liftedTo = lift ? (lift.to > lift.from ? lift.to - 1 : lift.to) : null
  const pulled = trim ? layout.placed[trim.index] : undefined
  return (
    <div
      ref={box}
      className="tl"
      data-drop-target=""
      tabIndex={0}
      onKeyDown={key}
      onKeyUp={() => setTrim(null)}
    >
      <div className="tl-stage">
        <Monitor preview={preview} layout={layout} aspect={aspect} empty={!clips.length && caret === null} />
        {asset?.rendering != null && (
          <div className="tl-rendering" role="progressbar" aria-label="Rendering" aria-valuenow={Math.round(asset.rendering * 100)}>
            <i style={{ transform: `translateX(${Math.round(asset.rendering * 100) - 100}%)` }} />
          </div>
        )}
      </div>
      <div ref={scroller} className="tl-scroll nodrag nopan nowheel">
        <div ref={track} className="tl-track" style={{ width: Math.max(room, layout.width + ROOM_AFTER + (caret === null ? 0 : GAP)) }}>
          <div className="tl-rail" onPointerDown={scrub} />
          <div className={`tl-clips ${trim ? "pulling" : ""}`} onPointerDown={scrub}>
            {layout.placed.map((held) => (
              <Clip
                key={`${named(held.clip)}#${shown.slice(0, held.index).filter((other) => other.clip === held.clip.clip).length}`}
                place={held}
                scale={scale}
                x={held.index === liftedTo ? lift!.x : held.x + (caret !== null && held.index >= caret ? GAP : 0)}
                selected={selected === held.index}
                lifted={held.index === liftedTo}
                onBody={onBody}
                onHandle={onHandle}
                onRemove={remove}
              />
            ))}
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
  )
}
