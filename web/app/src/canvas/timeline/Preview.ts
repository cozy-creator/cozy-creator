// The Edit played in the page, clip after clip, with no render: one `<video>` for the box, under the same rations as any other, there only
// while it holds its slot. A frame shown stays when it lets go, drawn to a canvas, so a paused box holds no decoder and a hover opens nothing.
import type { Client } from "../../api/client"
import { holdMedia, playingHere } from "../../api/events"
import type { Playback } from "../../api/transport"
import { signal } from "../../ui/signal"
import { clipAt, isStill, type Layout, type Placed } from "./model"

/** A pause or a scrub keeps its slot this long, for the next seek. */
const LET_GO_MS = 1500

/** A frame's middle: exact at the edit's rate, and nearest at any other. */
const timeOf = (place: Placed, frame: number, fps: number) => (place.in + (frame - place.at) + 0.5) / fps

export class Preview {
  readonly frame = signal(0)
  readonly playing = signal(false)
  /** Which clip is up, by its index. */
  readonly showing = signal<number | null>(null)
  /** A `<video>` holds a slot. */
  readonly live = signal(false)
  /** The selected clip has a decoded frame, rather than only a slot or metadata. */
  readonly ready = signal(false)
  /** What the canvas holds: which file, at which second. */
  readonly kept = signal<{ path: string; at: number } | null>(null)
  readonly failed = signal<string | null>(null)
  private client: Client
  private projectId: string | null
  private layout: Layout = { placed: [], frames: 0, scale: 1, width: 0 }
  private fps = 24
  private video: HTMLVideoElement | null = null
  private holder: HTMLElement | null = null
  private still: HTMLCanvasElement | null = null
  private playback: Playback | null = null
  private loaded: string | null = null
  private release: (() => void) | null = null
  private waiting: AbortController | null = null
  /** The ask for a slot in line: a seek while it waits waits on it too. */
  private holding: Promise<boolean> | null = null
  private wanted: number | null = null
  private turn = 0
  private raf = 0
  private began = 0
  private letting: ReturnType<typeof setTimeout> | undefined
  /** Whatever's waiting for a file's metadata stops waiting. */
  private gaveUp: (() => void) | null = null

  constructor(client: Client, projectId: string | null) {
    this.client = client
    this.projectId = projectId
  }

  attach(holder: HTMLElement | null, still: HTMLCanvasElement | null) {
    this.holder = holder
    this.still = still
    if (!holder) this.stop()
  }

  private open(holder: HTMLElement): HTMLVideoElement {
    if (this.video) return this.video
    const video = document.createElement("video")
    video.preload = "auto"
    video.playsInline = true
    video.onseeked = () => this.seeked(video)
    video.onloadeddata = () => this.seeked(video)
    video.onplaying = () => this.seeked(video)
    video.onended = () => {
      if (video !== this.video) return
      const place = this.place()
      if (this.playing.get() && place) this.after(place)
    }
    video.onerror = () => {
      if (video !== this.video) return
      // Bytes this browser can't decode, it can't. Bytes that didn't come are Calcifer's to fix by coming back.
      if (video.getAttribute("src") && video.error?.code !== MediaError.MEDIA_ERR_NETWORK) this.failed.set("This browser can't play it")
    }
    holder.append(video)
    this.video = video
    return video
  }

  edit(layout: Layout, fps: number) {
    this.layout = layout
    this.fps = fps
    const last = Math.max(0, layout.frames - 1)
    if (this.frame.get() > last) this.frame.set(last)
    if (!this.playing.get()) this.showing.set(clipAt(layout, this.frame.get())?.index ?? null)
  }

  private place(): Placed | undefined {
    return this.layout.placed[this.showing.get() ?? -1]
  }

  /** Shows this frame exactly, the playhead there at once. */
  seek(frame: number) {
    if (this.playing.get()) this.pause()
    this.keep()
    this.ready.set(false)
    const turn = ++this.turn
    this.wanted = null
    const at = Math.max(0, Math.min(Math.max(0, this.layout.frames - 1), frame))
    this.frame.set(at)
    const place = clipAt(this.layout, at)
    this.showing.set(place?.index ?? null)
    this.failed.set(null)
    clearTimeout(this.letting)
    if (!place?.clip.path || isStill(place.clip)) return this.letGoSoon()
    this.wanted = timeOf(place, at, this.fps)
    void this.load(place.clip.path).then((ready) => {
      if (!ready || turn !== this.turn) return
      if (this.video && !this.video.seeking && this.wanted !== null) this.video.currentTime = this.wanted
    })
  }

  toggle() {
    if (this.playing.get()) this.pause()
    else this.play()
  }

  /** From the playhead, or from the start once it's at the end. */
  play() {
    if (!this.layout.frames) return
    const from = this.frame.get() >= this.layout.frames - 1 ? 0 : this.frame.get()
    const place = clipAt(this.layout, from)
    if (!place) return
    clearTimeout(this.letting)
    this.failed.set(null)
    this.playing.set(true)
    this.enter(place, from)
  }

  pause() {
    this.playing.set(false)
    this.turn += 1
    cancelAnimationFrame(this.raf)
    this.video?.pause()
    this.keep()
    this.letGoSoon()
  }

  /** The clip's frame, then on from there. A picture has only time to count. */
  private enter(place: Placed, frame: number) {
    // Replacing a video's source clears its drawable frame. Preserve it before
    // changing clips, including when the incoming clip needs network or a seek.
    this.keep()
    this.ready.set(false)
    const turn = ++this.turn
    this.showing.set(place.index)
    this.frame.set(frame)
    cancelAnimationFrame(this.raf)
    if (isStill(place.clip) || !place.clip.path) {
      this.video?.pause()
      this.wanted = null
      this.began = performance.now() - ((frame - place.at) / this.fps) * 1000
      this.raf = requestAnimationFrame(this.tick)
      return
    }
    this.wanted = timeOf(place, frame, this.fps)
    void this.load(place.clip.path).then((ready) => {
      const video = this.video
      if (!ready || !video || turn !== this.turn) return
      if (this.wanted !== null) video.currentTime = this.wanted
      video.play().catch(() => {})
      this.raf = requestAnimationFrame(this.tick)
    })
  }

  private tick = () => {
    const place = this.place()
    if (!place || !this.playing.get()) return
    const frame = isStill(place.clip) || !place.clip.path
      ? place.at + Math.floor(((performance.now() - this.began) / 1000) * this.fps)
      : place.at + Math.floor((this.video?.currentTime ?? 0) * this.fps + 1e-3) - place.in
    if (frame >= place.at + place.frames) return this.after(place)
    this.frame.set(Math.max(place.at, frame))
    this.raf = requestAnimationFrame(this.tick)
  }

  private after(place: Placed) {
    const next = this.layout.placed.slice(place.index + 1).find((held) => held.frames > 0)
    if (next) return this.enter(next, next.at)
    this.frame.set(Math.max(0, this.layout.frames - 1))
    this.pause()
  }

  /** This file in the `<video>`, its slot held: false if it was let go while waiting, or can't be played. */
  private async load(path: string): Promise<boolean> {
    if (!this.holder || !this.projectId || !(await this.hold()) || !this.holder) return false
    const video = this.open(this.holder)
    if (this.loaded !== path) {
      this.playback?.stop()
      this.loaded = path
      this.playback = this.client.transport.play(this.client.file(this.projectId, path), video)
    }
    if (video.readyState >= HTMLMediaElement.HAVE_METADATA) return true
    return new Promise((done) => {
      const settle = (ready: boolean) => {
        video.removeEventListener("loadedmetadata", yes)
        video.removeEventListener("error", no)
        this.gaveUp = null
        done(ready)
      }
      const yes = () => settle(true)
      const no = () => settle(false)
      this.gaveUp?.()
      this.gaveUp = no
      video.addEventListener("loadedmetadata", yes)
      video.addEventListener("error", no)
    })
  }

  private hold(): Promise<boolean> {
    if (this.release) return Promise.resolve(true)
    if (this.holding) return this.holding
    const waiting = new AbortController()
    this.waiting = waiting
    if (playingHere.length >= 2) playingHere[0]()
    playingHere.push(this.stop)
    const holding: Promise<boolean> = holdMedia(waiting.signal).then((release) => {
      if (waiting.signal.aborted) {
        release()
        return false
      }
      this.release = release
      this.waiting = null
      this.live.set(true)
      return true
    }, () => false).finally(() => {
      if (this.holding === holding) this.holding = null
    })
    this.holding = holding
    return holding
  }

  private seeked(video: HTMLVideoElement) {
    if (video !== this.video || !this.holder || video.seeking || video.readyState < HTMLMediaElement.HAVE_CURRENT_DATA) return
    const place = this.place()
    if (!place || isStill(place.clip) || place.clip.path !== this.loaded) return
    if (this.wanted !== null && Math.abs(video.currentTime - this.wanted) > 1e-3) {
      video.currentTime = this.wanted
      return
    }
    this.wanted = null
    this.ready.set(true)
    this.keep()
    if (!this.playing.get()) this.letGoSoon()
  }

  private keep() {
    const { video, still, loaded } = this
    const context = still?.getContext("2d")
    const place = this.place()
    if (!this.ready.get() || !place || isStill(place.clip) || place.clip.path !== loaded || !video || !still || !context || !loaded || !video.videoWidth || video.seeking || video.readyState < HTMLMediaElement.HAVE_CURRENT_DATA) return
    still.width = video.videoWidth
    still.height = video.videoHeight
    context.drawImage(video, 0, 0)
    this.kept.set({ path: loaded, at: video.currentTime })
  }

  /** A still arrived: retain its real pixels for a later clip's handoff too. */
  picture(index: number, sha256: string, image: HTMLImageElement) {
    const place = this.place()
    const still = this.still
    const context = still?.getContext("2d")
    if (!this.holder || !still || !context || place?.index !== index || place.clip.sha256 !== sha256 || !isStill(place.clip) || !image.complete || !image.naturalWidth) return
    still.width = image.naturalWidth
    still.height = image.naturalHeight
    context.drawImage(image, 0, 0)
    this.kept.set({ path: place.clip.path ?? place.clip.sha256 ?? "", at: 0 })
    this.ready.set(true)
  }

  private letGoSoon() {
    clearTimeout(this.letting)
    if (this.release || this.waiting) this.letting = setTimeout(this.stop, LET_GO_MS)
  }

  /** Lets go of the slot, whoever asks: another player, or the box leaving the screen. */
  stop = () => {
    clearTimeout(this.letting)
    if (this.playing.get()) {
      this.playing.set(false)
      this.keep()
    }
    this.turn += 1
    cancelAnimationFrame(this.raf)
    this.waiting?.abort()
    this.waiting = null
    this.holding = null
    this.gaveUp?.()
    this.playback?.stop()
    this.playback = null
    this.video?.remove()
    this.video = null
    this.loaded = null
    this.wanted = null
    this.release?.()
    this.release = null
    this.live.set(false)
    this.ready.set(false)
    const at = playingHere.indexOf(this.stop)
    if (at >= 0) playingHere.splice(at, 1)
  }
}
