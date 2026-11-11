import type { Tool } from "../api/calcifer"
import { signal } from "../ui/signal"
import type { CarriedFile } from "./CanvasStore"
import { type Carried, carried } from "./checkpoints"
import type { BasicNodeKind, Operation } from "./palette"

// What's picked up within the page goes by pointer events, a mouse's, a pen's or a finger's alike, so a list can tell a scroll from a carry by
// which way a press first moves. Only files from the computer come by the browser's own drag.

/** How far a press moves before it's a drag, in screen pixels. */
export const STILL = 6

/** What's carried, and the ways it lands: on a box that takes it, as a new box of a tool starting with `fields`, as a take's file, or as a mention by name. */
export interface Load {
  node?: BasicNodeKind
  box?: Carried
  tool?: Tool
  operation?: Operation
  fields?: Record<string, unknown>
  take?: CarriedFile
  name?: string
}

/** Where what's carried is on the screen, while it's in the air. */
export const carriedAt = signal<{ x: number; y: number } | null>(null)

/** A take carried, while it's in the air and as it's let go: a slot says as it's carried over whether it takes it. */
export const carriedFile = signal<CarriedFile | null>(null)

/** Let go, what's under the pointer hears it, innermost first: whatever takes it stops it there. */
export const LET_GO = "letgo"
export type LetGo = CustomEvent<{ load: Load; x: number; y: number }>

/** While a press may carry, the browser starts no drag of its own, of a picture or of words selected: it would take the pointer. */
export const noNativeDrag = (event: DragEvent) => event.preventDefault()

/** The click a drag ends with is no click. A finger's tap comes a little after its lift; a click of its own has a press first. */
export function noClick() {
  const stop = (event: MouseEvent) => {
    event.stopPropagation()
    event.preventDefault()
    done()
  }
  const done = () => {
    clearTimeout(timer)
    window.removeEventListener("click", stop, true)
    window.removeEventListener("pointerdown", done, true)
  }
  const timer = setTimeout(done, 300)
  window.addEventListener("click", stop, true)
  window.addEventListener("pointerdown", done, true)
}

/** A copy of `from` under the pointer, as it was held: where the press was on it stays under the pointer, or its middle does. */
function ghostOf(from: HTMLElement, press: { x: number; y: number }) {
  const rect = from.getBoundingClientRect()
  const ghost = document.createElement("div")
  ghost.className = "carry-ghost"
  ghost.style.width = `${rect.width}px`
  ghost.style.height = `${rect.height}px`
  const copy = from.cloneNode(true) as HTMLElement
  copy.style.width = `${from.offsetWidth}px`
  copy.style.height = `${from.offsetHeight}px`
  copy.style.scale = String(rect.width / (from.offsetWidth || rect.width || 1))
  ghost.append(copy)
  document.body.append(ghost)
  const on = press.x >= rect.left && press.x <= rect.right && press.y >= rect.top && press.y <= rect.bottom
  const [dx, dy] = on ? [press.x - rect.left, press.y - rect.top] : [rect.width / 2, rect.height / 2]
  return {
    at: (x: number, y: number) => void (ghost.style.translate = `${x - dx}px ${y - dy}px`),
    remove: () => ghost.remove(),
  }
}

/** `load` carried from here, the press at `press` having moved off it: a ghost of `from` follows the pointer, and where it's let go hears it. Esc puts it down. */
export function lift(event: PointerEvent, load: Load, from: HTMLElement, press: { x: number; y: number }) {
  const id = event.pointerId
  const ghost = ghostOf(from, press)
  carried.set(load.box ?? null)
  carriedFile.set(load.take ?? null)
  document.documentElement.classList.add("carrying")
  // What it's carried over hears nothing of the pointer, as under the browser's own drag.
  if (from.isConnected) from.setPointerCapture(id)
  const move = (moved: PointerEvent) => {
    if (moved.pointerId !== id) return
    ghost.at(moved.clientX, moved.clientY)
    carriedAt.set({ x: moved.clientX, y: moved.clientY })
  }
  const end = (ended: PointerEvent | KeyboardEvent) => {
    if ("pointerId" in ended ? ended.pointerId !== id : ended.key !== "Escape") return
    if (!("pointerId" in ended)) ended.stopPropagation()
    window.removeEventListener("pointermove", move, true)
    window.removeEventListener("pointerup", end, true)
    window.removeEventListener("pointercancel", end, true)
    window.removeEventListener("keydown", end, true)
    window.removeEventListener("dragstart", noNativeDrag, true)
    ghost.remove()
    document.documentElement.classList.remove("carrying")
    if (ended.type === "pointerup" && "clientX" in ended) {
      noClick()
      const detail = { load, x: ended.clientX, y: ended.clientY }
      document.elementFromPoint(ended.clientX, ended.clientY)?.dispatchEvent(new CustomEvent(LET_GO, { bubbles: true, detail }))
    }
    carriedAt.set(null)
    carried.set(null)
    carriedFile.set(null)
  }
  window.addEventListener("pointermove", move, true)
  window.addEventListener("pointerup", end, true)
  window.addEventListener("pointercancel", end, true)
  window.addEventListener("keydown", end, true)
  window.addEventListener("dragstart", noNativeDrag, true)
  move(event)
}

/** A press on one thing that can be carried any way: moved STILL px, it's carried; one that doesn't move is its click. */
export function carryOnMove(event: PointerEvent, load: Load, from: HTMLElement) {
  if (event.button !== 0) return
  const id = event.pointerId
  const press = { x: event.clientX, y: event.clientY }
  const move = (moved: PointerEvent) => {
    if (moved.pointerId !== id || Math.hypot(moved.clientX - press.x, moved.clientY - press.y) < STILL) return
    done()
    lift(moved, load, from, press)
  }
  const done = () => {
    window.removeEventListener("pointermove", move, true)
    window.removeEventListener("pointerup", done, true)
    window.removeEventListener("pointercancel", done, true)
    window.removeEventListener("dragstart", noNativeDrag, true)
  }
  window.addEventListener("pointermove", move, true)
  window.addEventListener("pointerup", done, true)
  window.addEventListener("pointercancel", done, true)
  window.addEventListener("dragstart", noNativeDrag, true)
}
