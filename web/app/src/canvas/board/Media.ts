import { Texture, VideoSource } from "pixi.js"
import type { Client } from "../../api/client"
import { THUMB_TRIES, thumbWait, whenThumb } from "../../api/thumbs"

export type MediaSource =
  | { kind: "thumb"; sha256: string }
  | { kind: "file"; sha256: string; path: string }
  | { kind: "output"; run: number | string; output: string; index: number; rev?: number }
  | { kind: "local"; url: string }
/** `width`: the texture width wanted (a power of two, at least 256; a thumbnail's source is at most 512). */
export interface MediaRequest { source: MediaSource; width: number }
export interface MediaHandle { readonly texture?: Texture; readonly failed: boolean; readonly aspect?: number; release(): void }
export interface LatentHandle {
  readonly texture?: Texture
  /** The run says its newest preview is `number`: a newer one than shown is asked for now, not at the next poll. */
  newest(number: number): void
  release(): void
}
export interface VideoHandle { readonly texture?: Texture; release(): void }

/** Every picture in view is asked for at once (cl-982, cl-1018): the browser queues them on its own connections. The bound only keeps a
 *  5,000-card view from holding thousands of decodes at once. */
const LOADS = 64
const MIB = 1024 * 1024
/** Soft: pinned textures are never evicted, so a view of 5,000 cards may pass it. Unpinned ones go oldest first past either bound. */
const BUDGET_BYTES = 768 * MIB
const UNPINNED_BYTES = 64 * MIB
const LATENT_MS = 8000
const PREVIEW_TRIES = 4

interface Entry {
  key: string
  request: MediaRequest
  pins: number
  listeners: Set<() => void>
  texture?: Texture
  bytes: number
  aspect?: number
  failed: boolean
  tries: number
  pending: number
  loading?: AbortController
  waiting?: () => void
}
interface Latent { run: number; pins: number; listeners: Set<() => void>; texture?: Texture; tag?: string; timer?: ReturnType<typeof setTimeout>; loading?: AbortController; newest?: number }

/** A texture and the ImageBitmap it was made from: Pixi forgets a bitmap on destroy without closing it. */
function drop(texture: Texture | undefined) {
  if (!texture) return
  const pixels = texture.source.resource
  texture.destroy(true)
  if (pixels instanceof ImageBitmap) pixels.close()
}

/** Width and height from an image's header, for the formats Calcifer serves: null when it can't tell. */
export function naturalSize(bytes: Uint8Array): { width: number; height: number } | null {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength)
  const at = (i: number, text: string) => [...text].every((c, n) => bytes[i + n] === c.charCodeAt(0))
  if (bytes.length >= 24 && at(1, "PNG") && at(12, "IHDR")) return { width: view.getUint32(16), height: view.getUint32(20) }
  if (bytes.length >= 10 && at(0, "GIF8")) return { width: view.getUint16(6, true), height: view.getUint16(8, true) }
  if (bytes.length >= 32 && at(0, "RIFF") && at(8, "WEBP")) {
    if (at(12, "VP8X")) return { width: 1 + (view.getUint32(24, true) & 0xffffff), height: 1 + (view.getUint32(27, true) & 0xffffff) }
    if (at(12, "VP8L")) { const b = view.getUint32(21, true); return { width: 1 + (b & 0x3fff), height: 1 + ((b >> 14) & 0x3fff) } }
    if (at(12, "VP8 ")) return { width: view.getUint16(26, true) & 0x3fff, height: view.getUint16(28, true) & 0x3fff }
  }
  if (bytes[0] === 0xff && bytes[1] === 0xd8) {
    for (let i = 2; i + 9 < bytes.length;) {
      if (bytes[i] !== 0xff) return null
      const marker = bytes[i + 1]
      if (marker === 0xd8 || marker === 0x01 || (marker >= 0xd0 && marker <= 0xd7)) { i += 2; continue }
      // A start of frame (not DHT, JPG or DAC) holds the size; the first one is the image's.
      if (marker >= 0xc0 && marker <= 0xcf && marker !== 0xc4 && marker !== 0xc8 && marker !== 0xcc) return { height: view.getUint16(i + 5), width: view.getUint16(i + 7) }
      i += 2 + view.getUint16(i + 2)
    }
  }
  return null
}

/**
 * A project's pictures as GPU textures: fetched through the page's transport, decoded once at the size drawn, shared by source and width,
 * kept while any card holds them and then least recently used. Each texture's ImageBitmap stays as its source so a lost WebGL context
 * re-uploads it (its bytes are `textureBytes`: 256×144 is 144 KiB). Nothing enters Pixi's global caches; `destroy` leaves nothing behind.
 */
export class Media {
  private entries = new Map<string, Entry>()
  /** Unpinned entries with textures, oldest release first. */
  private unpinned = new Map<string, Entry>()
  private latents = new Map<number, Latent>()
  private videos = new Set<{ stop(): void; pause(): void; resume(): void }>()
  private active = 0
  private bytes = 0
  private unpinnedBytes = 0
  private visible = true
  private stopped = false
  private framed = false
  private owed = false
  /** Pictures landed while others were still on their way: told together, one frame for all of them (cl-1018). */
  private landed = new Set<Entry>()
  private landing?: ReturnType<typeof setTimeout>
  private soon?: ReturnType<typeof setTimeout>
  /** A video preview that failed every try, by digest, until when it isn't asked for again. */
  private unpreviewable = new Map<string, number>()

  constructor(private o: { client: Client; project: string; changed(): void }) {}

  acquire(request: MediaRequest, onChange: () => void): MediaHandle {
    const key = JSON.stringify([request.source, request.width])
    let entry = this.entries.get(key)
    if (!entry) this.entries.set(key, (entry = { key, request, pins: 0, listeners: new Set(), bytes: 0, failed: false, tries: 0, pending: 0 }))
    const held = entry
    if (this.unpinned.delete(key)) this.unpinnedBytes -= held.bytes
    held.pins++
    held.listeners.add(onChange)
    // An import's own bytes are already here: decoded at once, as it's dropped. A fetch waits for the frame being built to go out.
    if (request.source.kind === "local" && !held.texture && !held.loading && !held.waiting && !held.failed && this.visible && !this.stopped && this.active < LOADS) void this.load(held)
    else this.pumpSoon()
    let released = false
    return {
      get texture() { return held.texture },
      get failed() { return held.failed },
      get aspect() { return held.aspect },
      release: () => {
        if (released) return
        released = true
        held.listeners.delete(onChange)
        if (--held.pins) return
        if (!held.texture) this.discard(held)
        else { this.unpinned.set(held.key, held); this.unpinnedBytes += held.bytes; this.evict() }
      },
    }
  }

  /** The run's newest picture of what it's making, asked again every 8 s while held; asking is what keeps the machine making them. */
  latent(run: number, onChange: () => void): LatentHandle {
    let latent = this.latents.get(run)
    if (!latent) this.latents.set(run, (latent = { run, pins: 0, listeners: new Set() }))
    const held = latent
    held.pins++
    held.listeners.add(onChange)
    if (held.pins === 1) void this.askLatent(held)
    let released = false
    return {
      get texture() { return held.texture },
      newest: (number: number) => {
        if (released || !number || number === held.newest) return
        held.newest = number
        // Each preview is said as it's made: a short render showed none, polled every LATENT_MS (render: "a passive latent preview").
        if (!held.loading) void this.askLatent(held)
      },
      release: () => {
        if (released) return
        released = true
        held.listeners.delete(onChange)
        if (--held.pins) return
        this.dropLatent(held)
      },
    }
  }

  /** A video's hover preview, silent and looping, made by Calcifer as its file landed: the texture appears with its first decoded frame. */
  video(sha256: string, onChange: () => void): VideoHandle {
    const element = document.createElement("video")
    Object.assign(element, { muted: true, loop: true, playsInline: true, disablePictureInPicture: true, preload: "auto" })
    let texture: Texture | undefined, frame = 0, tries = 0, stopped = false, retry: ReturnType<typeof setTimeout> | undefined
    // Each decoded frame where the browser says so (requestVideoFrameCallback), else each animation frame while it's held.
    const framed = "requestVideoFrameCallback" in element
    const next = () => { frame = framed ? element.requestVideoFrameCallback(shown) : requestAnimationFrame(shown) }
    const shown = () => {
      if (stopped) return
      if (!element.videoWidth) return next()
      if (!texture) {
        const source = new VideoSource({ resource: element, autoLoad: false, autoPlay: false })
        source.autoUpdate = false
        texture = new Texture({ source })
      } else texture.source.update()
      this.tell(onChange)
      next()
    }
    const load = async () => {
      if (stopped || (this.unpreviewable.get(sha256) ?? 0) > Date.now()) return
      const path = this.o.client.preview(this.o.project, sha256)
      const url = await this.o.client.transport.src(tries ? `${path}&try=${tries}` : path).catch(() => undefined)
      if (stopped || !url) return
      element.src = url
      next()
      if (this.visible) element.play().catch(() => {})
    }
    element.addEventListener("error", () => {
      if (stopped || element.error?.code === MediaError.MEDIA_ERR_NETWORK) return
      // Made on its first ask, or its file still arriving: asked again while it's held, then not for a while.
      if (++tries < PREVIEW_TRIES) retry = setTimeout(() => void load(), 1000 * 2 ** tries)
      else this.unpreviewable.set(sha256, Date.now() + 30_000)
    })
    const control = {
      stop: () => {
        stopped = true
        clearTimeout(retry)
        if (framed) element.cancelVideoFrameCallback(frame)
        else cancelAnimationFrame(frame)
        element.pause()
        element.removeAttribute("src")
        element.load()
        texture?.destroy(true)
        texture = undefined
        this.videos.delete(control)
      },
      pause: () => element.pause(),
      resume: () => { if (element.src) element.play().catch(() => {}) },
    }
    this.videos.add(control)
    void load()
    return { get texture() { return texture }, release: control.stop }
  }

  setVisible(visible: boolean) {
    if (this.visible === visible || this.stopped) return
    this.visible = visible
    if (visible) {
      for (const latent of this.latents.values()) void this.askLatent(latent)
      for (const video of this.videos) video.resume()
      this.pump()
      return
    }
    for (const entry of this.entries.values()) { entry.loading?.abort(); entry.waiting?.(); entry.waiting = undefined }
    for (const latent of this.latents.values()) { latent.loading?.abort(); clearTimeout(latent.timer) }
    for (const video of this.videos) video.pause()
  }

  /** What it holds, in the names the cells read (the old cache's): `unusedBytes` is what no card holds, kept within `unusedTargetBytes`. */
  stats() {
    let textures = 0, pinnedTextures = 0, pinnedBytes = 0, failed = 0, queued = 0
    for (const entry of this.entries.values()) {
      if (entry.texture) textures++
      if (entry.pins && entry.texture) pinnedTextures++
      if (entry.pins) pinnedBytes += entry.bytes
      if (entry.failed) failed++
      else if (entry.pins && !entry.texture && !entry.loading) queued++
    }
    return { textures, pinnedTextures, textureBytes: this.bytes, pinnedBytes, unusedBytes: this.unpinnedBytes, unusedTargetBytes: UNPINNED_BYTES, loading: this.active, failed, queued }
  }

  destroy() {
    this.stopped = true
    clearTimeout(this.landing)
    clearTimeout(this.soon)
    this.landed.clear()
    for (const entry of [...this.entries.values()]) this.discard(entry)
    for (const latent of [...this.latents.values()]) this.dropLatent(latent)
    for (const video of [...this.videos]) video.stop()
  }

  /** `onChange` now, and the canvas told once this frame: a texture's arrival is drawn in the frame it lands in, every other arrival with it. */
  private tell(onChange: () => void) {
    try { onChange() } catch { /* one card's failure is its own */ }
    if (this.framed) { this.owed = true; return }
    this.framed = true
    this.o.changed()
    requestAnimationFrame(() => {
      this.framed = false
      if (this.owed && !this.stopped) { this.owed = false; this.tell(() => {}) }
    })
  }

  /** What landed is told once nothing else is on its way, or 100 ms after the first of it: a software-rendered frame costs ~100 ms, and
   *  N pictures landing together then cost one frame, not N. */
  private land() {
    if (!this.landed.size || this.stopped) return
    if (this.active > 0) { this.landing ??= setTimeout(() => { this.landing = undefined; this.tellLanded() }, 100); return }
    this.tellLanded()
  }

  private tellLanded() {
    clearTimeout(this.landing)
    this.landing = undefined
    const landed = [...this.landed]
    this.landed.clear()
    for (const entry of landed) if (this.entries.get(entry.key) === entry) for (const listener of [...entry.listeners]) this.tell(listener)
  }

  /** Loads asked for while a frame is being built start once it's out: 50 fetches begun inside the first frame were a sixth of it on a Mac
   *  (e2e 43794715946). */
  private pumpSoon() {
    if (this.soon) return
    this.soon = setTimeout(() => { this.soon = undefined; this.pump() }, 0)
  }

  private pump() {
    if (this.stopped || !this.visible) return
    for (const entry of this.entries.values()) {
      if (this.active >= LOADS) return
      if (entry.pins && !entry.texture && !entry.loading && !entry.waiting && !entry.failed) void this.load(entry)
    }
  }

  private async load(entry: Entry) {
    const loading = (entry.loading = new AbortController())
    this.active++
    const live = () => !this.stopped && this.visible && !loading.signal.aborted && this.entries.get(entry.key) === entry && entry.pins > 0
    const { source, width } = entry.request
    const sha256 = source.kind === "thumb" || source.kind === "file" ? source.sha256 : undefined
    let bitmap: ImageBitmap | undefined
    try {
      if (source.kind === "local") {
        // An importer's blob URL is an image resource (connect-src excludes blob:); the importer owns it, and it's never revoked here.
        const image = new Image()
        image.src = source.url
        try {
          await image.decode()
          if (!live()) return
          entry.aspect = image.naturalWidth / image.naturalHeight
          bitmap = await createImageBitmap(image, this.fit(image.naturalWidth, image.naturalHeight, width))
        } finally { image.removeAttribute("src") }
      } else {
        const response = await this.o.client.transport.fetch(this.path(source), { signal: loading.signal })
        if (response.status === 202 && sha256) {
          // Calcifer is still making it: waited for, not a failure, and no try spent.
          await response.body?.cancel()
          if (!live()) return
          entry.tries = 0
          entry.pending = Math.min(5, entry.pending + 1)
          this.wait(entry, sha256, thumbWait(entry.pending))
          return
        }
        if (!response.ok || response.status === 202) { await response.body?.cancel(); throw new Error("unavailable") }
        const blob = await response.blob()
        if (!live()) return
        // An original may be a camera's JPEG, turned by its EXIF as it's decoded: its header's size isn't the picture's.
        const size = source.kind === "file" ? null : naturalSize(new Uint8Array(await blob.slice(0, 256 * 1024).arrayBuffer()))
        if (!live()) return
        if (size) {
          entry.aspect = size.width / size.height
          bitmap = await createImageBitmap(blob, this.fit(size.width, size.height, width))
        } else {
          // An original, or a format whose header isn't read here: decoded whole for its size, then made the size drawn.
          const whole = await createImageBitmap(blob)
          entry.aspect = whole.width / whole.height
          const fit = this.fit(whole.width, whole.height, width)
          bitmap = fit.resizeWidth ? await createImageBitmap(whole, fit) : whole
          if (bitmap !== whole) whole.close()
        }
      }
      if (!live()) return
      entry.texture = Texture.from(bitmap, true)
      entry.bytes = bitmap.width * bitmap.height * 4
      this.bytes += entry.bytes
      bitmap = undefined
      entry.failed = false
      this.landed.add(entry)
      this.evict()
    } catch {
      if (!live()) return
      entry.tries++
      if (sha256 && entry.tries < THUMB_TRIES) this.wait(entry, sha256, thumbWait(entry.tries))
      else { entry.failed = true; this.landed.add(entry) }
    } finally {
      bitmap?.close()
      this.active--
      if (entry.loading === loading) entry.loading = undefined
      this.pump()
      this.land()
    }
  }

  /** `createImageBitmap` options for a picture `width`×`height` drawn `wanted` wide: made narrower in its one decode, never wider. */
  private fit(width: number, height: number, wanted: number): ImageBitmapOptions {
    if (width <= wanted) return {}
    return { resizeWidth: wanted, resizeHeight: Math.max(1, Math.round(wanted * height / width)), resizeQuality: "high" }
  }

  private path(source: Exclude<MediaSource, { kind: "local" }>): string {
    const { client, project } = this.o
    if (source.kind === "thumb") return client.thumb(project, source.sha256)
    if (source.kind === "file") return client.file(project, source.path)
    return client.output({ project, run: source.run }, source.output, source.index, source.rev)
  }

  private wait(entry: Entry, sha256: string, ms: number) {
    entry.waiting = whenThumb(this.o.project, sha256, ms, () => { entry.waiting = undefined; this.pump() })
  }

  private discard(entry: Entry) {
    this.entries.delete(entry.key)
    if (this.unpinned.delete(entry.key)) this.unpinnedBytes -= entry.bytes
    this.bytes -= entry.bytes
    entry.loading?.abort()
    entry.waiting?.()
    drop(entry.texture)
    entry.texture = undefined
    entry.bytes = 0
    entry.listeners.clear()
  }

  private evict() {
    for (const entry of this.unpinned.values()) {
      if (this.bytes <= BUDGET_BYTES && this.unpinnedBytes <= UNPINNED_BYTES) return
      this.discard(entry)
    }
  }

  private async askLatent(latent: Latent) {
    clearTimeout(latent.timer)
    latent.loading?.abort()
    if (this.stopped || !this.visible || !latent.pins) return
    const loading = (latent.loading = new AbortController())
    const live = () => !this.stopped && this.visible && latent.pins > 0 && latent.loading === loading && !loading.signal.aborted
    let bitmap: ImageBitmap | undefined
    try {
      const path = this.o.client.runPreview({ project: this.o.project, run: latent.run })
      const response = await this.o.client.transport.fetch(path, { signal: loading.signal, headers: latent.tag ? { "If-None-Match": latent.tag } : {} })
      const tag = response.headers.get("etag")
      if (response.status !== 200 || !tag || tag === latent.tag) { await response.body?.cancel(); return }
      const blob = await response.blob()
      if (!live()) return
      bitmap = await createImageBitmap(blob)
      if (!live()) return
      drop(latent.texture)
      latent.texture = Texture.from(bitmap, true)
      latent.tag = tag
      bitmap = undefined
      for (const listener of [...latent.listeners]) this.tell(listener)
    } catch {
      // A preview is optional while the run's own result is still being made.
    } finally {
      bitmap?.close()
      if (latent.loading === loading) {
        latent.loading = undefined
        if (!loading.signal.aborted && latent.pins && this.visible && !this.stopped) latent.timer = setTimeout(() => void this.askLatent(latent), LATENT_MS)
      }
    }
  }

  private dropLatent(latent: Latent) {
    this.latents.delete(latent.run)
    clearTimeout(latent.timer)
    latent.loading?.abort()
    drop(latent.texture)
    latent.texture = undefined
    latent.listeners.clear()
  }
}
