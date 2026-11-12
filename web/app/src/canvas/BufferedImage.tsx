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

interface Picture {
  sha: string
  image: HTMLImageElement
  ready: boolean
  dead: boolean
  running: boolean
  cancel?: () => void
}

interface Buffer {
  pictures: Map<string, Picture>
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
  onSize(aspect: number): void
  onShown(): void
}

/** Keep decoded DOM images nearby and swap only when the selected image is ready to paint. */
export function BufferedImage({ sha256, neighbors, onSize, onShown }: BufferedImageProps) {
  const { client, bridge, privateViewEpoch, signedOut } = useConnection()
  const store = useCanvasStore()
  const project = store.projectId
  const host = useRef<HTMLSpanElement>(null)
  const held = useRef<Buffer | null>(null)
  const callbacks = useRef({ onSize, onShown })
  callbacks.current = { onSize, onShown }
  const [failed, setFailed] = useState(false)
  const wanted = [...new Set([sha256, ...neighbors])].slice(0, HELD)
  const windowKey = wanted.join(",")

  const discard = (buffer: Buffer, picture: Picture) => {
    picture.dead = true
    picture.running = false
    picture.image.removeAttribute("src")
    picture.cancel?.()
    picture.image.remove()
    buffer.pictures.delete(picture.sha)
  }

  const reveal = (buffer: Buffer, picture: Picture) => {
    const element = host.current
    if (!element || buffer.stopped || picture.dead || !picture.ready || picture.sha !== buffer.selected) return
    if (buffer.displayed === picture && picture.image.parentElement === element) return
    // Reuse the decoded element itself; changing a React img key or src could paint a blank frame.
    element.replaceChildren(picture.image)
    element.dataset.displayedSha = picture.sha
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
          const url = await client.transport.src(client.thumb(project, picture.sha, WIDTH))
          if (buffer.stopped || picture.dead) return
          picture.image.src = url
          await picture.image.decode()
          if (buffer.stopped || picture.dead) return
          picture.ready = true
          reveal(buffer, picture)
          refresh(buffer)
        })().catch(() => {
          if (!buffer.stopped && !picture.dead && buffer.selected === picture.sha) setFailed(true)
        }).finally(() => {
          picture.running = false
          done()
        })
      }, () => picture.sha === buffer.selected)
    }
  }

  const refresh = (buffer: Buffer) => {
    if (buffer.stopped) return
    const selected = buffer.pictures.get(buffer.selected)
    if (selected?.ready) reveal(buffer, selected)
    const keep = new Set(buffer.wanted)
    // A far jump keeps the previous picture while loading without growing the decoded-image budget.
    if (buffer.displayed && !keep.has(buffer.displayed.sha)) {
      keep.add(buffer.displayed.sha)
      for (let i = buffer.wanted.length - 1; keep.size > HELD && i >= 0; i--) {
        if (buffer.wanted[i] !== buffer.selected) keep.delete(buffer.wanted[i])
      }
    }
    for (const picture of buffer.pictures.values()) if (!keep.has(picture.sha)) discard(buffer, picture)
    for (const sha of keep) {
      const held = buffer.pictures.get(sha)
      if (held) {
        if (!held.ready && !held.running && !buffer.queue.includes(held)) buffer.queue.push(held)
        continue
      }
      const image = new Image()
      image.alt = ""
      image.draggable = false
      image.decoding = "async"
      const picture: Picture = { sha, image, ready: false, dead: false, running: false }
      buffer.pictures.set(sha, picture)
      buffer.queue.push(picture)
    }
    buffer.queue = buffer.queue.filter((picture) => !picture.dead)
    buffer.queue.sort((a, b) => Number(b.sha === buffer.selected) - Number(a.sha === buffer.selected))
    pump(buffer)
  }

  useLayoutEffect(() => {
    const buffer: Buffer = { pictures: new Map(), queue: [], selected: sha256, wanted: [], displayed: null, stopped: false }
    held.current = buffer
    setFailed(false)
    const stop = () => {
      buffer.stopped = true
      for (const picture of buffer.pictures.values()) discard(buffer, picture)
      buffer.queue = []
      buffer.displayed = null
      host.current?.replaceChildren()
      host.current?.removeAttribute("data-displayed-sha")
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
    buffer.selected = sha256
    buffer.wanted = wanted
    setFailed(false)
    refresh(buffer)
  }, [client, store, project, sha256, windowKey])

  return <>
    <span ref={host} className="buffered-image" style={{ position: "absolute", inset: 0, display: "block", pointerEvents: "none" }} />
    {failed && <span className="media-note">Selected image unavailable</span>}
  </>
}
