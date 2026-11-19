import { createContext, type PointerEvent, type ReactNode, useCallback, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react"
import { isControl } from "../ui/press"
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
/** What's drawn over the frame and never zoomed with it (an orbit's bar, a picture's Edit), and how far it's zoomed now. */
export interface FocusLayer {
  overlay: HTMLElement | null
  scale(): number
}
const LayerContext = createContext<FocusLayer | null>(null)
export const useFocusLayer = () => useContext(LayerContext)

/** Image-space pan/zoom never changes the Wander camera. */
/** What sits right under the frame (a video's segments and Redo): its row's height is kept out of the fit. */
const UNDER = 44
/** Full screen, the transport goes once the pointer has rested this long, and comes back as it moves (cl-1080). */
const REST_MS = 2500
/** The most a picture or a video zooms: 8× its fit, or to 8 screen pixels to each of its own where that's more, so one of any size can be
 *  looked at pixel by pixel (cl-1184). */
const MOST = 8

/** One viewer for a picture, a video and an orbit (cl-1190; Paul, 11-17: "on videos, especially the 360 orbits, I should be able to zoom
 *  in on a section of the video"), as image viewers do it: the wheel or a pinch zooms about the pointer; zoomed, a drag pans, or, on an
 *  orbit, whose drag turns it, a Shift-drag or a middle-button drag (as 3D viewers pan); a double-click or Fit goes back to the fit, and
 *  at the fit a player's double-click is full screen. A video plays and scrubs on as it's zoomed. Zoomed in past one screen pixel to each
 *  of its own, its pixels are drawn as they are (`pixels`: its own size).
 *  `zooms` false: a player that's its own control (an Edit's cut, cl-957) is only fitted. `full`: it plays, and goes full screen (cl-1080),
 *  letterboxed on black at its own aspect, what's `under` it over its bottom. */
export function FocusFrame({ aspect, pixels, fitted = true, zooms = true, full: plays = !zooms, under, children }: { aspect: number; pixels?: { width: number; height: number }; fitted?: boolean; zooms?: boolean; full?: boolean; under?: ReactNode; children: ReactNode }) {
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
  const frame = useRef<HTMLDivElement>(null)
  const [overlay, setOverlay] = useState<HTMLDivElement | null>(null)
  const player = plays && canFull()
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
  // Its own width as it's drawn: a file whose measured sides lie the other way from its shape (turned by its EXIF) is drawn turned.
  const own = pixels ? ((pixels.width >= pixels.height) === (aspect >= 1) ? pixels.width : pixels.height) : 0
  const density = (window.devicePixelRatio || 1) * (window.visualViewport?.scale ?? 1)
  const most = Math.max(MOST, own && width ? (MOST * own) / (width * density) : 0)
  // Screen pixels to each of its own now. Zoomed in past one each, they're drawn as they are; fitted, smoothed, however small its file.
  const across = own && width ? (width * shown.scale * density) / own : 0
  const pixelated = shown.scale > 1 && across > 1.001
  const geometry = useRef({ width, height, room, most })
  geometry.current = { width, height, room, most }
  const bound = useCallback((next: Camera): Camera => {
    const g = geometry.current
    const x = Math.max(0, (g.width * next.scale - g.room.width) / 2)
    const y = Math.max(0, (g.height * next.scale - g.room.height) / 2)
    return { scale: next.scale, x: Math.max(-x, Math.min(x, next.x)), y: Math.max(-y, Math.min(y, next.y)) }
  }, [])
  const zoom = useCallback((value: number, relative = false, point = { x: 0, y: 0 }) => {
    if (resizing.current) return
    setCamera((was) => {
      const scale = Math.max(1, Math.min(geometry.current.most, relative ? was.scale * value : value))
      return bound({ scale, x: point.x - (point.x - was.x) * scale / was.scale, y: point.y - (point.y - was.y) * scale / was.scale })
    })
  }, [bound])
  const reset = useCallback(() => setCamera({ scale: 1, x: 0, y: 0 }), [])
  const layer = useMemo<FocusLayer>(() => ({ overlay, scale: () => cameraNow.current.scale }), [overlay])
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
    if (!element.current) return
    const held = element.current
    const measure = () => setRoom((was) => was.width === held.clientWidth && was.height === held.clientHeight ? was : { width: held.clientWidth, height: held.clientHeight })
    measure()
    const observer = new ResizeObserver(measure)
    observer.observe(held)
    return () => observer.disconnect()
  }, [])
  useLayoutEffect(() => {
    if (!resizing.current) setCamera((was) => { const next = bound(was); return was.x === next.x && was.y === next.y && was.scale === next.scale ? was : next })
  }, [width, height, room.width, room.height, bound])
  useLayoutEffect(() => {
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
  }, [zoomable, store, zoom, reset])
  useLayoutEffect(() => {
    if (!zoomable || !element.current) return
    const held = element.current
    const wheel = (event: WheelEvent) => {
      // Its own controls: the wheel there is theirs, not the frame's. A video and an orbit are what's looked at, so they zoom.
      if (isControl(event.target)) return
      event.preventDefault()
      event.stopPropagation()
      // About the pointer: from the middle of the frame as fitted, which a video's transport under it moves off the viewer's.
      const r = (frame.current ?? held).getBoundingClientRect()
      // A pinch comes as the wheel with Ctrl, in small steps: each counts five times a wheel's.
      zoom(Math.exp(-event.deltaY * (event.deltaMode === 1 ? 0.032 : event.ctrlKey ? 0.01 : 0.002)), true, { x: event.clientX - r.left - r.width / 2, y: event.clientY - r.top - r.height / 2 })
    }
    held.addEventListener("wheel", wheel, { passive: false })
    return () => held.removeEventListener("wheel", wheel)
  }, [zoomable, zoom])
  // F toggles full screen, but in a field of words; Esc leaves it, and only it (cl-1080).
  const offering = player && offered
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
  const end = (event: PointerEvent<HTMLDivElement>) => {
    if (dragging.current?.id !== event.pointerId) return
    dragging.current = null
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId)
  }
  /** Whether a press here pans: zoomed, on what's looked at, by a plain drag, or, where a plain drag is the orbit's turn, Shift or the
   *  middle button. */
  const pans = (event: { button: number; shiftKey: boolean; target: EventTarget }) => {
    const target = event.target as Element
    if (resizing.current || !zoomable || cameraNow.current.scale <= 1 || isControl(target) || !target.closest(".focus-image-transform")) return false
    return event.button === 1 || (event.button === 0 && (event.shiftKey || !target.closest(".orbit")))
  }
  return <ResizeContext.Provider value={{ start: startResize, move: moveResize, end: endResize }}><LayerContext.Provider value={layer}><FullScreenContext.Provider value={player ? screen : null}><div ref={element} className="focus-viewer" data-zoomed={shown.scale > 1 || undefined} data-resizing={Boolean(resize) || undefined} data-full={full || undefined} data-resting={(full && resting) || undefined} onDoubleClick={(event) => {
    if (isControl(event.target)) return
    // Zoomed, a double-click goes back to the fit; at the fit, a player's toggles full screen.
    if (zoomable && cameraNow.current.scale > 1) {
      event.stopPropagation()
      reset()
      return
    }
    if (!offering) return
    event.stopPropagation()
    toggle()
  }} onContextMenu={full ? (event) => event.stopPropagation() : undefined} onClickCapture={(event) => {
    if (moved.current) { event.preventDefault(); event.stopPropagation(); moved.current = false }
  }} onMouseDown={(event) => {
    // A middle-button pan isn't the browser's autoscroll.
    if (event.button === 1 && pans(event)) event.preventDefault()
  }} onPointerDown={(event) => {
    moved.current = false
    if (!pans(event)) return
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
      <div ref={frame} className="focus-frame" style={{ width: width || "100%", height: height || "100%" }}>
        <div className="focus-image-transform" data-pixelated={pixelated || undefined} style={{ transform: fitted ? `translate(${shown.x}px, ${shown.y}px) scale(${shown.scale})` : "none" }}>{children}</div>
        {/* Over the frame, at its size, never zoomed: an orbit's bar, a picture's Edit. */}
        <div ref={setOverlay} className="focus-overlay" />
        {/* In the frame's bottom right, its own (cl-958): held to the frame, not the window, and not scaled with what's zoomed. */}
        {zoomable && <div className="focus-image-zoom canvas-control" role="toolbar" aria-label="Image zoom">
          {camera.scale > 1 && <button type="button" className="tb fit" aria-label="Fit to frame" title="Fit to frame (double-click)" onClick={reset}>Fit</button>}
          <button type="button" className="tb" aria-label="Zoom out image" disabled={camera.scale <= 1} onClick={() => zoom(0.8, true)}>−</button>
          <button type="button" className="tb" aria-label="Zoom in image" disabled={camera.scale >= most} onClick={() => zoom(1.25, true)}>+</button>
        </div>}
      </div>
      {under && <div className="focus-under canvas-control" style={full ? undefined : { width: width || "100%" }}>{under}</div>}
    </div>
  </div></FullScreenContext.Provider></LayerContext.Provider></ResizeContext.Provider>
}
