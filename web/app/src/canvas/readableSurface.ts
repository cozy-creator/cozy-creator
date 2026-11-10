import type { CSSProperties } from "react"
import type { Viewport, XYPosition } from "@xyflow/react"
import { CARD_WIDTH, type ElementView } from "./CanvasStore"
import type { Box } from "./graph"
import { aspectOf, HEAD, shownOf } from "./shown"
import { ZOOMS } from "./ZoomControl"

export const MOST_HERE = 2.5
const LEAST_BOX = 0.35

export type ReadableProjection = Readonly<{
  style: CSSProperties
  bounds: Box
  view: Viewport
  resizeDelta(from: XYPosition, to: XYPosition): XYPosition
}>

const resizeAt = (inverse: number) => (from: XYPosition, to: XYPosition): XYPosition => ({ x: (to.x - from.x) * inverse, y: (to.y - from.y) * inverse })

/** One opened card's working frame. Its canonical pose is input, never a measured node's envelope. */
export function readableSurface(source: HTMLElement) {
  const initial = source.querySelector<HTMLElement>(":scope > .here-under")
  const limit = initial && Number.parseFloat(getComputedStyle(initial).maxHeight)
  let underRoom = limit && Number.isFinite(limit) ? limit : undefined

  return (view: ElementView, pose: Box, viewport: Viewport, fitRoom?: Box): ReadableProjection => {
    if (!view.here) {
      const inverse = 1 / viewport.zoom
      return { style: {}, bounds: { ...pose }, view: viewport, resizeDelta: resizeAt(inverse) }
    }
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
    const passive = Boolean(media && shown && !shown.absent && !view.hereWords && (shown.field === "image" || shown.field === "video"))
    const apply = (zoom: number, cap: number | undefined) => {
      const text = Math.round((1 / Math.min(MOST_HERE, zoom)) * 20) / 20
      const scale = Math.max(1, text)
      const localText = text / scale
      const contentWidth = passive ? pose.w / scale : pose.w
      card.style.width = `${passive ? Math.max(CARD_WIDTH, contentWidth) : pose.w}px`
      card.style.position = "absolute"
      card.style.left = "0px"
      card.style.top = "0px"
      card.style.scale = String(scale)
      card.style.setProperty("--text", String(localText))
      card.style.setProperty("--surface-media-width", passive ? card.style.width : "100%")
      card.style.setProperty("--surface-content-width", passive ? `${contentWidth}px` : "100%")
      card.style.setProperty("--surface-media-height", passive ? `${contentWidth / aspectOf(view)}px` : "auto")
      card.style.setProperty("--surface-media-overflow", passive ? "visible" : "hidden")
      card.style.setProperty("--here-room", cap === undefined ? "none" : `${cap}px`)
      // Individual scale composes with the card's landing/drag transforms. Its origin stays theirs.
      const origin = getComputedStyle(card).transformOrigin.split(" ").map(Number.parseFloat)
      const translate = `${(scale - 1) * origin[0]}px ${(scale - 1) * origin[1]}px`
      card.style.translate = translate
      return { scale, localText, translate, width: card.style.width, contentWidth }
    }
    const measure = (scale: number) => {
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
      const origin = anchor(scale)
      // React Flow measures its wrapper with integer offsetWidth/offsetHeight. Cover every drawn pixel
      // with that same envelope, so measuring it cannot change the node's origin again.
      const x = Math.floor(origin.x + left * scale)
      const y = Math.floor(origin.y + top * scale)
      return { x, y, w: Math.ceil(origin.x + right * scale) - x, h: Math.ceil(origin.y + bottom * scale) - y }
    }
    // The picture keeps its canonical canvas anchor while readable chrome rises above it.
    const anchor = (scale: number) => {
      if (!passive || !media) return { x: pose.x, y: pose.y }
      const content = media.querySelector<HTMLElement>(":scope > .media-pixels") ?? media
      // Native CSS widths retain the actual fractional layout without camera-dependent rect rounding.
      const frameWidth = Number.parseFloat(getComputedStyle(media).width)
      const contentWidth = Number.parseFloat(getComputedStyle(content).width)
      return { x: pose.x - (media.offsetLeft + (frameWidth - contentWidth) / 2) * scale, y: pose.y + HEAD - media.offsetTop * scale }
    }

    let zoom = viewport.zoom
    let fitted: Box | undefined
    let bounds: Box
    let style: CSSProperties
    try {
      source.after(card)
      if (fitRoom) {
        const sample = (at: number) => {
          const selected = apply(at, undefined)
          let cap = underRoom
          if (under?.offsetWidth) {
            const root = card.getBoundingClientRect()
            const rect = under.getBoundingClientRect()
            const drawnScale = root.width / card.offsetWidth
            const top = (rect.top - root.top) / drawnScale
            const factor = (rect.width / under.offsetWidth) / drawnScale
            const ink = Number.parseFloat(getComputedStyle(under).borderImageOutset) * factor
            const mainTop = Math.min(0, ...[...card.querySelectorAll<HTMLElement>(":scope > .head > *")]
              .filter((part) => part.offsetWidth && part.offsetHeight)
              .map((part) => (part.getBoundingClientRect().top - root.top) / drawnScale))
            const available = fitRoom.h / (at * selected.scale) + mainTop - top - ink
            const most = fitRoom.h * (1 - LEAST_BOX) / (at * selected.scale)
            const contents = under.firstElementChild as HTMLElement | null
            const padding = contents ? getComputedStyle(contents) : null
            const control = [...under.querySelectorAll<HTMLElement>("button, input, select, textarea, [contenteditable]:not([contenteditable=false])")].find((held) => held.offsetHeight > 0)
            const line = control ? Math.max(control.offsetHeight, Number.parseFloat(getComputedStyle(control).fontSize)) : 0
            const least = line + (padding ? Number.parseFloat(padding.paddingTop) + Number.parseFloat(padding.paddingBottom) : 0) + under.clientTop * 2
            cap = Math.max(least, Math.floor(Math.min(available, most) / factor))
          }
          const current = apply(at, cap)
          const bounds = measure(current.scale)
          return { cap, bounds, fits: bounds.w * at <= fitRoom.w && bounds.h * at <= fitRoom.h }
        }
        // Start independently of the current camera. Quantized type scale makes fit non-monotonic:
        // remeasure each smaller candidate instead of bisecting across those scale changes.
        zoom = MOST_HERE
        let chosen = sample(zoom)
        for (let i = 0; i < 16 && !chosen.fits && zoom > ZOOMS[0]; i++) {
          zoom = Math.max(ZOOMS[0], Math.min(fitRoom.w / chosen.bounds.w, fitRoom.h / chosen.bounds.h, zoom - Number.EPSILON))
          chosen = sample(zoom)
        }
        if (!chosen.fits && zoom !== ZOOMS[0]) {
          zoom = ZOOMS[0]
          chosen = sample(zoom)
        }
        if (chosen.fits) {
          underRoom = chosen.cap
          fitted = measure(apply(zoom, underRoom).scale)
        }
      }
      const selected = apply(viewport.zoom, underRoom)
      bounds = measure(selected.scale)
      style = {
        width: selected.width,
        position: "absolute",
        left: `${anchor(selected.scale).x - bounds.x}px`,
        top: `${anchor(selected.scale).y - bounds.y}px`,
        scale: String(selected.scale),
        translate: selected.translate,
        "--text": String(selected.localText),
        "--surface-media-width": passive ? selected.width : "100%",
        "--surface-content-width": passive ? `${selected.contentWidth}px` : "100%",
        "--surface-media-height": passive ? `${selected.contentWidth / aspectOf(view)}px` : "auto",
        "--surface-media-overflow": passive ? "visible" : "hidden",
        "--surface-inverse": String(1 / (viewport.zoom * selected.scale)),
        "--here-room": underRoom === undefined ? "none" : `${underRoom}px`,
      } as CSSProperties
      // Keep these numbers, not mutable controller state, for a pointer-down snapshot.
      const inverse = 1 / (viewport.zoom * (passive ? 1 : selected.scale))
      const next = fitRoom && fitted ? {
        x: fitRoom.x + fitRoom.w / 2 - (fitted.x + fitted.w / 2) * zoom,
        y: fitRoom.y + fitRoom.h / 2 - (fitted.y + fitted.h / 2) * zoom,
        zoom,
      } : viewport
      return {
        style,
        bounds,
        view: next,
        resizeDelta: resizeAt(inverse),
      }
    } finally {
      card.remove()
    }
  }
}
