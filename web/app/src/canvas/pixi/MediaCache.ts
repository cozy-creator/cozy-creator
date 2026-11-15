import { Texture } from "pixi.js"
import type { Client } from "../../api/client"
import { THUMB_TRIES, thumbWait, whenThumb } from "../../api/thumbs"

/** The prepared thumbnail is always the source below 512px. Only its decoded GPU copy is smaller. */
export const THUMB_SOURCE_WIDTH = 512
export const MIN_TEXTURE_WIDTH = 256
const MIB = 1024 * 1024
const CACHE_BYTES = 768 * MIB
const UNUSED_BYTES = 64 * MIB
const LOADS = 6
const span = (name: string, start: number) => { try { performance.measure(name, { start, end: performance.now() }) } catch { /* diagnostics only */ } }

export interface MediaRequest {
  /** Project-relative API path, or an import's existing blob URL. Never logged. */
  path: string
  /** Blob URLs belong to the importer; the cache must not revoke somebody else's URL. */
  local?: boolean
  sha256?: string
  width: number
}

export interface MediaHandle {
  readonly key: string
  readonly texture: Texture | undefined
  readonly failed: boolean
  readonly aspect: number | undefined
  release(): void
}

interface Entry {
  key: string
  request: MediaRequest
  listeners: Set<() => void>
  pins: number
  texture?: Texture
  bitmap?: ImageBitmap
  bytes: number
  aspect?: number
  tries: number
  pendingChecks?: number
  failed: boolean
  controller?: AbortController
  waiting?: () => void
  asked?: number
}

type FailureStage = "fetch" | "http" | "decode" | "texture"
interface MediaFailure {
  stage: FailureStage
  httpStatus?: number
  errorName: string
  attempt: number
  width: number
  local: boolean
}

/**
 * One scene owns its decoded sources, never Pixi's global Assets cache. Visible pictures are pinned, including a previous resolution
 * until its replacement is decoded. The 768MiB target is deliberately soft: 5,000 square 256px pictures exceed it. Overflow is reported,
 * not hidden by missing visible pictures. Unpinned pixels have their own 64MiB limit and are evicted first. Focus retains its separate
 * existing five-picture DOM buffer; this cache never preloads neighboring versions or starts a video decoder.
 */
export class MediaCache {
  private entries = new Map<string, Entry>()
  /** In insertion order, oldest unpin first. A large cull must not rescan all visible textures for each released card. */
  private unused = new Map<string, Entry>()
  private residentBytes = 0
  private unusedBytes = 0
  private active = 0
  private stopped = false
  private visible = true
  private sourceBytes = 0
  private fetchedBytes = 0
  private decodedTotal = 0
  private lastFailure?: MediaFailure

  constructor(private client: Client, private project: string, private invalidate: () => void) {}

  key(request: MediaRequest): string {
    return JSON.stringify([request.path, request.width, Boolean(request.local)])
  }

  acquire(request: MediaRequest, changed: () => void): MediaHandle {
    const key = this.key(request)
    let entry = this.entries.get(key)
    if (!entry) {
      entry = { key, request, listeners: new Set(), pins: 0, bytes: 0, tries: 0, failed: false, asked: performance.now() }
      this.entries.set(key, entry)
    }
    const held = entry
    if (this.unused.delete(key)) this.unusedBytes -= held.bytes
    held.pins++
    held.listeners.add(changed)
    this.pump()
    let released = false
    return {
      key,
      get texture() { return held.texture },
      get failed() { return held.failed },
      get aspect() { return held.aspect },
      release: () => {
        if (released) return
        released = true
        held.listeners.delete(changed)
        held.pins--
        if (!held.pins) {
          if (!held.texture) this.discard(held)
          else { this.unused.set(held.key, held); this.unusedBytes += held.bytes }
        }
        this.evict()
      },
    }
  }

  setVisible(visible: boolean) {
    this.visible = visible
    if (!visible) {
      for (const entry of this.entries.values()) {
        entry.controller?.abort()
        entry.waiting?.()
        entry.waiting = undefined
      }
    } else this.pump()
  }

  private pump() {
    if (this.stopped || !this.visible) return
    for (const entry of this.entries.values()) {
      if (this.active >= LOADS) break
      if (entry.pins && !entry.texture && !entry.controller && !entry.waiting && !entry.failed) void this.load(entry)
    }
  }

  private async load(entry: Entry) {
    const controller = new AbortController()
    entry.controller = controller
    this.active++
    let blobBytes = 0
    let raw: ImageBitmap | undefined
    let bitmap: ImageBitmap | undefined
    let image: HTMLImageElement | undefined
    let stage: FailureStage = "fetch"
    let httpStatus: number | undefined
    let at = performance.now()
    if (entry.asked !== undefined) span("cozy.thumb.queued", entry.asked)
    const step = (name: string) => { span(name, at); at = performance.now() }
    const live = () => !this.stopped && this.visible && !controller.signal.aborted && this.entries.get(entry.key) === entry && entry.pins > 0
    try {
      const { request } = entry
      if (request.local) {
        // Import previews are image resources; connect-src deliberately excludes blob URLs.
        image = new Image()
        const released = () => image?.removeAttribute("src")
        controller.signal.addEventListener("abort", released, { once: true })
        image.src = request.path
        stage = "decode"
        await image.decode()
        if (!live()) return
        raw = await createImageBitmap(image)
      } else {
        const response = await this.client.transport.fetch(request.path, { signal: controller.signal })
        step("cozy.thumb.fetch")
        httpStatus = response.status
        if (response.status === 202 && request.sha256) {
          await response.body?.cancel()
          if (!live()) return
          // Accepted work is still pending, not a failed image. A large thumbnail queue can outlast the error retry budget.
          entry.tries = 0
          entry.pendingChecks = Math.min(5, (entry.pendingChecks ?? 0) + 1)
          entry.waiting = whenThumb(this.project, request.sha256, thumbWait(entry.pendingChecks), () => {
            entry.waiting = undefined
            this.pump()
          })
          return
        }
        if (!response.ok || response.status === 202) {
          stage = "http"
          await response.body?.cancel()
          throw new Error("Thumbnail is not available")
        }
        const blob = await response.blob()
        step("cozy.thumb.body")
        if (!live()) return
        blobBytes = blob.size
        this.sourceBytes += blobBytes
        this.fetchedBytes += blobBytes
        stage = "decode"
        raw = await createImageBitmap(blob)
        step("cozy.thumb.decode")
      }
      if (!live()) return
      entry.aspect = raw.width / raw.height
      this.decodedTotal += raw.width * raw.height * 4
      const width = Math.min(raw.width, Math.max(MIN_TEXTURE_WIDTH, request.width))
      bitmap = raw.width > width
        ? await createImageBitmap(raw, { resizeWidth: width, resizeHeight: Math.max(1, Math.round(width / entry.aspect)), resizeQuality: "high" })
        : raw
      if (bitmap !== raw) raw.close()
      step("cozy.thumb.resize")
      raw = undefined
      if (!live()) return
      // skipCache: no global reference survives project close/vault lock. Keep the bitmap for WebGL context restoration.
      stage = "texture"
      entry.texture = Texture.from(bitmap, true)
      step("cozy.thumb.texture")
      entry.bitmap = bitmap
      entry.bytes = bitmap.width * bitmap.height * 4
      this.residentBytes += entry.bytes
      bitmap = undefined
      entry.failed = false
      for (const changed of [...entry.listeners]) changed()
      this.invalidate()
      this.evict()
    } catch (error) {
      if (!live()) return
      entry.tries++
      const name = error instanceof Error ? error.name : "Error"
      this.lastFailure = { stage, httpStatus, errorName: /^[A-Za-z][A-Za-z0-9_.]{0,63}$/.test(name) ? name : "Error", attempt: entry.tries, width: entry.request.width, local: Boolean(entry.request.local) }
      if (entry.request.sha256 && entry.tries < THUMB_TRIES) {
        entry.waiting = whenThumb(this.project, entry.request.sha256, thumbWait(entry.tries), () => {
          entry.waiting = undefined
          this.pump()
        })
      } else {
        entry.failed = true
        for (const changed of [...entry.listeners]) changed()
        this.invalidate()
      }
    } finally {
      image?.removeAttribute("src")
      raw?.close()
      bitmap?.close()
      this.sourceBytes -= blobBytes
      this.active--
      if (entry.controller === controller) entry.controller = undefined
      this.pump()
    }
  }

  private discard(entry: Entry) {
    this.entries.delete(entry.key)
    if (this.unused.delete(entry.key)) this.unusedBytes -= entry.bytes
    this.residentBytes -= entry.bytes
    entry.controller?.abort()
    entry.waiting?.()
    entry.texture?.destroy(true)
    entry.bitmap?.close()
    entry.listeners.clear()
    entry.texture = undefined
    entry.bitmap = undefined
    entry.bytes = 0
  }

  private evict() {
    for (const entry of this.unused.values()) {
      if (this.residentBytes <= CACHE_BYTES && this.unusedBytes <= UNUSED_BYTES) break
      this.discard(entry)
    }
  }

  resourceStats() {
    let bytes = 0, pinnedBytes = 0, textures = 0, pinnedTextures = 0, failed = 0, queued = 0, minimumWidth = Infinity, minimumPinnedWidth = Infinity
    for (const entry of this.entries.values()) {
      bytes += entry.bytes
      if (entry.texture) { textures++; minimumWidth = Math.min(minimumWidth, entry.texture.width) }
      if (entry.pins) { pinnedBytes += entry.bytes; if (entry.texture) { pinnedTextures++; minimumPinnedWidth = Math.min(minimumPinnedWidth, entry.texture.width) } }
      if (entry.failed) failed++
      else if (entry.pins && !entry.texture && !entry.controller) queued++
    }
    return {
      textureBytes: bytes, decodedBytes: bytes, sourceBlobBytes: this.sourceBytes,
      pinnedBytes, unusedBytes: bytes - pinnedBytes, overflowBytes: Math.max(0, bytes - CACHE_BYTES),
      targetBytes: CACHE_BYTES, unusedTargetBytes: UNUSED_BYTES, textures, pinnedTextures,
      loading: this.active, queued, failed, fetchedBytes: this.fetchedBytes, decodedTotalBytes: this.decodedTotal,
      // Decoded and GPU byte estimates describe potentially overlapping storage; do not add them to process memory.
      minimumTextureWidth: textures ? minimumWidth : 0, minimumPinnedTextureWidth: pinnedTextures ? minimumPinnedWidth : 0,
      configuredMinimumTextureWidth: MIN_TEXTURE_WIDTH, preparedSourceWidth: THUMB_SOURCE_WIDTH,
      // No URL, response body, digest, token or arbitrary exception message enters diagnostics.
      lastFailure: this.lastFailure,
    }
  }

  destroy() {
    this.stopped = true
    for (const entry of [...this.entries.values()]) this.discard(entry)
  }
}
