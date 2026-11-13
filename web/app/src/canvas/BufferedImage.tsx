import { useLayoutEffect, useRef, useState } from "react"
import { THUMB_TRIES, thumbWait, whenThumb } from "../api/thumbs"
import { useConnection } from "../app/Connection"
import { useCanvasStore } from "./context"

/** What Calcifer keeps of every picture for the canvas, made as its file lands (cl-982): a box of 240 CSS px at 100% on a screen of
 *  twice the density. Drawn wider, a picture is its own file. */
const PICTURE = 512
const HELD = 5

interface Source {
  key: string
  sha: string
  width: number
  path?: string
}

interface Picture extends Source {
  image: HTMLImageElement
  ready: boolean
  dead: boolean
  running: boolean
  /** How many times it wasn't there yet. */
  tries: number
  /** Stops waiting to ask again. */
  wait?: () => void
}

interface Buffer {
  pictures: Map<string, Picture>
  sources: Map<string, Source>
  queue: Picture[]
  selected: string
  wanted: string[]
  displayed: Picture | null
  stopped: boolean
}

export interface BufferedImageProps {
  /** The picture shown, or none. While it loads, the picture shown before it stays; no other ever stands in for it (cl-942). */
  sha256: string | null
  /** Nearby versions, nearest first, kept decoded. The current image and at most four neighbors are held. */
  neighbors: string[]
  cardWidth: number
  /** Original project-relative image path, used where a box is drawn wider than its thumbnail. */
  originalPath?: string
  neighborPaths?: Record<string, string>
  /** Optional actual on-screen width in CSS pixels; display density is applied here. */
  displayWidth?: number
  onSize(aspect: number): void
  onShown(): void
}

/** Keep decoded DOM images nearby and swap only when the selected image is ready to paint. */
export function BufferedImage({ sha256, neighbors, cardWidth, originalPath, neighborPaths, displayWidth, onSize, onShown }: BufferedImageProps) {
  const { client, bridge, privateViewEpoch, signedOut } = useConnection()
  const store = useCanvasStore()
  const project = store.projectId
  const host = useRef<HTMLSpanElement>(null)
  const held = useRef<Buffer | null>(null)
  const callbacks = useRef({ onSize, onShown })
  callbacks.current = { onSize, onShown }
  const [failed, setFailed] = useState(false)
  // Its own picture drawn: until then its frame pulses, faintly (cl-982).
  const [shown, setShown] = useState(false)
  // Unknown until it's measured, before it's first drawn: only the size it's drawn at is asked for, not a smaller one first.
  const [resolution, setResolution] = useState<number | null>(null)
  const sources = resolution === null ? [] : [...new Set([...(sha256 ? [sha256] : []), ...neighbors])].slice(0, HELD).map((sha): Source => {
    const path = sha === sha256 ? originalPath ?? neighborPaths?.[sha] : neighborPaths?.[sha]
    const width = resolution === 0 && path ? 0 : PICTURE
    const original = width === 0 ? path : undefined
    return { key: JSON.stringify([sha, width, original]), sha, width, path: original }
  })
  const selected = sha256 ? sources[0]?.key ?? "" : ""
  const wanted = sources.map((source) => source.key)
  const windowKey = JSON.stringify(wanted)

  useLayoutEffect(() => {
    const element = host.current
    if (!element) return
    const focus = element.closest<HTMLElement>(".focus-image-transform")
    let frame = 0
    const measure = () => {
      frame = 0
      // Wander's signal is the upper zoom bound of its current bucket, so later zooms within that bucket stay sharp: a box is
      // never drawn wider than that, so only Focus, which sizes a picture itself, reads the page's layout (each read laid out
      // the whole page again, a box at a time, as a project opened: cl-885).
      const width = displayWidth ?? (focus ? element.getBoundingClientRect().width || cardWidth : cardWidth * store.sharpness.get())
      const density = (window.devicePixelRatio || 1) * (window.visualViewport?.scale ?? 1)
      const pixels = Math.ceil(Math.max(1, width) * density)
      if (element.dataset.requestedWidth !== String(pixels)) element.dataset.requestedWidth = String(pixels)
      const next = pixels > PICTURE ? 0 : PICTURE
      setResolution((was) => was === next ? was : next)
    }
    const scheduleMeasure = () => { if (!frame) frame = requestAnimationFrame(measure) }
    const resize = new ResizeObserver(scheduleMeasure)
    resize.observe(element)
    // Only the focused image needs a transform observer; Wander uses the shared sharpness signal.
    const transform = focus ? new MutationObserver(scheduleMeasure) : null
    if (focus) transform?.observe(focus, { attributes: true, attributeFilter: ["style", "class"] })
    const unwatch = store.sharpness.subscribe(scheduleMeasure)
    window.addEventListener("resize", scheduleMeasure)
    window.visualViewport?.addEventListener("resize", scheduleMeasure)
    // Measured in the next frame with every other box's, never here: a layout read in each mount's effect, between other boxes'
    // style writes, laid the page out again once a box.
    scheduleMeasure()
    return () => {
      if (frame) cancelAnimationFrame(frame)
      resize.disconnect()
      transform?.disconnect()
      unwatch()
      window.removeEventListener("resize", scheduleMeasure)
      window.visualViewport?.removeEventListener("resize", scheduleMeasure)
    }
  }, [store, project, cardWidth, displayWidth])

  const discard = (buffer: Buffer, picture: Picture) => {
    picture.dead = true
    picture.running = false
    picture.image.removeAttribute("src")
    picture.wait?.()
    picture.image.remove()
    buffer.pictures.delete(picture.key)
  }

  /** Nothing drawn: what it shows now can't be drawn, and no other picture is drawn in its place. */
  const hide = (buffer: Buffer) => {
    buffer.displayed = null
    setShown(false)
    host.current?.replaceChildren()
    host.current?.removeAttribute("data-displayed-sha")
    host.current?.removeAttribute("data-displayed-resolution")
    host.current?.removeAttribute("data-displayed-width")
  }

  const reveal = (buffer: Buffer, picture: Picture) => {
    const element = host.current
    if (!element || buffer.stopped || picture.dead || !picture.ready || picture.key !== buffer.selected) return
    if (buffer.displayed === picture && picture.image.parentElement === element) return
    // Reuse the decoded element itself; changing a React img key or src could paint a blank frame.
    element.replaceChildren(picture.image)
    element.dataset.displayedSha = picture.sha
    element.dataset.displayedResolution = picture.width === 0 ? "original" : String(picture.width)
    element.dataset.displayedWidth = String(picture.image.naturalWidth)
    buffer.displayed = picture
    setFailed(false)
    setShown(true)
    const { naturalWidth: width, naturalHeight: height } = picture.image
    if (width && height) callbacks.current.onSize(width / height)
    callbacks.current.onShown()
  }

  /** `picture` asked for, at once: each is served from its project's store. One not made yet is asked again when Calcifer says it's
   *  there; one still on its way here, later each time. */
  const load = (buffer: Buffer, picture: Picture) => {
    const id = project
    if (!id || buffer.stopped || picture.dead || picture.ready || picture.running) return
    picture.running = true
    void (async () => {
      // A box's first picture asked once the frame it's drawn in is up: fifty requests set up at once held a project's first frame
      // ~100 ms (web-opens A/B 43395065723).
      if (!buffer.displayed && !picture.tries) await new Promise((done) => requestAnimationFrame(() => setTimeout(done, 0)))
      if (buffer.stopped || picture.dead) return
      const base = picture.path ? client.file(id, picture.path) : client.thumb(id, picture.sha)
      const url = await client.transport.src(picture.tries ? `${base}${base.includes("?") ? "&" : "?"}try=${picture.tries}` : base)
      if (buffer.stopped || picture.dead) return
      picture.image.src = url
      await picture.image.decode()
      if (buffer.stopped || picture.dead) return
      picture.running = false
      picture.ready = true
      reveal(buffer, picture)
      refresh(buffer)
    })().catch(() => {
      if (buffer.stopped || picture.dead) return
      if (picture.tries + 1 < THUMB_TRIES) {
        picture.tries++
        picture.wait = whenThumb(id, picture.sha, thumbWait(picture.tries), () => {
          picture.wait = undefined
          picture.running = false
          if (!buffer.stopped && !picture.dead) refresh(buffer)
        })
        return
      }
      picture.running = false
      if (buffer.selected !== picture.key) return
      if (buffer.displayed?.sha !== picture.sha) hide(buffer)
      setFailed(true)
    })
  }

  /** The picture shown first, and its neighbours once it's drawn: a project opening asks for each box's own picture before any other's. */
  const pump = (buffer: Buffer) => {
    if (!project || buffer.stopped) return
    buffer.queue = buffer.queue.filter((picture) => !picture.dead && !picture.ready)
    const selected = buffer.pictures.get(buffer.selected)
    if (selected && !selected.ready) load(buffer, selected)
    else for (const picture of buffer.queue) load(buffer, picture)
  }

  const refresh = (buffer: Buffer) => {
    if (buffer.stopped) return
    const selected = buffer.pictures.get(buffer.selected)
    if (selected?.ready) reveal(buffer, selected)
    const keep = new Set(buffer.wanted)
    // A far jump keeps the previous picture while loading without growing the decoded-image budget.
    if (buffer.displayed && !keep.has(buffer.displayed.key)) {
      keep.add(buffer.displayed.key)
      for (let i = buffer.wanted.length - 1; keep.size > HELD && i >= 0; i--) {
        if (buffer.wanted[i] !== buffer.selected) keep.delete(buffer.wanted[i])
      }
    }
    for (const picture of buffer.pictures.values()) if (!keep.has(picture.key)) discard(buffer, picture)
    for (const key of keep) {
      const held = buffer.pictures.get(key)
      if (held) {
        if (!held.ready && !held.running && !buffer.queue.includes(held)) buffer.queue.push(held)
        continue
      }
      const image = new Image()
      image.alt = ""
      image.draggable = false
      image.decoding = "async"
      const source = buffer.sources.get(key)
      if (!source) continue
      const picture: Picture = { ...source, image, ready: false, dead: false, running: false, tries: 0 }
      buffer.pictures.set(key, picture)
      buffer.queue.push(picture)
    }
    buffer.queue = buffer.queue.filter((picture) => !picture.dead)
    buffer.queue.sort((a, b) => Number(b.key === buffer.selected) - Number(a.key === buffer.selected))
    pump(buffer)
  }

  useLayoutEffect(() => {
    const buffer: Buffer = { pictures: new Map(), sources: new Map(), queue: [], selected, wanted: [], displayed: null, stopped: false }
    held.current = buffer
    setFailed(false)
    const stop = () => {
      buffer.stopped = true
      for (const picture of buffer.pictures.values()) discard(buffer, picture)
      buffer.queue = []
      buffer.sources.clear()
      buffer.selected = ""
      buffer.wanted = []
      hide(buffer)
    }
    const epoch = project ? bridge.projectEpoch(project) : 0
    const unwatch = privateViewEpoch.subscribe(() => {
      if (project && bridge.projectEpoch(project) !== epoch) stop()
    })
    const unsign = signedOut.subscribe(() => { if (signedOut.get()) stop() })
    if (!project || signedOut.get()) stop()
    return () => {
      unwatch()
      unsign()
      stop()
    }
  }, [client, bridge, privateViewEpoch, signedOut, store, project])

  useLayoutEffect(() => {
    const buffer = held.current
    if (!buffer || buffer.stopped) return
    buffer.selected = selected
    buffer.wanted = wanted
    buffer.sources = new Map(sources.map((source) => [source.key, source]))
    setFailed(false)
    // Showing none, as while a take is made or its file is on its way here: the picture before it goes, so it can't be taken for it.
    if (!selected) hide(buffer)
    refresh(buffer)
  }, [client, store, project, selected, windowKey])

  return <>
    <span ref={host} className="buffered-image" data-loading={sha256 && !shown && !failed ? "" : undefined} style={{ position: "absolute", inset: 0, display: "block", pointerEvents: "none" }} />
    {failed && <span className="media-note">Selected image unavailable</span>}
  </>
}
