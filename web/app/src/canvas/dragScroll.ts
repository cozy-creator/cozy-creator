import { type RefObject, useEffect } from "react"

/** How far a press moves before it's a drag, in screen pixels. */
const STILL = 5
/** What's left of the speed each frame of a carry after letting go. */
const CARRY = 0.92
/** The fastest a carry starts, in its own pixels a millisecond: a flick goes a little way, never the whole column. */
const FASTEST = 1.5

/**
 * A column scrolled by dragging it up or down, with a mouse or a finger alike, carrying on a little when let go; the wheel scrolls it as
 * ever. A press that doesn't move is a click on what's under it. A drag that leaves sideways is the browser's, so a take is carried out.
 * Its ends fade where there's more to see.
 */
export function useDragScroll(ref: RefObject<HTMLElement | null>) {
  useEffect(() => {
    const el = ref.current
    if (!el) return
    let from: { x: number; y: number; top: number; id: number } | null = null
    let dragging = false
    let moves: { y: number; t: number }[] = []
    let speed = 0
    let coast = 0
    // Its own pixels per screen pixel: the column is drawn scaled.
    const scale = () => el.offsetHeight / (el.getBoundingClientRect().height || el.offsetHeight)
    const fade = () => {
      el.style.setProperty("--fade-top", el.scrollTop > 1 ? "24px" : "0px")
      el.style.setProperty("--fade-bottom", el.scrollTop + el.clientHeight < el.scrollHeight - 1 ? "24px" : "0px")
    }
    const down = (event: PointerEvent) => {
      if (event.button !== 0) return
      cancelAnimationFrame(coast)
      from = { x: event.clientX, y: event.clientY, top: el.scrollTop, id: event.pointerId }
      dragging = false
      moves = [{ y: event.clientY, t: event.timeStamp }]
      speed = 0
    }
    const move = (event: PointerEvent) => {
      if (!from || event.pointerId !== from.id) return
      const dy = event.clientY - from.y
      if (!dragging) {
        if (Math.abs(dy) < STILL || Math.abs(dy) < Math.abs(event.clientX - from.x)) return
        dragging = true
        el.setPointerCapture(event.pointerId)
      }
      el.scrollTop = from.top - dy * scale()
      // Its pace over the last tenth of a second, as a hand moves.
      moves = [...moves.filter((held) => event.timeStamp - held.t < 100), { y: event.clientY, t: event.timeStamp }]
      const first = moves[0]
      const pace = ((first.y - event.clientY) * scale()) / Math.max(50, event.timeStamp - first.t)
      speed = Math.max(-FASTEST, Math.min(FASTEST, pace))
    }
    const up = (event: PointerEvent) => {
      if (!from || event.pointerId !== from.id) return
      from = null
      if (!dragging) return
      let at = performance.now()
      const step = (now: number) => {
        el.scrollTop += speed * (now - at)
        speed *= CARRY ** ((now - at) / 16)
        at = now
        if (Math.abs(speed) > 0.02) coast = requestAnimationFrame(step)
      }
      coast = requestAnimationFrame(step)
    }
    // A drag is no click, and a vertical one carries nothing out.
    const click = (event: MouseEvent) => {
      if (!dragging) return
      dragging = false
      event.stopPropagation()
      event.preventDefault()
    }
    const carry = (event: DragEvent) => {
      if (from && Math.abs(event.clientY - from.y) > Math.abs(event.clientX - from.x)) event.preventDefault()
    }
    fade()
    const watch = new ResizeObserver(fade)
    watch.observe(el)
    for (const child of el.children) watch.observe(child)
    el.addEventListener("scroll", fade, { passive: true })
    el.addEventListener("pointerdown", down)
    el.addEventListener("pointermove", move)
    el.addEventListener("pointerup", up)
    el.addEventListener("pointercancel", up)
    el.addEventListener("click", click, true)
    el.addEventListener("dragstart", carry, true)
    return () => {
      cancelAnimationFrame(coast)
      watch.disconnect()
      el.removeEventListener("scroll", fade)
      el.removeEventListener("pointerdown", down)
      el.removeEventListener("pointermove", move)
      el.removeEventListener("pointerup", up)
      el.removeEventListener("pointercancel", up)
      el.removeEventListener("click", click, true)
      el.removeEventListener("dragstart", carry, true)
    }
  }, [ref])
}
