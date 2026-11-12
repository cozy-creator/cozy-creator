import type { CSSProperties } from "react"
import type { Viewport } from "@xyflow/react"
import { CARD_WIDTH, type ElementView } from "./CanvasStore"
import type { Box } from "./graph"
import { aspectOf, bodyBox, HEAD, shownOf } from "./shown"
import { ZOOMS } from "./ZoomControl"

export const MOST_HERE = 2.5
const LEAST_BOX = 0.35

export type ReadableProjection = Readonly<{
  style: CSSProperties
  bounds: Box
  view: Viewport
}>

/**
 * One opened card's working frame, in canvas units: a zoom draws all of it larger or smaller alike, never differently (cl-890).
 * Its canonical pose is input, never a measured node's envelope.
 */
export function readableSurface(source: HTMLElement) {
  const initial = source.querySelector<HTMLElement>(":scope > .here-under")
  const limit = initial && Number.parseFloat(getComputedStyle(initial).maxHeight)
  let underRoom = limit && Number.isFinite(limit) ? limit : undefined

  return (view: ElementView, pose: Box, viewport: Viewport, fitRoom?: Box): ReadableProjection => {
    const body = bodyBox(view, pose)
    if (!view.here) return { style: {}, bounds: body, view: viewport }
    // Candidate fits must not resize the live card while React Flow is observing it.
    // Measure its actual current markup/styles in the same parent, without starting another media load.
    const card = source.cloneNode(true) as HTMLElement
    card.inert = true
    card.setAttribute("aria-hidden", "true")
    card.style.visibility = "hidden"
    card.style.position = "absolute"
    for (const part of [card, ...card.querySelectorAll<HTMLElement>("*")]) {
      part.style.setProperty("animation", "none", "important")
      part.style.setProperty("transition", "none", "important")
    }
    card.querySelectorAll("iframe").forEach((frame) => {
      frame.removeAttribute("src")
      frame.removeAttribute("srcdoc")
    })
    const inputs = source.querySelectorAll<HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement>("input, textarea, select")
    card.querySelectorAll<HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement>("input, textarea, select").forEach((copy, i) => {
      copy.value = inputs[i].value
      if (copy instanceof HTMLInputElement && inputs[i] instanceof HTMLInputElement) copy.checked = inputs[i].checked
    })
    const originals = source.querySelectorAll<HTMLImageElement | HTMLVideoElement | HTMLAudioElement>("img, video, audio")
    card.querySelectorAll<HTMLImageElement | HTMLVideoElement | HTMLAudioElement>("img, video, audio").forEach((copy, i) => {
      const original = originals[i]
      if (copy instanceof HTMLImageElement && original instanceof HTMLImageElement) {
        copy.width = original.naturalWidth || original.width
        copy.height = original.naturalHeight || original.height
      }
      copy.removeAttribute("src")
      copy.removeAttribute("srcset")
      copy.removeAttribute("poster")
      copy.removeAttribute("autoplay")
      copy.querySelectorAll("source").forEach((child) => child.remove())
    })
    const under = card.querySelector<HTMLElement>(":scope > .here-under")
    const media = card.querySelector<HTMLElement>(":scope > .media")
    const shown = shownOf(view)
    const passive = Boolean(media && shown && !shown.absent && (shown.field === "image" || (!view.hereWords && shown.field === "video")))
    // A picture narrower than a card keeps its size, centred in a frame as wide as the card's head needs.
    const width = `${passive ? Math.max(CARD_WIDTH, body.w) : body.w}px`
    const frame = {
      "--surface-media-width": passive ? width : "100%",
      "--surface-content-width": passive ? `${body.w}px` : "100%",
      "--surface-media-height": passive ? `${body.w / aspectOf(view)}px` : "auto",
      "--surface-media-overflow": passive ? "visible" : "hidden",
    }
    card.style.width = width
    card.style.left = "0px"
    card.style.top = "0px"
    for (const [name, value] of Object.entries(frame)) card.style.setProperty(name, value)
    const apply = (cap: number | undefined) => card.style.setProperty("--here-room", cap === undefined ? "none" : `${cap}px`)
    // The picture keeps its canonical canvas anchor while the head rises above it.
    const anchor = () => {
      if (!passive || !media) return { x: body.x, y: body.y }
      const content = media.querySelector<HTMLElement>(":scope > .media-pixels") ?? media
      // Native CSS widths retain the actual fractional layout without camera-dependent rect rounding.
      const frameWidth = Number.parseFloat(getComputedStyle(media).width)
      const contentWidth = Number.parseFloat(getComputedStyle(content).width)
      return { x: body.x - media.offsetLeft - (frameWidth - contentWidth) / 2, y: body.y + HEAD - media.offsetTop }
    }
    const measure = () => {
      const root = card.getBoundingClientRect()
      const drawnScale = root.width / card.offsetWidth
      let left = 0, top = 0, right = card.offsetWidth, bottom = card.offsetHeight
      const parts = [...card.children, ...(card.querySelector(":scope > .head")?.children ?? []), ...(media?.children ?? [])]
      for (const part of parts) {
        if (!(part instanceof HTMLElement) || !part.offsetWidth || !part.offsetHeight || getComputedStyle(part).display === "none") continue
        const rect = part.getBoundingClientRect()
        const outset = Number.parseFloat(getComputedStyle(part).borderImageOutset) * (rect.width / part.offsetWidth) / drawnScale
        left = Math.min(left, (rect.left - root.left) / drawnScale - outset)
        top = Math.min(top, (rect.top - root.top) / drawnScale - outset)
        right = Math.max(right, (rect.right - root.left) / drawnScale + outset)
        bottom = Math.max(bottom, (rect.bottom - root.top) / drawnScale + outset)
      }
      const origin = anchor()
      // React Flow measures its wrapper with integer offsetWidth/offsetHeight. Cover every drawn pixel
      // with that same envelope, so measuring it cannot change the node's origin again.
      const x = Math.floor(origin.x + left)
      const y = Math.floor(origin.y + top)
      return { x, y, w: Math.ceil(origin.x + right) - x, h: Math.ceil(origin.y + bottom) - y }
    }

    let zoom = viewport.zoom
    let fitted: Box | undefined
    try {
      source.after(card)
      if (fitRoom) {
        // What's under it is as tall against the box at any zoom. It takes at most 1 - LEAST_BOX of the height, past which it scrolls;
        // the zoom fits all of it, at most MOST_HERE. Then what's under it has the height left: cut short, all of it; whole, what's left
        // below it as it's centred, so it grows there as it's typed in and nothing moves, and scrolls past that.
        apply(undefined)
        let envelope = measure()
        let short = false
        let factor = 1
        if (under?.offsetWidth) {
          const root = card.getBoundingClientRect()
          const rect = under.getBoundingClientRect()
          const drawnScale = root.width / card.offsetWidth
          factor = (rect.width / under.offsetWidth) / drawnScale
          const ink = Number.parseFloat(getComputedStyle(under).borderImageOutset) * factor
          const above = anchor().y + (rect.top - root.top) / drawnScale - ink - envelope.y
          const contents = under.firstElementChild as HTMLElement | null
          const padding = contents ? getComputedStyle(contents) : null
          const control = [...under.querySelectorAll<HTMLElement>("button, input, select, textarea, [contenteditable]:not([contenteditable=false])")].find((held) => held.offsetHeight > 0)
          const line = control ? Math.max(control.offsetHeight, Number.parseFloat(getComputedStyle(control).fontSize)) : 0
          const least = line + (padding ? Number.parseFloat(padding.paddingTop) + Number.parseFloat(padding.paddingBottom) : 0) + under.clientTop * 2
          const most = Math.max(least, Math.floor((above * (1 - LEAST_BOX)) / LEAST_BOX / factor))
          if (under.offsetHeight > most) {
            short = true
            apply(most)
            envelope = measure()
          }
        }
        zoom = Math.min(MOST_HERE, fitRoom.w / envelope.w, fitRoom.h / envelope.h)
        let cap: number | undefined
        if (under?.offsetWidth) {
          const spare = (fitRoom.h / zoom - envelope.h) / factor
          cap = Math.floor(under.offsetHeight + (short ? spare : spare / 2))
          apply(cap)
          envelope = measure()
        }
        if (zoom >= ZOOMS[0]) {
          underRoom = cap
          fitted = envelope
        }
      }
      apply(underRoom)
      const bounds = measure()
      const at = anchor()
      const style = {
        width,
        position: "absolute",
        left: `${at.x - bounds.x}px`,
        top: `${at.y - bounds.y}px`,
        ...frame,
        "--here-room": underRoom === undefined ? "none" : `${underRoom}px`,
      } as CSSProperties
      const next = fitRoom && fitted ? {
        x: fitRoom.x + fitRoom.w / 2 - (fitted.x + fitted.w / 2) * zoom,
        y: fitRoom.y + fitRoom.h / 2 - (fitted.y + fitted.h / 2) * zoom,
        zoom,
      } : viewport
      return { style, bounds, view: next }
    } finally {
      card.remove()
    }
  }
}
