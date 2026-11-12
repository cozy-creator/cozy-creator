import { createContext, type PointerEvent, type ReactNode, useCallback, useContext, useLayoutEffect, useRef, useState } from "react"
import { useCanvasStore } from "./context"

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
/** What sits right under the frame, at its right (a video's Pick up from here): its row's height is kept out of the fit. */
const UNDER = 44

export function FocusFrame({ enabled, aspect, fitted = true, under, children }: { enabled: boolean; aspect: number; fitted?: boolean; under?: ReactNode; children: ReactNode }) {
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
  const roomHeight = Math.max(0, room.height - (under ? UNDER : 0))
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
    const by = (direction: number) => { if (fitted) zoom(direction > 0 ? 1.25 : 0.8, true) }
    const to = (scale: number) => { if (fitted) zoom(scale) }
    store.focusZoomBy = by
    store.focusZoomTo = to
    store.focusFit = reset
    return () => {
      if (store.focusZoomBy === by) store.focusZoomBy = undefined
      if (store.focusZoomTo === to) store.focusZoomTo = undefined
      if (store.focusFit === reset) store.focusFit = undefined
    }
  }, [enabled, fitted, store, zoom, reset])
  useLayoutEffect(() => {
    if (!enabled || !fitted || !element.current) return
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
  }, [enabled, fitted, zoom])
  if (!enabled) return <>{children}</>
  const end = (event: PointerEvent<HTMLDivElement>) => {
    if (dragging.current?.id !== event.pointerId) return
    dragging.current = null
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId)
  }
  return <ResizeContext.Provider value={{ start: startResize, move: moveResize, end: endResize }}><div ref={element} className="focus-viewer" data-zoomed={shown.scale > 1 || undefined} data-resizing={Boolean(resize) || undefined} onClickCapture={(event) => {
    if (moved.current) { event.preventDefault(); event.stopPropagation(); moved.current = false }
  }} onPointerDown={(event) => {
    moved.current = false
    if (resizing.current || !fitted || camera.scale <= 1 || event.button !== 0 || (event.target as Element).closest(UI) || !(event.target as Element).closest(".focus-image-transform")) return
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
      {under && <div className="focus-under" style={{ width: width || "100%" }}>{under}</div>}
    </div>
    {fitted && <div className="focus-image-zoom" role="toolbar" aria-label="Image zoom">
      <button type="button" className="tb" aria-label="Zoom out image" disabled={camera.scale <= 1} onClick={() => zoom(0.8, true)}>−</button>
      <button type="button" className="tb" aria-label="Zoom in image" disabled={camera.scale >= 8} onClick={() => zoom(1.25, true)}>+</button>
    </div>}
  </div></ResizeContext.Provider>
}
