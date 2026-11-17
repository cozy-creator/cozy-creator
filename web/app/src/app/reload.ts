import { useEffect } from "react"
import type { Outbox } from "../api/outbox"

/** What keeps the page from reloading itself now: a form holding what the person typed, a recovery secret shown once. */
const holds = new Set<symbol>()
let again: (() => void) | null = null
/** How long nothing may be in the way before it reloads: a field just left saves what it held in that time. */
const CLEAR_MS = 1000

/** While `held`, the page doesn't reload itself for a new build of Cozy. */
export function useReloadHold(held: boolean) {
  useEffect(() => {
    if (!held) return
    const hold = Symbol("reload hold")
    holds.add(hold)
    return () => {
      holds.delete(hold)
      again?.()
    }
  }, [held])
}

/** A field the person may be typing in. */
function editing(): boolean {
  const field = document.activeElement
  if (field instanceof HTMLTextAreaElement || field instanceof HTMLSelectElement) return true
  if (field instanceof HTMLInputElement) return !["button", "checkbox", "radio", "range", "submit", "reset", "color", "file", "image"].includes(field.type)
  return field instanceof HTMLElement && field.isContentEditable
}

/**
 * Calcifer came back as another build of Cozy: the page reloads itself, with no notice, as soon as nothing would be lost (Paul, 11-17:
 * "remove this notification ... auto-refresh the webui to fetch the new version from calcifer"). Not while anything the person did is
 * on its way to Calcifer (an edit, a file's bytes, the canvas's view), a field has focus, a form holds what was typed or a recovery secret
 * is shown, a pointer is down or a file is dragged over the page. It reloads where it is: the address says the project and the box in
 * Focus, and the view it saved says where the canvas was. Never safe, it waits, and says nothing.
 */
export function reloadWhenSafe(outbox: Outbox) {
  if (again) return
  let pressed = false
  let dragged = false
  let timer: ReturnType<typeof setTimeout> | undefined
  const clear = () => outbox.quiet.get() && !holds.size && !pressed && !dragged && !editing()
  const check = () => {
    clearTimeout(timer)
    if (!clear()) return
    // Still clear a moment later: what leaving a field saves is on its way by then, and holds it.
    timer = setTimeout(() => {
      if (clear()) location.reload()
    }, CLEAR_MS)
  }
  again = check
  const on = (type: string, handler: (event: Event) => void) => window.addEventListener(type, handler, true)
  on("pointerdown", () => { pressed = true; clearTimeout(timer) })
  on("pointerup", () => { pressed = false; check() })
  on("pointercancel", () => { pressed = false; check() })
  on("dragenter", () => { dragged = true; clearTimeout(timer) })
  on("dragleave", (event) => { if (!(event as DragEvent).relatedTarget) { dragged = false; check() } })
  on("drop", () => { dragged = false; check() })
  on("dragend", () => { dragged = false; check() })
  on("focusin", () => clearTimeout(timer))
  on("focusout", () => setTimeout(check))
  on("input", () => clearTimeout(timer))
  outbox.quiet.subscribe(check)
  check()
}
