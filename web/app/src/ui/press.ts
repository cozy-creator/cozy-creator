/** Where the last press began. A surface a click off closes closes only when the press began off it too: a selection or a drag that starts
 *  inside it and ends outside never closes it (Paul, 11-12). Surfaces that close on a press outside (`pointerdown`) keep the rule as they are. */
let began: Element | null = null

window.addEventListener("pointerdown", (event) => void (began = event.target instanceof Element ? event.target : null), true)

/** Whether the press that ended in a click began outside what `inside` says is the surface's own. One on what's gone since (a control that
 *  shut itself, as a Confirm's Not now does) began inside what held it. */
export const pressBeganOutside = (inside: (element: Element) => boolean) => began !== null && began.isConnected && !inside(began)
