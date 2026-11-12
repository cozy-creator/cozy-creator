import { type PointerEvent, type ReactNode, useCallback, useLayoutEffect, useRef, useState } from "react"
import { useCanvasStore } from "./context"

type Camera = { scale: number; x: number; y: number }
const UI = "button, input, select, textarea, a, video, audio, [contenteditable], [role=slider], .tl, .orbit"

/** Image-space pan/zoom never changes the Wander camera. */
export function FocusFrame({ enabled, aspect, fitted = true, children }: { enabled: boolean; aspect: number; fitted?: boolean; children: ReactNode }) {
  const store = useCanvasStore()
  const element = useRef<HTMLDivElement>(null)
  const [room, setRoom] = useState({ width: 0, height: 0 })
  const [camera, setCamera] = useState<Camera>({ scale: 1, x: 0, y: 0 })
  const dragging = useRef<{ id: number; x: number; y: number; from: Camera } | null>(null)
  const width = fitted && room.width && room.height ? Math.min(room.width, room.height * aspect) : room.width
  const height = fitted && width ? width / aspect : room.height
  const geometry = useRef({ width, height, room })
  geometry.current = { width, height, room }
  const bound = useCallback((next: Camera): Camera => {
    const g = geometry.current
    const x = Math.max(0, (g.width * next.scale - g.room.width) / 2)
    const y = Math.max(0, (g.height * next.scale - g.room.height) / 2)
    return { scale: next.scale, x: Math.max(-x, Math.min(x, next.x)), y: Math.max(-y, Math.min(y, next.y)) }
  }, [])
  const zoom = useCallback((value: number, relative = false, point = { x: 0, y: 0 }) => {
    setCamera((was) => {
      const scale = Math.max(0.25, Math.min(8, relative ? was.scale * value : value))
      return bound({ scale, x: point.x - (point.x - was.x) * scale / was.scale, y: point.y - (point.y - was.y) * scale / was.scale })
    })
  }, [bound])
  const reset = useCallback(() => setCamera({ scale: 1, x: 0, y: 0 }), [])
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
    if (enabled) setCamera((was) => { const next = bound(was); return was.x === next.x && was.y === next.y && was.scale === next.scale ? was : next })
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
  return <div ref={element} className="focus-viewer" onPointerDown={(event) => {
    if (!fitted || event.button !== 0 || (event.target as Element).closest(UI)) return
    event.preventDefault()
    dragging.current = { id: event.pointerId, x: event.clientX, y: event.clientY, from: camera }
    event.currentTarget.setPointerCapture(event.pointerId)
  }} onPointerMove={(event) => {
    const start = dragging.current
    if (start?.id === event.pointerId) setCamera(bound({ ...start.from, x: start.from.x + event.clientX - start.x, y: start.from.y + event.clientY - start.y }))
  }} onPointerUp={end} onPointerCancel={end} onLostPointerCapture={end}>
    <div className="focus-image-transform" style={{ width: width || "100%", height: height || "100%", transform: fitted ? `translate(${camera.x}px, ${camera.y}px) scale(${camera.scale})` : "none" }}>{children}</div>
    {fitted && <div className="focus-image-zoom" role="toolbar" aria-label="Image zoom">
      <button type="button" className="tb" aria-label="Zoom out image" onClick={() => zoom(0.8, true)}>−</button>
      <button type="button" className="tb" aria-label="Fit image" onClick={reset}>Fit</button>
      <button type="button" className="tb" aria-label="Zoom in image" onClick={() => zoom(1.25, true)}>+</button>
    </div>}
  </div>
}
