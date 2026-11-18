import { createContext, type PointerEvent, type ReactNode, useCallback, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react"
import { useSignal } from "../ui/signal"
import { useCanvasStore } from "./context"
import { canFull, enterFull, fullElement, fullNow, FullScreenContext, justLeft, leaveFull, typing } from "./FullScreen"

type Camera = { scale: number; x: number; y: number }
type Resize = { width: number; height: number; from: { width: number; height: number; camera: Camera } }
interface FocusResize {
  start(): void
  move(pixelWidth: number, pixelHeight: number): void
  end(): void
}
const ResizeContext = createContext<FocusResize | null>(null)
export const useFocusResize = () => useContext(ResizeContext)
const UI = "button, input, select, textarea, a, video, audio, [contenteditable], [role=slider], .tl, .orbit"

/** Image-space pan/zoom never changes the Wander camera. */
/** What sits right under the frame (a video's segments and Redo): its row's height is kept out of the fit. */
const UNDER = 44
/** Full screen, the transport goes once the pointer has rested this long, and comes back as it moves (cl-1080). */
const REST_MS = 2500

/** `zooms`: a picture can be zoomed and panned in it; a player that's its own control (an Edit's cut, cl-957) is only fitted, and goes full
 *  screen (cl-1080): letterboxed on black at its own aspect, what's `under` it over its bottom. */
export function FocusFrame({ enabled, aspect, fitted = true, zooms = true, under, children }: { enabled: boolean; aspect: number; fitted?: boolean; zooms?: boolean; under?: ReactNode; children: ReactNode }) {
  const zoomable = fitted && zooms
  const store = useCanvasStore()
  const element = useRef<HTMLDivElement>(null)
  const [room, setRoom] = useState({ width: 0, height: 0 })
  const [camera, setCamera] = useState<Camera>({ scale: 1, x: 0, y: 0 })
  const [resize, setResize] = useState<Resize | null>(null)
  const resizing = useRef<Resize["from"] | null>(null)
  const cameraNow = useRef(camera)
  cameraNow.current = camera
  const dragging = useRef<{ id: number; x: number; y: number; from: Camera } | null>(null)
  const moved = useRef(false)
  const player = !zooms && canFull()
  const showing = useSignal(fullElement)
  const full = showing !== null && showing === element.current
  const [offered, setOffered] = useState(false)
  const offers = useRef(0)
  const offer = useCallback(() => {
    if (++offers.current === 1) setOffered(true)
    return () => {
      if (--offers.current === 0) setOffered(false)
    }
  }, [])
  const toggle = useCallback(() => {
    const held = element.current
    if (!held) return
    if (fullNow() === held) leaveFull()
    else enterFull(held)
  }, [])
  const screen = useMemo(() => ({ full, toggle, offer }), [full, toggle, offer])
  const [resting, setResting] = useState(false)
  // Full screen, what's under it lies over the picture, which has the whole height.
  const roomHeight = Math.max(0, room.height - (under && !full ? UNDER : 0))
  const fitWidth = fitted && room.width && roomHeight ? Math.min(room.width, roomHeight * aspect) : room.width
  const fitHeight = fitted && fitWidth ? fitWidth / aspect : room.height
  const width = resize?.width ?? fitWidth
  const height = resize?.height ?? fitHeight
  const shown = resize ? {
    scale: resize.from.camera.scale,
    x: resize.from.camera.x + (width - resize.from.width) * resize.from.camera.scale / 2,
    y: resize.from.camera.y + (height - resize.from.height) * resize.from.camera.scale / 2,
  } : camera
  const geometry = useRef({ width, height, room })
  geometry.current = { width, height, room }
  const bound = useCallback((next: Camera): Camera => {
    const g = geometry.current
    const x = Math.max(0, (g.width * next.scale - g.room.width) / 2)
    const y = Math.max(0, (g.height * next.scale - g.room.height) / 2)
    return { scale: next.scale, x: Math.max(-x, Math.min(x, next.x)), y: Math.max(-y, Math.min(y, next.y)) }
  }, [])
  const zoom = useCallback((value: number, relative = false, point = { x: 0, y: 0 }) => {
    if (resizing.current) return
    setCamera((was) => {
      const scale = Math.max(1, Math.min(8, relative ? was.scale * value : value))
      return bound({ scale, x: point.x - (point.x - was.x) * scale / was.scale, y: point.y - (point.y - was.y) * scale / was.scale })
    })
  }, [bound])
  const reset = useCallback(() => setCamera({ scale: 1, x: 0, y: 0 }), [])
  const startResize = useCallback(() => {
    const { width, height } = geometry.current
    const from = { width, height, camera: cameraNow.current }
    resizing.current = from
    setResize({ width, height, from })
  }, [])
  const moveResize = useCallback((pixelWidth: number, pixelHeight: number) => {
    const from = resizing.current
    if (from) setResize({ width: pixelWidth / from.camera.scale, height: pixelHeight / from.camera.scale, from })
  }, [])
  const endResize = useCallback(() => {
    resizing.current = null
    setResize(null)
    reset()
  }, [reset])
  useLayoutEffect(() => {
    if (!enabled || !element.current) return
    const held = element.current
    const measure = () => setRoom((was) => was.width === held.clientWidth && was.height === held.clientHeight ? was : { width: held.clientWidth, height: held.clientHeight })
    measure()
    const observer = new ResizeObserver(measure)
    observer.observe(held)
    return () => observer.disconnect()
  }, [enabled])
  useLayoutEffect(() => {
    if (enabled && !resizing.current) setCamera((was) => { const next = bound(was); return was.x === next.x && was.y === next.y && was.scale === next.scale ? was : next })
  }, [enabled, width, height, room.width, room.height, bound])
  useLayoutEffect(() => {
    if (!enabled) return
    const by = (direction: number) => { if (zoomable) zoom(direction > 0 ? 1.25 : 0.8, true) }
    const to = (scale: number) => { if (zoomable) zoom(scale) }
    store.focusZoomBy = by
    store.focusZoomTo = to
    store.focusFit = reset
    return () => {
      if (store.focusZoomBy === by) store.focusZoomBy = undefined
      if (store.focusZoomTo === to) store.focusZoomTo = undefined
      if (store.focusFit === reset) store.focusFit = undefined
    }
  }, [enabled, zoomable, store, zoom, reset])
  useLayoutEffect(() => {
    if (!enabled || !zoomable || !element.current) return
    const held = element.current
    const wheel = (event: WheelEvent) => {
      if ((event.target as Element).closest(UI)) return
      event.preventDefault()
      event.stopPropagation()
      const r = held.getBoundingClientRect()
      zoom(Math.exp(-event.deltaY * (event.deltaMode === 1 ? 0.032 : 0.002)), true, { x: event.clientX - r.left - r.width / 2, y: event.clientY - r.top - r.height / 2 })
    }
    held.addEventListener("wheel", wheel, { passive: false })
    return () => held.removeEventListener("wheel", wheel)
  }, [enabled, zoomable, zoom])
  // F toggles full screen, but in a field of words; Esc leaves it, and only it (cl-1080).
  const offering = enabled && player && offered
  useEffect(() => {
    if (!offering) return
    const key = (event: KeyboardEvent) => {
      const held = element.current
      if (!held || event.defaultPrevented || event.metaKey || event.ctrlKey || event.altKey) return
      if (event.key === "Escape") {
        if (fullNow() !== held && !justLeft()) return
        event.preventDefault()
        leaveFull()
      } else if (event.key.toLowerCase() === "f" && !event.repeat && !typing(event.target)) {
        event.preventDefault()
        toggle()
      }
    }
    document.addEventListener("keydown", key)
    return () => document.removeEventListener("keydown", key)
  }, [offering, toggle])
  // Full screen, its transport goes once the pointer rests, and comes back as it moves: over the transport, it stays.
  useEffect(() => {
    const held = element.current
    if (!full || !held) return
    let timer = 0
    const wake = (event?: Event) => {
      setResting(false)
      clearTimeout(timer)
      if (!(event?.target instanceof Element && event.target.closest(".focus-under"))) timer = window.setTimeout(() => setResting(true), REST_MS)
    }
    wake()
    held.addEventListener("pointermove", wake)
    held.addEventListener("pointerdown", wake)
    return () => {
      clearTimeout(timer)
      held.removeEventListener("pointermove", wake)
      held.removeEventListener("pointerdown", wake)
      setResting(false)
    }
  }, [full])
  if (!enabled) return <>{children}</>
  const end = (event: PointerEvent<HTMLDivElement>) => {
    if (dragging.current?.id !== event.pointerId) return
    dragging.current = null
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId)
  }
  return <ResizeContext.Provider value={{ start: startResize, move: moveResize, end: endResize }}><FullScreenContext.Provider value={player ? screen : null}><div ref={element} className="focus-viewer" data-zoomed={shown.scale > 1 || undefined} data-resizing={Boolean(resize) || undefined} data-full={full || undefined} data-resting={(full && resting) || undefined} onDoubleClick={(event) => {
    // A double-click on the picture toggles full screen, as a player's does.
    if (!offering || (event.target as Element).closest("button, input, select, textarea, a, [contenteditable], .focus-under, .orbit-bar")) return
    event.stopPropagation()
    toggle()
  }} onContextMenu={full ? (event) => event.stopPropagation() : undefined} onClickCapture={(event) => {
    if (moved.current) { event.preventDefault(); event.stopPropagation(); moved.current = false }
  }} onPointerDown={(event) => {
    moved.current = false
    if (resizing.current || !zoomable || camera.scale <= 1 || event.button !== 0 || (event.target as Element).closest(UI) || !(event.target as Element).closest(".focus-image-transform")) return
    event.preventDefault()
    dragging.current = { id: event.pointerId, x: event.clientX, y: event.clientY, from: camera }
    event.currentTarget.setPointerCapture(event.pointerId)
  }} onPointerMove={(event) => {
    const start = dragging.current
    if (start?.id === event.pointerId) {
      if (Math.hypot(event.clientX - start.x, event.clientY - start.y) > 4) moved.current = true
      setCamera(bound({ ...start.from, x: start.from.x + event.clientX - start.x, y: start.from.y + event.clientY - start.y }))
    }
  }} onPointerUp={end} onPointerCancel={end} onLostPointerCapture={end}>
    <div className="focus-stack">
      <div className="focus-image-transform" style={{ width: width || "100%", height: height || "100%", transform: fitted ? `translate(${shown.x}px, ${shown.y}px) scale(${shown.scale})` : "none" }}>{children}</div>
      {/* In the frame's bottom right, a picture's own (cl-958): held to the frame, not the window, and not scaled with the picture. */}
      {zoomable && <div className="focus-image-zoom" role="toolbar" aria-label="Image zoom">
        <button type="button" className="tb" aria-label="Zoom out image" disabled={camera.scale <= 1} onClick={() => zoom(0.8, true)}>−</button>
        <button type="button" className="tb" aria-label="Zoom in image" disabled={camera.scale >= 8} onClick={() => zoom(1.25, true)}>+</button>
      </div>}
      {under && <div className="focus-under" style={full ? undefined : { width: width || "100%" }}>{under}</div>}
    </div>
  </div></FullScreenContext.Provider></ResizeContext.Provider>
}
