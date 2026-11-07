import { useCallback, useEffect, useRef } from "react"
import { type Load, lift, noClick, noNativeDrag, STILL } from "./carry"

/** What's carried from a press on a list, and what its ghost is drawn from. */
export interface Lift {
  load: Load
  from: HTMLElement
}

/** What's left of the speed each frame of a carry on after letting go. */
const CARRY = 0.92
/** The fastest a carry on starts, in its own pixels a millisecond: a flick goes a little way, never the whole list. */
const FASTEST = 1.5
/** How far a drag may lean off the list's own way and still scroll it: 35°. */
const LEAN = Math.tan((35 * Math.PI) / 180)
/** How long the marker of where the list is stays once it stops. */
const MARKED_MS = 700

/**
 * A list scrolled by dragging it its own way, up and down or across, by a mouse, a pen or a finger alike, carrying on a little when let go; the wheel
 * scrolls it as ever. A press does nothing till it's moved STILL px; then a drag within 35° of the list's way scrolls it, and any other picks up
 * what was pressed (`lift`) and carries it out. A list with nothing to scroll carries whichever way. A press that doesn't move is a click on what's
 * under it. While it scrolls, a slim marker shows where it is, and goes; there's never a bar. Down a list, its ends fade where there's more to see.
 * Returns the list's ref.
 */
export function useDragScroll({ lift: liftOf, axis = "y" }: { lift?: (pressed: Element) => Lift | null; axis?: "x" | "y" } = {}) {
  const lifts = useRef(liftOf)
  useEffect(() => {
    lifts.current = liftOf
  })
  return useCallback(
    (el: HTMLElement | null) => {
      if (!el) return
      const down = axis === "y"
      const along = (event: PointerEvent) => (down ? event.clientY : event.clientX)
      const at = () => (down ? el.scrollTop : el.scrollLeft)
      const go = (to: number) => void (down ? (el.scrollTop = to) : (el.scrollLeft = to))
      const room = () => (down ? el.scrollHeight - el.clientHeight : el.scrollWidth - el.clientWidth)
      // Its own pixels per screen pixel: it may be drawn scaled.
      const scale = () => {
        const rect = el.getBoundingClientRect()
        return down ? el.offsetHeight / (rect.height || el.offsetHeight) : el.offsetWidth / (rect.width || el.offsetWidth)
      }
      let press: { x: number; y: number; from: number; id: number; on: Element } | null = null
      let scrolling = false
      let moves: { at: number; t: number }[] = []
      let speed = 0
      let coast = 0
      let marker: HTMLElement | null = null
      let unmark = 0
      // Where it was last, so a list hidden and shown again is where it was.
      let last = 0
      let hidden = false

      const fade = () => {
        if (!down) return
        el.style.setProperty("--fade-top", el.scrollTop > 1 ? "24px" : "0px")
        el.style.setProperty("--fade-bottom", el.scrollTop + el.clientHeight < el.scrollHeight - 1 ? "24px" : "0px")
      }
      const mark = () => {
        const span = room()
        if (span < 1) return
        marker ??= document.body.appendChild(Object.assign(document.createElement("div"), { className: `scroll-marker ${axis}` }))
        const rect = el.getBoundingClientRect()
        const track = (down ? rect.height : rect.width) - 6
        const size = Math.max(24, (track * (down ? el.clientHeight : el.clientWidth)) / (down ? el.scrollHeight : el.scrollWidth))
        const offset = 3 + ((track - size) * Math.min(1, Math.max(0, at() / span)))
        marker.style.left = `${down ? rect.right - 5 : rect.left + offset}px`
        marker.style.top = `${down ? rect.top + offset : rect.bottom - 5}px`
        marker.style[down ? "height" : "width"] = `${size}px`
        marker.classList.add("on")
        clearTimeout(unmark)
        unmark = window.setTimeout(() => marker?.classList.remove("on"), MARKED_MS)
      }
      const scrolled = () => {
        if (hidden) return
        last = at()
        fade()
        mark()
      }
      const resized = () => {
        const gone = !el.clientHeight && !el.clientWidth
        if (hidden && !gone && at() !== last) go(last)
        hidden = gone
        fade()
      }

      const stop = () => {
        window.removeEventListener("pointermove", move, true)
        window.removeEventListener("pointerup", up, true)
        window.removeEventListener("pointercancel", up, true)
        window.removeEventListener("dragstart", noNativeDrag, true)
      }
      const pressed = (event: PointerEvent) => {
        if (event.button !== 0 || (event.target as Element).closest("input, textarea, select, [contenteditable]")) return
        cancelAnimationFrame(coast)
        press = { x: event.clientX, y: event.clientY, from: at(), id: event.pointerId, on: event.target as Element }
        scrolling = false
        moves = [{ at: along(event), t: event.timeStamp }]
        speed = 0
        // Off the list a moment later, the pointer is still this press's.
        window.addEventListener("pointermove", move, true)
        window.addEventListener("pointerup", up, true)
        window.addEventListener("pointercancel", up, true)
        window.addEventListener("dragstart", noNativeDrag, true)
      }
      const move = (event: PointerEvent) => {
        if (!press || event.pointerId !== press.id) return
        if (!scrolling) {
          const [dx, dy] = [event.clientX - press.x, event.clientY - press.y]
          if (Math.hypot(dx, dy) < STILL) return
          const [ways, off] = down ? [Math.abs(dy), Math.abs(dx)] : [Math.abs(dx), Math.abs(dy)]
          const held = lifts.current?.(press.on)
          if (held && (off > ways * LEAN || room() < 1)) {
            stop()
            lift(event, held.load, held.from, press)
            press = null
            return
          }
          scrolling = true
          el.setPointerCapture(event.pointerId)
        }
        go(press.from - (along(event) - (down ? press.y : press.x)) * scale())
        // Its pace over the last tenth of a second, as a hand moves.
        moves = [...moves.filter((held) => event.timeStamp - held.t < 100), { at: along(event), t: event.timeStamp }]
        const first = moves[0]
        speed = Math.max(-FASTEST, Math.min(FASTEST, ((first.at - along(event)) * scale()) / Math.max(50, event.timeStamp - first.t)))
      }
      const up = (event: PointerEvent) => {
        if (!press || event.pointerId !== press.id) return
        press = null
        stop()
        if (!scrolling) return
        noClick()
        let then = performance.now()
        const step = (now: number) => {
          go(at() + speed * (now - then))
          speed *= CARRY ** ((now - then) / 16)
          then = now
          if (Math.abs(speed) > 0.02) coast = requestAnimationFrame(step)
        }
        coast = requestAnimationFrame(step)
      }
      fade()
      const watch = new ResizeObserver(resized)
      watch.observe(el)
      for (const child of el.children) watch.observe(child)
      el.addEventListener("scroll", scrolled, { passive: true })
      el.addEventListener("pointerdown", pressed)
      return () => {
        cancelAnimationFrame(coast)
        clearTimeout(unmark)
        marker?.remove()
        stop()
        watch.disconnect()
        el.removeEventListener("scroll", scrolled)
        el.removeEventListener("pointerdown", pressed)
      }
    },
    [axis],
  )
}
