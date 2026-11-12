import { useLayoutEffect, useRef, useState } from "react"
import { useConnection } from "../app/Connection"
import { useCanvasStore } from "./context"

const WIDTH = 2048
const HELD = 5
const LOADING = 2

// Share only in-flight work, not decoded pictures. Neighbors must not fill the browser's API connections.
const requests: { urgent(): boolean; start(): void }[] = []
let loading = 0
function nextRequest() {
  requests.sort((a, b) => Number(b.urgent()) - Number(a.urgent()))
  while (loading < LOADING && requests.length) requests.shift()!.start()
}
function schedule(start: (done: () => void) => void, urgent: () => boolean): () => void {
  let running = false
  let released = false
  const done = () => {
    if (released) return
    released = true
    if (running) loading--
    else {
      const at = requests.indexOf(request)
      if (at >= 0) requests.splice(at, 1)
    }
    nextRequest()
  }
  const request = { urgent, start() { running = true; loading++; start(done) } }
  requests.push(request)
  nextRequest()
  return done
}

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
  cancel?: () => void
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
  sha256: string
  /** Nearby versions, nearest first. The current image and at most four neighbors are held. */
  neighbors: string[]
  cardWidth: number
  /** Original project-relative image path, used beyond the widest thumbnail. */
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
  const [resolution, setResolution] = useState(256)
  const sources = [...new Set([sha256, ...neighbors])].slice(0, HELD).map((sha): Source => {
    const path = sha === sha256 ? originalPath ?? neighborPaths?.[sha] : neighborPaths?.[sha]
    const width = resolution === 0 && path ? 0 : resolution || WIDTH
    const original = width === 0 ? path : undefined
    return { key: JSON.stringify([sha, width, original]), sha, width, path: original }
  })
  const selected = sources[0].key
  const wanted = sources.map((source) => source.key)
  const windowKey = JSON.stringify(wanted)

  useLayoutEffect(() => {
    const element = host.current
    if (!element) return
    const focus = element.closest<HTMLElement>(".focus-image-transform")
    let frame = 0
    const measure = () => {
      frame = 0
      const actual = element.getBoundingClientRect().width
      // Wander's signal is the upper zoom bound of its current bucket, so later zooms within that bucket stay sharp.
      const width = displayWidth ?? (focus ? actual || cardWidth : Math.max(actual, cardWidth * store.sharpness.get()))
      const density = (window.devicePixelRatio || 1) * (window.visualViewport?.scale ?? 1)
      const pixels = Math.ceil(Math.max(1, width) * density)
      element.dataset.requestedWidth = String(pixels)
      const next = pixels > WIDTH ? 0 : pixels > 1024 ? WIDTH : pixels > 512 ? 1024 : pixels > 256 ? 512 : 256
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
    measure()
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
    picture.cancel?.()
    picture.image.remove()
    buffer.pictures.delete(picture.key)
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
    const { naturalWidth: width, naturalHeight: height } = picture.image
    if (width && height) callbacks.current.onSize(width / height)
    callbacks.current.onShown()
  }

  const pump = (buffer: Buffer) => {
    if (!project || buffer.stopped) return
    while (buffer.queue.length) {
      const picture = buffer.queue.shift()!
      if (picture.dead || picture.ready || picture.running) continue
      picture.running = true
      picture.cancel = schedule((done) => {
        void (async () => {
          if (buffer.stopped || picture.dead) return
          const path = picture.path ? client.file(project, picture.path) : client.thumb(project, picture.sha, picture.width)
          const url = await client.transport.src(path)
          if (buffer.stopped || picture.dead) return
          picture.image.src = url
          await picture.image.decode()
          if (buffer.stopped || picture.dead) return
          picture.ready = true
          reveal(buffer, picture)
          refresh(buffer)
        })().catch(() => {
          if (!buffer.stopped && !picture.dead && buffer.selected === picture.key) setFailed(true)
        }).finally(() => {
          picture.running = false
          done()
        })
      }, () => picture.key === buffer.selected)
    }
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
      const picture: Picture = { ...source, image, ready: false, dead: false, running: false }
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
      buffer.displayed = null
      host.current?.replaceChildren()
      host.current?.removeAttribute("data-displayed-sha")
      host.current?.removeAttribute("data-displayed-resolution")
      host.current?.removeAttribute("data-displayed-width")
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
    refresh(buffer)
  }, [client, store, project, selected, windowKey])

  return <>
    <span ref={host} className="buffered-image" style={{ position: "absolute", inset: 0, display: "block", pointerEvents: "none" }} />
    {failed && <span className="media-note">Selected image unavailable</span>}
  </>
}
