/** Where the last press began. A surface a click off closes closes only when the press began off it too: a selection or a drag that starts
 *  inside it and ends outside never closes it (Paul, 11-12). Surfaces that close on a press outside (`pointerdown`) keep the rule as they are. */
let began: Element | null = null

window.addEventListener("pointerdown", (event) => void (began = event.target instanceof Element ? event.target : null), true)

/** Whether the press that ended in a click began outside what `inside` says is the surface's own. One on what's gone since (a control that
 *  shut itself, as a Confirm's Not now does) began inside what held it. */
export const pressBeganOutside = (inside: (element: Element) => boolean) => began !== null && began.isConnected && !inside(began)

/** A control: a press, a drag or the wheel on it is its own, never the canvas's, a frame's pan or zoom, or a card's. Native controls, and
 *  what's marked `.canvas-control` (a corner, a timeline, a bar or toolbar over a picture). What's looked at (`data-looked-at`: a video, an
 *  orbit) isn't, though the canvas leaves its keys to it: it zooms and pans with the frame, and its own drag (an orbit's turn) is its own. */
const CONTROL = "button, input, select, textarea, a, audio, [contenteditable]:not([contenteditable=false]), .canvas-control"
export const isControl = (target: EventTarget | null) => {
  const held = target instanceof Element ? target.closest(CONTROL) : null
  return Boolean(held && !held.matches("[data-looked-at]"))
}

/** Where typed keys go to what has the focus, not to the canvas's keys: text being written and the canvas's own controls. A plain button
 *  isn't: Space still pans over one, as the pan's press takes the focus to the canvas. */
const TYPING = "input, select, textarea, [contenteditable]:not([contenteditable=false]), .canvas-control"
export const takesKeys = (target: EventTarget | null) => target instanceof Element && Boolean(target.closest(TYPING))
