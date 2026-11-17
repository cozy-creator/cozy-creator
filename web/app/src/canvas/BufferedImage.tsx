import { useLayoutEffect, useRef, useState } from "react"
import { THUMB_TRIES, thumbPending, thumbWait, whenThumb } from "../api/thumbs"
import { useConnection } from "../app/Connection"
import { useCanvasStore } from "./context"

/** The take shown and at most four neighbours, each its own file, decoded. */
const HELD = 5

interface Source {
  key: string
  sha: string
  /** Its own file, project-relative; none, the thumbnail Calcifer keeps of it (cl-982). */
  path?: string
}

interface Picture extends Source {
  image: HTMLImageElement
  ready: boolean
  dead: boolean
  running: boolean
  /** Request/cache-bust generation, including accepted pending checks. */
  tries: number
  /** Consecutive genuine failures; an accepted pending response does not spend this budget. */
  errors: number
  pendingChecks: number
  probe?: AbortController
  /** Stops waiting to ask again. */
  wait?: () => void
}

interface Buffer {
  pictures: Map<string, Picture>
  sources: Map<string, Source>
  queue: Picture[]
  selected: string
  /** The selected take's thumbnail, drawn until its own file is decoded: none when it has no file of its own to draw. */
  standIn: string
  wanted: string[]
  displayed: Picture | null
  stopped: boolean
}

export interface BufferedImageProps {
  /** The picture shown, or none. While it loads, the picture shown before it stays; no other take's ever stands in for it (cl-942). */
  sha256: string | null
  /** Nearby versions, nearest first, kept decoded. The current image and at most four neighbors are held. */
  neighbors: string[]
  /** The shown take's own file, project-relative: what's drawn, never a copy of it. */
  originalPath?: string
  neighborPaths?: Record<string, string>
  onSize(aspect: number): void
  onShown(): void
}

const keyOf = (sha: string, path?: string) => JSON.stringify([sha, path ?? null])

/** A picture in Focus (cl-1184; Paul, 11-17: "displaying full resolution right?"): its own file at its own pixels, never a thumbnail's
 *  copy, and once each of its pixels is drawn wider than a screen's, drawn as they are, not smoothed. Its thumbnail shows at once, until
 *  the file is decoded. Its neighbouring takes' files stay decoded, so a step to one swaps at once. */
export function BufferedImage({ sha256, neighbors, originalPath, neighborPaths, onSize, onShown }: BufferedImageProps) {
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
  const sources = [...new Set([...(sha256 ? [sha256] : []), ...neighbors])].slice(0, HELD).map((sha): Source => {
    const path = sha === sha256 ? originalPath ?? neighborPaths?.[sha] : neighborPaths?.[sha]
    return { key: keyOf(sha, path), sha, path }
  })
  const selected = sha256 ? sources[0]?.key ?? "" : ""
  const standIn: Source | null = sha256 && sources[0]?.path ? { key: keyOf(sha256), sha: sha256 } : null
  const all = standIn ? [sources[0], standIn, ...sources.slice(1)] : sources
  const wanted = all.map((source) => source.key)
  const windowKey = JSON.stringify(wanted)
  /** Measures, in the next frame, whether its file's pixels are drawn wider than the screen's. */
  const sharpen = useRef(() => {})

  useLayoutEffect(() => {
    const element = host.current
    if (!element) return
    const focus = element.closest<HTMLElement>(".focus-image-transform")
    let frame = 0
    const measure = () => {
      frame = 0
      const image = held.current?.displayed?.path ? held.current.displayed.image : null
      let across = 0
      if (image?.naturalWidth && image.naturalHeight) {
        const drawn = element.getBoundingClientRect()
        const density = (window.devicePixelRatio || 1) * (window.visualViewport?.scale ?? 1)
        across = Math.min(drawn.width / image.naturalWidth, drawn.height / image.naturalHeight) * density
      }
      element.toggleAttribute("data-pixelated", across > 1.001)
    }
    const schedule = () => { if (!frame) frame = requestAnimationFrame(measure) }
    sharpen.current = schedule
    const resize = new ResizeObserver(schedule)
    resize.observe(element)
    // Focus zooms by its transform: no resize says so.
    const zoom = focus ? new MutationObserver(schedule) : null
    if (focus) zoom?.observe(focus, { attributes: true, attributeFilter: ["style"] })
    window.addEventListener("resize", schedule)
    window.visualViewport?.addEventListener("resize", schedule)
    return () => {
      if (frame) cancelAnimationFrame(frame)
      sharpen.current = () => {}
      resize.disconnect()
      zoom?.disconnect()
      window.removeEventListener("resize", schedule)
      window.visualViewport?.removeEventListener("resize", schedule)
    }
  }, [])

  const discard = (buffer: Buffer, picture: Picture) => {
    picture.dead = true
    picture.running = false
    picture.probe?.abort()
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
    host.current?.removeAttribute("data-pixelated")
  }

  const reveal = (buffer: Buffer, picture: Picture) => {
    const element = host.current
    if (!element || buffer.stopped || picture.dead || !picture.ready) return
    // The selected take's own file; until that's drawn, its thumbnail.
    if (picture.key !== buffer.selected && (picture.key !== buffer.standIn || buffer.displayed?.key === buffer.selected)) return
    if (buffer.displayed === picture && picture.image.parentElement === element) return
    // Reuse the decoded element itself; changing a React img key or src could paint a blank frame.
    element.replaceChildren(picture.image)
    element.dataset.displayedSha = picture.sha
    element.dataset.displayedResolution = picture.path ? "original" : "thumbnail"
    element.dataset.displayedWidth = String(picture.image.naturalWidth)
    buffer.displayed = picture
    setFailed(false)
    setShown(true)
    sharpen.current()
    const { naturalWidth: width, naturalHeight: height } = picture.image
    if (width && height) callbacks.current.onSize(width / height)
    callbacks.current.onShown()
  }

  /** `picture` asked for, at once: each is served from its project's store. One not made yet is asked again when Calcifer says it's
   *  there; one still on its way here, later each time. */
  const load = (buffer: Buffer, picture: Picture) => {
    const id = project
    if (!id || buffer.stopped || picture.dead || picture.ready || picture.running || picture.errors >= THUMB_TRIES) return
    picture.running = true
    const attempt = picture.tries
    const current = () => !buffer.stopped && !picture.dead && store.active && store.projectId === id
      && buffer.pictures.get(picture.key) === picture && picture.tries === attempt
    const base = picture.path ? client.file(id, picture.path) : client.thumb(id, picture.sha)
    const path = attempt ? `${base}${base.includes("?") ? "&" : "?"}try=${attempt}` : base
    let imageRequested = false
    void (async () => {
      // A box's first picture asked once the frame it's drawn in is up: fifty requests set up at once held a project's first frame
      // ~100 ms (web-opens A/B 43395065723).
      if (!buffer.displayed && !picture.tries) await new Promise((done) => requestAnimationFrame(() => setTimeout(done, 0)))
      if (!current()) return
      const url = await client.transport.src(path)
      if (!current()) return
      picture.image.src = url
      imageRequested = true
      await picture.image.decode()
      if (!current()) return
      picture.running = false
      picture.ready = true
      picture.errors = 0
      picture.pendingChecks = 0
      reveal(buffer, picture)
      refresh(buffer)
    })().catch(async () => {
      if (!current()) return
      let pending = false
      // Original files retain their existing load/retry path. Only a failed thumbnail needs readiness classification.
      if (!picture.path && imageRequested) {
        const probe = new AbortController()
        picture.probe = probe
        try { pending = await thumbPending(client.transport, path, probe.signal) } catch { /* Unknown failures remain bounded. */ }
        finally { if (picture.probe === probe) picture.probe = undefined }
        if (probe.signal.aborted || !current()) return
      }
      if (pending) { picture.errors = 0; picture.pendingChecks = Math.min(5, picture.pendingChecks + 1) }
      else picture.errors++
      if (picture.errors < THUMB_TRIES) {
        picture.tries++
        picture.wait = whenThumb(id, picture.sha, thumbWait(pending ? picture.pendingChecks : picture.errors), () => {
          picture.wait = undefined
          picture.running = false
          if (!buffer.stopped && !picture.dead && store.active && store.projectId === id && buffer.pictures.get(picture.key) === picture && picture.tries === attempt + 1) refresh(buffer)
        })
        return
      }
      picture.running = false
      if (buffer.selected !== picture.key) return
      if (buffer.displayed?.sha !== picture.sha) hide(buffer)
      setFailed(true)
    })
  }

  /** The picture shown first, its thumbnail beside it, drawn as it comes; its neighbours once its own file is drawn. */
  const pump = (buffer: Buffer) => {
    if (!project || buffer.stopped) return
    buffer.queue = buffer.queue.filter((picture) => !picture.dead && !picture.ready)
    const selected = buffer.pictures.get(buffer.selected)
    if (selected && !selected.ready) {
      const standIn = buffer.pictures.get(buffer.standIn)
      if (standIn) load(buffer, standIn)
      load(buffer, selected)
    } else for (const picture of buffer.queue) load(buffer, picture)
  }

  const refresh = (buffer: Buffer) => {
    if (buffer.stopped) return
    const selected = buffer.pictures.get(buffer.selected)
    const standIn = buffer.pictures.get(buffer.standIn)
    if (selected?.ready) reveal(buffer, selected)
    else if (standIn?.ready) reveal(buffer, standIn)
    if (selected && !selected.ready && selected.errors >= THUMB_TRIES) {
      if (buffer.displayed?.sha !== selected.sha) hide(buffer)
      setFailed(true)
    }
    const keep = new Set(buffer.wanted)
    // Its own file drawn, its thumbnail goes.
    if (selected?.ready) keep.delete(buffer.standIn)
    const most = HELD + Number(keep.has(buffer.standIn))
    // A far jump keeps the previous picture while loading without growing the decoded-image budget.
    if (buffer.displayed && !keep.has(buffer.displayed.key)) {
      keep.add(buffer.displayed.key)
      for (let i = buffer.wanted.length - 1; keep.size > most && i >= 0; i--) {
        if (buffer.wanted[i] !== buffer.selected && buffer.wanted[i] !== buffer.standIn) keep.delete(buffer.wanted[i])
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
      const picture: Picture = { ...source, image, ready: false, dead: false, running: false, tries: 0, errors: 0, pendingChecks: 0 }
      buffer.pictures.set(key, picture)
      buffer.queue.push(picture)
    }
    buffer.queue = buffer.queue.filter((picture) => !picture.dead)
    buffer.queue.sort((a, b) => Number(b.key === buffer.selected) - Number(a.key === buffer.selected))
    pump(buffer)
  }

  useLayoutEffect(() => {
    const buffer: Buffer = { pictures: new Map(), sources: new Map(), queue: [], selected, standIn: "", wanted: [], displayed: null, stopped: false }
    held.current = buffer
    setFailed(false)
    const stop = () => {
      buffer.stopped = true
      for (const picture of buffer.pictures.values()) discard(buffer, picture)
      buffer.queue = []
      buffer.sources.clear()
      buffer.selected = ""
      buffer.standIn = ""
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
    buffer.standIn = standIn?.key ?? ""
    buffer.wanted = wanted
    buffer.sources = new Map(all.map((source) => [source.key, source]))
    setFailed(false)
    // Showing none, as while a take is made or its file is on its way here: the picture before it goes, so it can't be taken for it.
    if (!selected) hide(buffer)
    refresh(buffer)
  }, [client, store, project, selected, windowKey])

  return <>
    <span ref={host} className="buffered-image" data-loading={sha256 && !shown && !failed ? "" : undefined} style={{ position: "absolute", inset: 0, display: "block", pointerEvents: "none" }} />
    {failed && <span className="media-note">This image isn't available</span>}
  </>
}
