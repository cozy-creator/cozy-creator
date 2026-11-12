import { useQuery } from "@tanstack/react-query"
import { type CSSProperties, type PointerEvent, type ReactNode, useEffect, useLayoutEffect, useRef, useState } from "react"
import { parseLink, Player as MachinePlayer } from "../../../player/cozy-webrtc.js"
import type { Client } from "../api/client"
import { runFullQuery, runQuery } from "../api/queries"
import { holdMedia, playingHere } from "../api/events"
import type { Playback } from "../api/transport"
import { useConnection } from "../app/Connection"
import type { MainFile } from "../api/calcifer"
import { useCanvasStore } from "./context"
import type { ElementView } from "./CanvasStore"
import { fieldsOf } from "./controls"
import { Icon, type IconName } from "../ui/Icon"
import { useSignal } from "../ui/signal"
import { aspectOf, faceOf, failureOf, hexOf, makes, makingRun, type Shown, shownOf, valueIn, valuesOf, watched } from "./shown"
import { chipWords, clock, lineOf, mentionsIn, withMentions } from "./text"
import { isSelf } from "./versions"
import { OrbitView, orbitOf } from "./Orbit"
import { BufferedImage } from "./BufferedImage"
import { useArriving } from "./RunFace"
import { bodyText, CopyError } from "../ui/CopyError"

const STRIP_FRAMES = 8
/** How long the pointer rests on a video before its preview plays: one passing over on its way somewhere starts nothing. */
const PREVIEW_AFTER_MS = 250

/**
 * At most two different thumbnails asked at once. Calcifer makes few at a time, and `<img>`s waiting on it would fill the browser's six
 * connections to it, stalling every other call. Boxes drawing the same request share its place in line. An element scrolled away gives up
 * its own place. What a person is looking at right now, `now`, goes at once, past the line.
 */
function line(limit: number) {
  type Start = (complete: () => void) => void
  type Load = { path: string; subscribers: Set<() => void>; running: boolean }
  const loads = new Map<string, Load>()
  const waiting = new Set<Load>()
  let running = 0

  const complete = (load: Load) => {
    if (loads.get(load.path) !== load) return
    loads.delete(load.path)
    load.subscribers.clear()
    if (load.running) running -= 1
    else waiting.delete(load)
    next()
  }
  const begin = (load: Load) => {
    waiting.delete(load)
    load.running = true
    running += 1
    for (const start of load.subscribers) start()
  }
  const next = () => {
    for (const load of waiting) {
      if (running >= limit) break
      begin(load)
    }
  }

  return (path: string, start: Start, now = false): (() => void) => {
    let load = loads.get(path)
    if (!load) {
      load = { path, subscribers: new Set(), running: false }
      loads.set(path, load)
      waiting.add(load)
    }
    const joined = load
    const ready = () => start(() => {
      if (joined.subscribers.has(ready)) complete(joined)
    })
    joined.subscribers.add(ready)
    if (joined.running) ready()
    else if (now) begin(joined)
    next()

    return () => {
      joined.subscribers.delete(ready)
      if (!joined.subscribers.size) complete(joined)
    }
  }
}

const thumbLine = line(2)
/** What's been up: coming back on screen is no arrival, and fades nothing. */
const arrived = new Set<string>()

/** What an open box draws: Calcifer's widest, never wider than the file, so a picture at its own resolution. */
const FULL = 2048

/** A just-landed take's picture, at the size an open box draws it, asked once and decoded ahead, so a click on it shows it at once. */
const preloaded = new Map<string, HTMLImageElement>()
const mediaEpoch = new Map<string, number>()
const mediaKey = (project: string | null, key: string) => `${project ?? ""}\0${key}`
export function revokeProjectMedia(project: string) {
  mediaEpoch.set(project, (mediaEpoch.get(project) ?? 0) + 1)
  const prefix = `${project}\0`
  for (const [key, image] of preloaded) if (key.startsWith(prefix)) { if (image.src.startsWith("blob:")) URL.revokeObjectURL(image.src); image.removeAttribute("src"); preloaded.delete(key) }
  for (const key of arrived) if (key.startsWith(prefix)) arrived.delete(key)
  for (const key of widest.keys()) if (key.startsWith(prefix)) widest.delete(key)
}
export function preload(client: Client, projectId: string | null, sha256: string) {
  const key = mediaKey(projectId, `${sha256}:${FULL}`)
  if (!projectId || preloaded.has(key)) return
  const epoch = mediaEpoch.get(projectId) ?? 0
  const img = new Image()
  preloaded.set(key, img)
  const live = () => preloaded.get(key) === img && (mediaEpoch.get(projectId) ?? 0) === epoch
  void client.transport.src(client.thumb(projectId, sha256, FULL)).then((url) => {
    if (!live()) { if (url.startsWith("blob:")) URL.revokeObjectURL(url); return }
    img.src = url
    void img.decode().then(() => { if (live()) arrived.add(key) }, () => { if (live()) preloaded.delete(key) })
  }, () => { if (live()) preloaded.delete(key) })
}

/** How often a thumbnail that isn't there is asked again (1 s, 2 s, 4 s, 8 s, 16 s), as its file may still be arriving. */
const THUMB_TRIES = 5

/** Thumbnails come in these widths, so a file has few sizes in Calcifer's cache: as wide as it's drawn on screen, the canvas's zoom and the
 *  screen's density in, up to Calcifer's widest, which is never wider than the file. */
function thumbWidth(cardWidth: number, zoom = 1): number {
  const wanted = cardWidth * zoom * Math.min(2, window.devicePixelRatio || 1)
  return wanted <= 256 ? 256 : wanted <= 512 ? 512 : wanted <= 1024 ? 1024 : FULL
}
/** The widest of each file drawn yet: zooming out keeps it rather than asking a smaller one. */
const widest = new Map<string, number>()

export function Thumb({
  sha256,
  cardWidth,
  strip,
  wave,
  exact,
  className,
  frame,
  now,
  onSize,
  onShown,
}: {
  sha256: string
  cardWidth: number
  strip?: number
  /** A video's sound as a waveform, rather than its picture. */
  wave?: boolean
  /** `cardWidth` is the size wanted, not a card's: a small strip stays small. */
  exact?: boolean
  className?: string
  frame?: number
  /** What a person is looking at: past the line. */
  now?: boolean
  onSize?: (aspect: number) => void
  onShown?: () => void
}) {
  const { client } = useConnection()
  const store = useCanvasStore()
  const [image, setImage] = useState<{ path: string; src: string; request: number; complete: () => void; live: () => boolean }>()
  const nextRequest = useRef(0)
  const [failedPath, setFailedPath] = useState<string>()
  const projectId = store.projectId
  const zoom = useSignal(store.sharpness)
  const width = exact ? cardWidth : Math.max(thumbWidth(cardWidth, zoom), strip ? 0 : (widest.get(mediaKey(projectId, sha256)) ?? 0))
  const base = projectId ? client.thumb(projectId, sha256, width, strip, wave) : undefined
  // A file still on its way here has none yet (404 file_missing): asked again, later each time, before it says it has none.
  const [tries, setTries] = useState<{ base?: string; n: number }>({ n: 0 })
  const attempt = tries.base === base ? tries.n : 0
  const path = base && attempt ? `${base}&try=${attempt}` : base
  const retry = useRef<number | undefined>(undefined)
  useEffect(() => () => clearTimeout(retry.current), [])
  const failed = (at: string) => {
    if (attempt >= THUMB_TRIES) return setFailedPath(at)
    setImage(undefined)
    retry.current = window.setTimeout(() => setTries({ base, n: attempt + 1 }), 1000 * 2 ** attempt)
  }
  useEffect(() => {
    if (!path) return
    let live = true
    const epoch = projectId ? mediaEpoch.get(projectId) ?? 0 : 0
    const current = () => live && (!projectId || (mediaEpoch.get(projectId) ?? 0) === epoch)
    const request = ++nextRequest.current
    const leave = thumbLine(path, (complete) => {
      void client.transport.src(path).then((src) => {
        if (!current()) { if (src.startsWith("blob:")) URL.revokeObjectURL(src); return }
        setFailedPath(undefined)
        setImage({ path, src, request, complete, live: current })
      }, () => {
        if (!live) return
        leave()
        failed(path)
      })
    }, now)
    return () => {
      live = false
      leave()
    }
  }, [client, path])
  // A strip that can't be made leaves the poster under it as it was.
  if (path && failedPath === path) return strip || wave ? null : <span className="media-note">No preview</span>
  if (!image || failedPath === image.path) return null
  return (
    <img
      key={image.request}
      src={image.src}
      alt=""
      className={`${className ?? ""} ${arrived.has(mediaKey(projectId, `${sha256}:${width}`)) ? "" : "arrives"}`}
      draggable={false}
      style={frame === undefined ? undefined : { transform: `translateX(${(-frame * 100) / (strip ?? 1)}%)` }}
      onError={() => {
        if (image.path !== path || !image.live()) return
        image.complete()
        failed(image.path)
      }}
      onLoad={(event) => {
        if (image.path !== path || !image.live()) return
        image.complete()
        const { naturalWidth, naturalHeight } = event.currentTarget
        if (naturalWidth && naturalHeight) onSize?.(naturalWidth / naturalHeight)
        arrived.add(mediaKey(projectId, `${sha256}:${width}`))
        if (!strip && !wave) widest.set(mediaKey(projectId, sha256), Math.max(width, widest.get(mediaKey(projectId, sha256)) ?? 0))
        onShown?.()
      }}
    />
  )
}

/**
 * Whether a video's preview plays: once the pointer has rested on it a moment. Only a pointer that moves counts, as a pan slides tiles under
 * one that doesn't; a finger has nothing to rest, and a held button is a drag. A wheel ends it, and so does a press, until the pointer leaves.
 */
export function usePreview(video: boolean) {
  const [on, setOn] = useState(false)
  const resting = useRef<number | undefined>(undefined)
  const pressed = useRef(false)
  useEffect(() => () => clearTimeout(resting.current), [])
  const end = () => {
    if (resting.current === undefined) return
    clearTimeout(resting.current)
    resting.current = undefined
    setOn(false)
  }
  const leave = {
    onPointerLeave() {
      pressed.current = false
      end()
    },
  }
  if (!video) return { on: false, hover: leave }
  return {
    on,
    hover: {
      ...leave,
      onPointerMove(event: PointerEvent<HTMLElement>) {
        if (event.pointerType === "touch" || event.buttons || pressed.current || resting.current !== undefined) return
        resting.current = window.setTimeout(() => setOn(true), PREVIEW_AFTER_MS)
      },
      onPointerDown() {
        pressed.current = true
        end()
      },
      // Seen on the way down: the canvas's pan takes the wheel before it could come back up.
      onWheelCapture: end,
    },
  }
}

/** Previews that couldn't play, by hash, and until when a hover doesn't ask again: a file still arriving has one soon. */
const unpreviewable = new Map<string, number>()
const PREVIEW_TRIES = 4
const PREVIEW_AGAIN_MS = 30_000

/**
 * A video's preview over its poster: a few seconds from the poster's frame on, silent and looping, which Calcifer made once as the file
 * landed. Nothing loads before the hover, it shows from its first frame, and it goes with the hover or with its tile.
 */
export function Preview({ sha256 }: { sha256: string }) {
  const { client } = useConnection()
  const store = useCanvasStore()
  const [src, setSrc] = useState<string>()
  const [playing, setPlaying] = useState(false)
  const [attempt, setAttempt] = useState(0)
  const element = useRef<HTMLVideoElement>(null)
  const retry = useRef<number | undefined>(undefined)
  useEffect(() => () => clearTimeout(retry.current), [])
  const projectId = store.projectId
  useEffect(() => {
    if (!projectId || (unpreviewable.get(sha256) ?? 0) > Date.now()) return
    let live = true
    const path = client.preview(projectId, sha256)
    void client.transport.src(attempt ? `${path}&try=${attempt}` : path).then((url) => {
      if (live) setSrc(url)
    })
    return () => {
      live = false
    }
  }, [client, projectId, sha256, attempt])
  // Its bytes and decoder go with it, not when it's collected.
  useEffect(() => {
    const el = element.current
    if (!el || !src) return
    el.src = src
    el.play().catch(() => {})
    return () => {
      el.removeAttribute("src")
      el.load()
    }
  }, [src])
  if (!src) return null
  return (
    <video
      ref={element}
      className={`preview ${playing ? "on" : ""}`}
      aria-hidden
      muted
      loop
      playsInline
      disablePictureInPicture
      onPlaying={() => setPlaying(true)}
      onError={(event) => {
        if (event.currentTarget.error?.code === MediaError.MEDIA_ERR_NETWORK) return
        setSrc(undefined)
        // Made on its first ask, or its file still arriving: asked again while the pointer rests, then not for a while.
        if (attempt + 1 < PREVIEW_TRIES) retry.current = window.setTimeout(() => setAttempt(attempt + 1), 1000 * 2 ** attempt)
        else unpreviewable.set(sha256, Date.now() + PREVIEW_AGAIN_MS)
      }}
    />
  )
}

/** A video's frames walked across, one picture from Calcifer, so no `<video>` opens until it's clicked. */
export function WalkedFrames({ sha256, cardWidth, at }: { sha256: string; cardWidth: number; at: number }) {
  return <Thumb sha256={sha256} cardWidth={cardWidth} strip={STRIP_FRAMES} frame={Math.min(STRIP_FRAMES - 1, Math.floor(at * STRIP_FRAMES))} className="strip" />
}

/** A video's poster, its preview on a hover, and the frame a scrub is at. */
function Poster({ shown, cardWidth, name, scrubbed, previewing }: { shown: Shown; cardWidth: number; name: string; scrubbed?: number; previewing: boolean }) {
  const store = useCanvasStore()
  const at = scrubbed !== undefined && shown.duration_ms ? (scrubbed * 1000) / shown.duration_ms : null
  return (
    <div className="scrub">
      <Thumb sha256={shown.sha256} cardWidth={cardWidth} onSize={(aspect) => store.setAspect(name, aspect)} onShown={() => store.showThumb(name)} />
      {at !== null ? <WalkedFrames sha256={shown.sha256} cardWidth={cardWidth} at={at} /> : previewing && <Preview sha256={shown.sha256} />}
      {at !== null && shown.duration_ms ? <span className="tc">{clock(at * shown.duration_ms)}</span> : null}
    </div>
  )
}

/** What a click plays: a file here over Range, or from the machine that has it or is still making it. */
type Source = { path: string } | { run: number; output: string }

// A project's file, by its hash. Anything else from a state isn't followed.
const FILE = /^files\/[0-9a-f]{64}(\.\w+)?$/

/** Its bytes are in the project's files/, which the file route serves, so a person can download it. */
export const downloadable = (file: Pick<MainFile, "path" | "absent">) => !file.absent && FILE.test(file.path)

export function sourceOf(view: ElementView, shown: Shown | null): Source | null {
  const asset = view.asset
  if (!asset || view.placing) return null
  if (!shown && asset.kind === "video") {
    const run = watched(view)?.run ?? (view.viewed === undefined ? asset.making?.[0] : undefined)
    // What of it landed here plays on its glimpse; before any has, straight from its machine.
    if (run) return view.glimpse?.media === "video" && view.glimpse.number === run ? null : { run, output: "video" }
  }
  if (shown?.field !== "video" && shown?.field !== "audio") return null
  if (!shown.absent) return FILE.test(shown.path) ? { path: shown.path } : null
  return shown.machine && shown.run ? { run: shown.run, output: shown.output } : null
}

/** Straight from the machine: `play` says where and signs a cap for this output alone, and the shared browser player follows it. */
async function fromMachine(client: Client, source: { project: string; run: number; output: string }, el: HTMLMediaElement, signal: AbortSignal, failed: (why: string) => void): Promise<Playback> {
  const [name, index] = source.output.split("/")
  const answer = await client.play({ project: source.project, run: source.run }, name, Number(index ?? 0))
  signal.throwIfAborted()
  const player = new MachinePlayer(el, parseLink(new URL(answer.url).hash))
  let stopping = false
  player.addEventListener("status", (event) => {
    const { state, detail } = (event as CustomEvent<{ state: string; detail: string }>).detail
    if (state === "error" && !stopping) failed(detail)
  })
  return {
    stop() {
      stopping = true
      player.close()
      el.pause()
      el.removeAttribute("src")
      el.load()
    },
  }
}

/** What a double-click can show large: the file itself, here or on its machine, and nothing that's only words. */
export function viewable(view: ElementView | undefined): boolean {
  const shown = view ? shownOf(view) : null
  if (!view || !shown || view.hereWords) return false
  return shown.field === "image" ? !shown.absent && FILE.test(shown.path) : shown.field === "video" && Boolean(sourceOf(view, shown))
}

/** Played on click, or at once with `auto`, one of the two live ones across tabs: a file Calcifer serves, by its API path, or one straight from a machine. */
export function Player({ field, path, machine, seek, auto, from, onTime }: { field: string; path?: string; machine?: { project: string; run: number; output: string }; seek?: ElementView["seek"]; auto?: boolean; from?: number; onTime?(seconds: number): void }) {
  const { client, stream } = useConnection()
  const [state, setState] = useState<"idle" | "waiting" | "playing" | "unplayable">("idle")
  const [why, setWhy] = useState("This browser can't play it")
  const element = useRef<HTMLVideoElement & HTMLAudioElement>(null)
  const stopRef = useRef<() => void>(() => {})
  const wanted = useRef<number | null>(from ?? null)
  const asked = useRef(seek?.ask ?? 0)

  useEffect(() => () => stopRef.current(), [])
  // Asked to play at once: once it's there, so React's second mount in development doesn't stop it.
  useEffect(() => {
    if (auto) setState("waiting")
  }, [auto])

  // Calcifer came back: what was playing reloads where it was.
  useEffect(
    () =>
      stream.subscribe((event) => {
        const el = element.current
        if (event.type !== "hello" || !el?.getAttribute("src") || !el.error) return
        const at = el.currentTime
        el.load()
        el.currentTime = at
        el.play().catch(() => {})
      }),
    [stream],
  )

  // A piece of the cut was clicked: from there, starting if it hasn't.
  useEffect(() => {
    if (!seek || seek.ask === asked.current) return
    asked.current = seek.ask
    wanted.current = seek.at
    if (element.current) element.current.currentTime = seek.at
    setState((now) => (now === "idle" ? "waiting" : now))
  }, [seek])

  useEffect(() => {
    if (state !== "waiting") return
    const waiting = new AbortController()
    let release: (() => void) | null = null
    let playback: Playback | null = null
    const failed = (reason: string) => {
      setWhy(reason)
      setState("unplayable")
    }
    const stop = () => {
      waiting.abort()
      playback?.stop()
      release?.()
      const at = playingHere.indexOf(stop)
      if (at >= 0) playingHere.splice(at, 1)
      setState((now) => (now === "unplayable" ? now : "idle"))
    }
    stopRef.current = stop
    if (playingHere.length >= 2) playingHere[0]()
    playingHere.push(stop)
    holdMedia(waiting.signal)
      .then(async (held) => {
        release = held
        const el = element.current
        if (!el) return stop()
        playback = path !== undefined ? client.transport.play(path, el) : await fromMachine(client, machine!, el, waiting.signal, failed)
        if (waiting.signal.aborted) return playback.stop()
        setState("playing")
        if (wanted.current !== null) el.currentTime = wanted.current
        wanted.current = null
        el.play().catch(() => {})
      })
      .catch((error: unknown) => {
        if (!waiting.signal.aborted) failed(error instanceof Error ? error.message : String(error))
      })
  }, [state, client, path, machine?.project, machine?.run, machine?.output])

  if (state === "unplayable") return <span className="media-note over">{why}</span>
  if (state === "idle") {
    return (
      <button type="button" className="play nodrag" aria-label="Play" onClick={() => setState("waiting")}>
        <Icon name="play" />
      </button>
    )
  }
  const Tag = field === "audio" ? "audio" : "video"
  return (
    <>
      <Tag
        ref={element}
        className={`live nodrag nopan nowheel ${field}`}
        controls
        preload="none"
        onTimeUpdate={(event) => onTime?.(event.currentTarget.currentTime)}
        onSeeked={(event) => onTime?.(event.currentTarget.currentTime)}
        onEnded={() => stopRef.current()}
        onError={() => {
          const el = element.current
          // Bytes that didn't come are Calcifer's to fix by coming back. Bytes this browser can't decode, it can't.
          if (path !== undefined && el?.getAttribute("src") && el.error?.code !== MediaError.MEDIA_ERR_NETWORK && stream.health.get().state === "live") setState("unplayable")
        }}
      />
      {/* A stop, not ✕: in a corner ✕ reads as close or delete (Paul, 11-12). */}
      <button type="button" className="stop nodrag" aria-label="Stop" title="Stop" onClick={() => stopRef.current()}>
        <Icon name="stop" />
      </button>
    </>
  )
}

const GLYPHS: Record<string, IconName> = { image: "picture", video: "video", audio: "sound" }

/** How far it's got, on the picture: a veil that moves and never reflows, and the one line it says. An Edit rendered here is a progress bar. */
function Progress({ veil = true, fraction, says, phase, rendering }: Way) {
  return (
    <div
      className={`prog ${fraction === undefined ? "unknown" : ""} ${phase ?? ""}`}
      {...(rendering ? { role: "progressbar", "aria-label": "Rendering", "aria-valuenow": Math.round((fraction ?? 0) * 100) } : {})}
    >
      {veil && <i style={fraction === undefined ? undefined : { transform: `translateX(${Math.round(fraction * 100) - 100}%)` }} />}
      {says && <b>{says}</b>}
    </div>
  )
}

/** How far it's got, in one line: an import placed, or an Edit rendered here. */
interface Way {
  veil?: boolean
  fraction?: number
  phase?: string
  says?: string
  rendering?: boolean
}

/** How often a rendering box asks its run's machine for a preview while none comes: the machine makes them only while asked. */
const PREVIEW_ASK_MS = 8000

/** What its run is making, faded under how far it's got (cr-282): the newest picture its machine made of the latents. Asked for as
 *  each new one is made, and every few seconds while none is, which is what keeps the machine making them; an answer with no new
 *  one leaves the last in place. The take replaces it. */
function LatentPreview({ view, number }: { view: ElementView; number: number }) {
  const { client } = useConnection()
  const store = useCanvasStore()
  const project = store.projectId
  const later = number !== view.run?.number
  const { data } = useQuery({ ...runQuery(client, project ?? "", number), enabled: Boolean(project && later) })
  const run = later ? data : view.run
  const running = run?.status === "in_progress" && run.machine_state !== "queued"
  const newest = run?.progress?.preview ?? 0
  const [ask, setAsk] = useState(0)
  const [shown, setShown] = useState<{ url: string; tag: string }>()
  const held = useRef<{ url: string; tag: string }>(undefined)
  useEffect(() => {
    if (running) setAsk((n) => n + 1)
  }, [running, newest])
  useEffect(() => {
    if (!running) return
    const timer = setInterval(() => setAsk((n) => n + 1), PREVIEW_ASK_MS)
    return () => clearInterval(timer)
  }, [running])
  useEffect(() => {
    if (!project || !running || !ask) return
    const epoch = mediaEpoch.get(project) ?? 0
    const asked = new AbortController()
    const tag = held.current?.tag
    void (async () => {
      const response = await client.transport.fetch(client.runPreview({ project, run: number }), { signal: asked.signal, headers: tag ? { "If-None-Match": tag } : {} })
      const now = response.headers.get("etag")
      if (response.status !== 200 || !now || now === tag) return
      const blob = await response.blob()
      if (asked.signal.aborted || !store.active || store.projectId !== project || (mediaEpoch.get(project) ?? 0) !== epoch) return
      const picture = { url: URL.createObjectURL(blob), tag: now }
      if (held.current) URL.revokeObjectURL(held.current.url)
      held.current = picture
      setShown(picture)
    })().catch(() => {})
    return () => asked.abort()
  }, [client, store, project, number, running, ask])
  useEffect(() => {
    setShown(undefined)
    return () => {
      if (held.current) URL.revokeObjectURL(held.current.url)
      held.current = undefined
    }
  }, [project, number])
  if (!shown || !project || !store.active) return null
  return (
    <div className="latent-preview" data-preview={shown.tag}>
      <img src={shown.url} alt="" draggable={false} />
    </div>
  )
}

/** The picture in a box's frame, one take's alone. At rest, as wide as it's drawn, in line with the rest. Open, its small thumbnail at once, the one
 *  its tile and its box at rest already have, scaled up, and its own resolution over it as soon as that's decoded. Another take's picture is
 *  never what stands in. */
function FramePicture({ sha256, open, restWidth, onSize, onShown }: { sha256: string; open: boolean; restWidth: number; onSize: (aspect: number) => void; onShown: () => void }) {
  const [full, setFull] = useState(false)
  if (full && !open) setFull(false)
  return (
    <>
      {open && !full && <Thumb sha256={sha256} cardWidth={restWidth} now />}
      <Thumb
        sha256={sha256}
        cardWidth={open ? FULL : restWidth}
        exact={open}
        now={open}
        onSize={onSize}
        onShown={() => {
          if (open) setFull(true)
          onShown()
        }}
      />
    </>
  )
}

/** Words as a frame draws them, read only: each mention a chip, struck through when it names nothing. */
function Said({ words, view, name }: { words: string; view: ElementView; name: string }) {
  const store = useCanvasStore()
  return (
    <p className="words">
      {withMentions(words).map((part, i) =>
        part.mention ? (
          <span key={i} className={`chip ${view.asset?.missing?.includes(part.text.slice(1, -1)) || (store.mentionOf(part.text.slice(1, -1), name)?.gone ?? isSelf(part.text.slice(1, -1))) ? "missing" : ""}`}>
            {chipWords(part.text.slice(1, -1))}
          </span>
        ) : (
          part.text
        ),
      )}
    </p>
  )
}

/** Behind its words, faded, the pictures they mention, or `mentions`: one fills the frame, up to four tile it 2×2, up to nine 3×3. */
export function Ghosts({ name, view, mentions, cardWidth = 960 }: { name: string; view: ElementView; mentions?: string[]; cardWidth?: number }) {
  const store = useCanvasStore()
  useSignal(store.names)
  useSignal(store.tools)
  const pictures = view.asset ? store.mentionedPictures(name, mentions) : []
  const element = useRef<HTMLDivElement>(null)
  const [frame, setFrame] = useState<{ width: number; height: number }>()
  useLayoutEffect(() => {
    if (view.asset?.kind !== "image" || frame || !element.current) return
    const held = element.current
    const measure = () => {
      if (held.clientWidth > 0 && held.clientHeight > 0) setFrame({ width: held.clientWidth, height: held.clientHeight / 2 })
    }
    measure()
    const observer = new ResizeObserver(measure)
    observer.observe(held)
    return () => observer.disconnect()
  }, [view.asset?.kind, Boolean(pictures.length), frame])
  if (!pictures.length) return null
  const side = Math.ceil(Math.sqrt(pictures.length))
  return (
    <div ref={element} className={`ghosts ${frame ? "fixed-image-references" : ""}`} data-image-reference={view.asset?.kind === "image" ? "" : undefined} style={{ "--side": side, ...(frame ? { width: frame.width, height: frame.height } : {}) } as CSSProperties} aria-hidden>
      {pictures.map((ghost) =>
        ghost.sha256 ? (
          <Thumb key={ghost.mention} sha256={ghost.sha256} cardWidth={cardWidth / side} />
        ) : (
          <svg key={ghost.mention} className={`ghost-blank ${ghost.making ? "making" : ""}`} data-mention={ghost.mention} viewBox={`0 0 ${100 * (ghost.shape ?? 1)} 100`} preserveAspectRatio="xMidYMid meet">
            <rect x="2" y="2" width={100 * (ghost.shape ?? 1) - 4} height="96" rx="3" />
          </svg>
        ),
      )}
    </div>
  )
}

/** A run's words in a box's prompt: what the binder sent as its text, each mention spelled as its box spells itself (binder.rs `prompt`). */
function askedIn(input: Record<string, unknown> | undefined, field: string): string | null {
  const said = input?.[field]
  const text = typeof said === "object" && said !== null ? (said as { text?: unknown }).text : said
  return typeof text === "string" ? text : null
}

/** A take being made, with nothing of it yet: behind its face (RunFace), what the words its run was asked mention, until it lands. Asked and
 *  not sent yet, it has no run: the box's own words are what's sent. */
function MadeFrom({ view, name, run, cardWidth }: { view: ElementView; name: string; run?: number; cardWidth: number }) {
  const { client } = useConnection()
  const store = useCanvasStore()
  const tools = useSignal(store.tools)
  const { data, isError } = useQuery({ ...runFullQuery(client, store.projectId ?? "", run ?? 0, 0), enabled: Boolean(store.projectId && run !== undefined) })
  const control = view.asset && fieldsOf(view.asset, tools).find((held) => held.type === "prompt")
  if (!control) return null
  const own = valueIn(view, control.field)
  const words = run === undefined || isError ? (typeof own === "string" ? own : null) : askedIn(data?.request?.input, control.field)
  if (!words?.trim()) return null
  return <Ghosts name={name} view={view} mentions={mentionsIn(words)} cardWidth={cardWidth} />
}

/** A picture laid out by `Spread`: its file, else a frame at its shape for one with nothing made yet. */
export interface SpreadPicture {
  key: string
  sha256?: string
  /** Width over height, while its file isn't read yet or for a frame. */
  shape?: number
  making?: boolean
}

/** How many rows `aspects` fill a frame of `frame` (width over height) best: each row an even share of the height, its pictures even shares
 *  of the width, each covering its cell, so the rows that crop them least, the fewest on a tie. */
function rowsFor(aspects: number[], frame: number): number {
  let best = 1
  let most = -1
  for (let rows = 1; rows <= aspects.length; rows++) {
    const per = Math.ceil(aspects.length / rows)
    let kept = 0
    for (let row = 0; row < rows; row++) {
      const these = aspects.slice(row * per, (row + 1) * per)
      const cell = (frame * rows) / Math.max(1, these.length)
      for (const aspect of these) kept += Math.min(aspect / cell, cell / aspect)
    }
    if (kept / aspects.length > most + 1e-6) [best, most] = [rows, kept / aspects.length]
  }
  return best
}

/** Every picture, filling its frame as large as they can (cl-949, Paul 11-12: "as large as possible in the background, full-bleed where
 *  possible"): side by side at full height when they fit, else the even grid that crops them least; never one picked. What's being made
 *  shows at rest (`Making`) and what's being written in Focus (`Backdrop`) lay out the same. */
export function Spread({ pictures, cardWidth, className }: { pictures: SpreadPicture[]; cardWidth: number; className?: string }) {
  const element = useRef<HTMLDivElement>(null)
  const [frame, setFrame] = useState(16 / 9)
  const [read, setRead] = useState<Record<string, number>>({})
  useLayoutEffect(() => {
    const held = element.current
    if (!held) return
    const measure = () => held.clientWidth > 0 && held.clientHeight > 0 && setFrame(held.clientWidth / held.clientHeight)
    measure()
    const observer = new ResizeObserver(measure)
    observer.observe(held)
    return () => observer.disconnect()
  }, [])
  if (!pictures.length) return null
  const rows = rowsFor(pictures.map((picture) => read[picture.key] ?? picture.shape ?? 1), frame)
  const per = Math.ceil(pictures.length / rows)
  const lines = Array.from({ length: rows }, (_, row) => pictures.slice(row * per, (row + 1) * per)).filter((line) => line.length)
  return (
    <div ref={element} className={`spread ${className ?? ""}`} data-rows={lines.length}>
      {lines.map((line, row) => (
        <div key={row} className="spread-row">
          {line.map((picture) => (
            <span key={picture.key} className="spread-picture" data-picture={picture.key}>
              {picture.sha256 ? (
                <Thumb sha256={picture.sha256} cardWidth={cardWidth / per} now onSize={(aspect) => setRead((was) => (was[picture.key] === aspect ? was : { ...was, [picture.key]: aspect }))} />
              ) : (
                <svg className={`ghost-blank ${picture.making ? "making" : ""}`} viewBox={`0 0 ${100 * (picture.shape ?? 1)} 100`} preserveAspectRatio="xMidYMid meet">
                  <rect x="2" y="2" width={100 * (picture.shape ?? 1) - 4} height="96" rx="3" />
                </svg>
              )}
            </span>
          ))}
        </div>
      ))}
    </div>
  )
}

/** The pictures and videos a run was sent, by their files, from every prompt it was sent (binder.rs `prompt`: a Shot's summary and its
 *  beats alike), each once: none when no prompt says anything of them. */
function sentPictures(input: unknown): string[] | undefined {
  const found = new Set<string>()
  let said = false
  const walk = (value: unknown) => {
    if (Array.isArray(value)) return value.forEach(walk)
    if (!value || typeof value !== "object") return
    const references = (value as { references?: unknown }).references
    if (Array.isArray(references)) {
      said = true
      for (const reference of references as ({ type?: unknown; file?: unknown } | null)[]) {
        if ((reference?.type === "image" || reference?.type === "video") && typeof reference.file === "string" && /^(sha256:)?[0-9a-f]{64}$/i.test(reference.file)) found.add(reference.file)
      }
    }
    Object.values(value).forEach(walk)
  }
  walk(input)
  return said ? [...found] : undefined
}

/** An image or a video being made, at rest (cl-940; cl-947 for a video's summary, Paul 11-12): the pictures its run reads, full-bleed,
 *  each at full height side by side, and the start of the words it was sent (an image's prompt, a Shot's summary) over a scrim, its
 *  mentions marked as the editor marks them. The run's own words and pictures, not the box's since; with no run yet, the box's own, which
 *  are what's sent. A preview of what it makes, when its package sends one, covers the pictures; the words stay. Inside its frame:
 *  nothing of it changes the box's size. */
function Making({ view, name, run, cardWidth }: { view: ElementView; name: string; run?: number; cardWidth: number }) {
  const { client } = useConnection()
  const store = useCanvasStore()
  const tools = useSignal(store.tools)
  useSignal(store.names)
  const { data, isError } = useQuery({ ...runFullQuery(client, store.projectId ?? "", run ?? 0, 0), enabled: Boolean(store.projectId && run !== undefined) })
  const control = view.asset && fieldsOf(view.asset, tools).find((held) => held.type === "prompt")
  if (!control) return null
  const own = valueIn(view, control.field)
  const asked = run !== undefined && !isError
  const words = asked ? askedIn(data?.request?.input, control.field) : typeof own === "string" ? own : null
  const sent = asked ? sentPictures(data?.request?.input) : undefined
  const pictures = sent ?? (words ? store.mentionedPictures(name, mentionsIn(words)).flatMap((ghost) => (ghost.sha256 ? [ghost.sha256] : [])) : [])
  if (!words?.trim() && !pictures.length) return null
  return (
    <div className="making" data-making={run ?? ""}>
      {pictures.length > 0 && (
        <div className="making-pictures" aria-hidden>
          <Spread pictures={pictures.map((sha256) => ({ key: sha256, sha256 }))} cardWidth={cardWidth} />
        </div>
      )}
      {words?.trim() && (
        <p className="making-words">
          {withMentions(words).map((part, i) => (part.mention ? <span key={i} className="chip">{chipWords(part.text.slice(1, -1))}</span> : part.text))}
        </p>
      )}
    </div>
  )
}

export function Media({ view, name, cardWidth, restWidth = cardWidth, children }: { view: ElementView; name: string; cardWidth: number; restWidth?: number; children?: ReactNode }) {
  const { client } = useConnection()
  const store = useCanvasStore()
  const { asset, placing, preview } = view
  // Being worked on, a take can be played and scrubbed, unless its words are over it.
  const open = Boolean(view.here && !view.hereWords)
  const shown = shownOf(view)
  const watch = watched(view)
  // The take looked at failed: its error in its frame, nothing of it on its way (cl-954).
  const failedTake = failureOf(view)
  const pendingImage = asset?.kind === "image" && Boolean(watch && watch.number === undefined) && !failedTake
  // Drawn: only the take shown, as its takes and label say, once its file is here; never another in its place (cl-942). Its neighbours,
  // or those of what showed before a take was asked for, stay decoded.
  const nearImage = asset?.kind === "image" ? shown ?? shownOf({ ...view, watch: undefined }) : null
  const drawnImage = asset?.kind === "image" && shown && !shown.absent ? shown : null
  // A dropped picture shows its own bytes for as long as it shows that file: no copy of it is asked for, so nothing takes its place.
  const own = Boolean(preview && (!preview.sha256 || !nearImage || hexOf(nearImage.sha256) === preview.sha256))
  const field = shown?.field ?? placing?.media ?? asset?.kind
  const source = sourceOf(view, shown)
  const making = asset?.standing === "making"
  const onItsWay = making || Boolean(view.queued) || view.asked !== undefined
  const busy = onItsWay && (Boolean(watch && watch.number === undefined) || !shown) && !failedTake
  // Made, with its file still on its way here: it says so, as a render on its way does.
  const arriving = useArriving(view)
  const run = onItsWay ? makingRun(view) : arriving
  // What a render showed so far: only the run its frame is on's, never another take's (cl-945).
  const glimpse = view.glimpse && (run === undefined || view.glimpse.number === run) ? view.glimpse : undefined
  const here = shown && !shown.absent ? shown : null
  // A video whose file says `orbit` turns, not plays: open, a drag turns it from the file here.
  const orbit = field === "video" ? orbitOf(shown) : null
  const turning = open && orbit && here && source && "path" in source && store.projectId ? { path: client.file(store.projectId, source.path), orbit } : null
  // On the canvas: an opened box plays the file itself.
  const { on: previewing, hover } = usePreview(field === "video" && Boolean(here) && !view.here)
  const tools = useSignal(store.tools)
  const words = lineOf(valuesOf(view), asset ? fieldsOf(asset, tools) : [])
  const setAspect = (aspect: number) => store.setAspect(name, aspect)
  // An Edit with no film yet is its first clip, and with no clip, what it's for.
  const clips = asset?.timeline?.tracks[0]?.clips
  const firstClip = clips?.find((clip) => clip.sha256 && !clip.missing)?.sha256
  const imageVersions = (asset?.prefixes ?? []).filter((version) => version.main?.field === "image")
  const imageAt = imageVersions.findIndex((version) => version.main?.sha256 === nearImage?.sha256)
  const neighbors = [1, -1, 2, -2].flatMap((offset) => {
    const next = imageVersions[imageAt + offset]?.main
    return next && !next.absent ? [next.sha256] : []
  })
  const neighborPaths = Object.fromEntries(imageVersions.flatMap((version) => version.main && !version.main.absent ? [[version.main.sha256, version.main.path]] : []))

  const slid = view.slid !== undefined && view.slid === shown?.number
  const blank = !preview && !shown && !firstClip
  const glimpsed = Boolean(glimpse && !shown && view.viewed === undefined && (making || asset?.failure))
  // The take its glimpse was playing as, opened, landing: its player plays on from there.
  const playhead = useRef<Playhead | null>(null)
  const playsOn = open && !glimpsed && source && "path" in source && here?.run !== undefined && playhead.current?.run === here.run ? playhead.current.at : undefined
  useEffect(() => {
    if (playsOn !== undefined) playhead.current = null
  }, [playsOn])
  // Its take being made, with nothing of it yet: what its words mention is behind its face.
  // An image or video being made at rest says what it's made from, in its frame (cl-940, cl-947), over what it shows; an image too, open on
  // a take of it being made (cl-945).
  const makingMedia = !failedTake && (!view.here || (asset?.kind === "image" && Boolean(watch && watch.number === undefined) && !view.hereWords)) && (asset?.kind === "image" || asset?.kind === "video") && !asset.timeline && onItsWay && asset.rendering == null && makes(view)
  const madeFrom = blank && busy && makes(view) && !asset?.timeline && !view.hereWords && view.viewed === undefined && !glimpse && !makingMedia
  const glyph = blank && !words && !clips && faceOf(view) === "edit" && !making && field ? GLYPHS[field] : undefined
  // On its way, its face says so whole, unless a take is looked at, a glimpse of what it's making shows or its words are being written:
  // then along its bottom.
  const full = (busy || arriving !== undefined) && view.viewed === undefined && !glimpse && !(view.here && view.hereWords)
  // What stopped its last try, in place: an approved box looked at shows nothing wrong (Card's dot).
  const failed = !busy && !placing && Boolean(asset?.failure) && view.viewed === undefined && !(asset?.pinned && faceOf(view) === "media")
  return (
    <div className={`media ${glimpsed ? "glimpsed" : ""} ${asset?.kind === "image" ? "image-asset" : ""} ${pendingImage ? "pending-image" : ""} ${asset?.kind === "image" && view.hereWords ? "image-editing" : ""} ${view.here && view.hereWords ? "writing" : ""} ${shown?.alpha ? "alpha" : ""} ${slid ? "slid" : ""} ${blank ? "blank" : ""}`} style={{ aspectRatio: aspectOf(view) }} onAnimationEnd={() => slid && store.patch(name, { slid: undefined })} {...hover}>
      <div className="media-pixels" data-image-reference={asset?.kind === "image" && shown && !view.hereWords ? "" : undefined}>
        {preview && (
          <img
            src={preview.url}
            alt=""
            draggable={false}
            onLoad={(event) => {
              const { naturalWidth, naturalHeight } = event.currentTarget
              if (naturalWidth && naturalHeight) setAspect(naturalWidth / naturalHeight)
            }}
          />
        )}
        {here && field === "video" && <Poster shown={here} cardWidth={cardWidth} name={name} scrubbed={view.here ? view.scrub : undefined} previewing={previewing} />}
        {nearImage && field === "image" && !own && <BufferedImage sha256={drawnImage?.sha256 ?? null} originalPath={drawnImage?.path} neighborPaths={neighborPaths} neighbors={nearImage.absent || nearImage === drawnImage ? neighbors : [nearImage.sha256, ...neighbors]} cardWidth={cardWidth} onSize={setAspect} onShown={() => store.showThumb(name)} />}
        {here && field !== "video" && field !== "image" && <FramePicture key={here.sha256} sha256={here.sha256} open={Boolean(view.here)} restWidth={restWidth} onSize={setAspect} onShown={() => store.showThumb(name)} />}
        {shown?.absent && arriving === undefined && <span className="media-note">{shown.machine ? `Kept on ${shown.machine}` : "Kept on another computer"}</span>}
        {!shown && firstClip && <Thumb sha256={firstClip} cardWidth={cardWidth} />}
      </div>
      {blank && words && !madeFrom && !makingMedia && <Said words={words} view={view} name={name} />}
      {blank && !words && clips && <span className="media-note">Drag videos and stills here</span>}
      {blank && !words && field === "file" && <span className="media-note">A file</span>}
      {glyph && (
        <span className="blank-glyph">
          <Icon name={glyph} />
        </span>
      )}
      {glimpsed && glimpse && <Glimpse glimpse={glimpse} failed={!making} open={Boolean(view.here)} playhead={playhead} />}
      {full && <div className="media-making" />}
      {madeFrom && <MadeFrom view={view} name={name} run={run} cardWidth={cardWidth} />}
      {makingMedia && <Making view={view} name={name} run={run} cardWidth={cardWidth} />}
      {/* At rest it keeps its size (cl-936): what's being made fades in over it, unless what it shows is approved; an image's covers its references (cl-940). */}
      {(busy || (!view.here && making && (!asset?.pinned || makingMedia))) && run !== undefined && view.viewed === undefined && !glimpse && <LatentPreview view={view} number={run} />}
      {busy && asset?.rendering != null && <Progress fraction={asset.rendering} says={`Rendering ${Math.round(asset.rendering * 100)}%`} rendering />}
      {failedTake ? <FailedHere whole message={failedTake.message.replace(`${name}: `, "")} copy={bodyText(failedTake, watch?.run, undefined)} /> : failed && <FailedHere message={asset!.failure!.message.replace(`${name}: `, "")} copy={bodyText(asset!.failure!, view.run?.number, view.run?.machine)} />}
      {/* What it can do is in Focus: at rest a box is its name, its picture, its tool and shape, and how its render goes (cl-932). */}
      {view.here && asset?.kind === "image" && asset.tool && !view.hereWords && !pendingImage && shown && <button type="button" className="image-edit-action nodrag nopan" aria-label="Edit image" onClick={(event) => { event.stopPropagation(); store.beginImageEdit(name) }}>Edit</button>}
      {turning && here && <OrbitView key={here.sha256} path={turning.path} shown={here} orbit={turning.orbit} />}
      {orbit && here && !open && !busy && (
        <span className="orbit-badge" title="An orbit: open it and drag to turn">
          <Icon name="orbit" />
        </span>
      )}
      {open && source && field && store.projectId && !turning && (
        <Player
          key={JSON.stringify(source)}
          field={field}
          path={"path" in source ? client.file(store.projectId, source.path) : undefined}
          machine={"run" in source ? { ...source, project: store.projectId } : undefined}
          seek={view.seek}
          onTime={field === "video" ? (seconds) => store.patch(name, { scrub: seconds }) : undefined}
          auto={playsOn !== undefined}
          from={playsOn}
        />
      )}
      {children}
    </div>
  )
}

/** Why its last render failed, along its bottom, until it's tried again. */
function FailedHere({ message, copy, whole = false }: { message: string; copy: string; whole?: boolean }) {
  return (
    <div className={`run-face lines failed ${whole ? "whole" : ""}`} data-phase="failed">
      <p className="face-state error-text nodrag nopan">
        <b title={message}><span className="face-words">Didn't render: {message}</span></b>
        <CopyError text={copy} />
      </p>
    </div>
  )
}

/** Where an opened glimpse was playing, by its run's number: the take that run makes plays on from there. */
interface Playhead {
  run: number
  at: number
}

/**
 * What a render has made so far, its newest revision here, over the box until its take lands, and kept dimmed when it fails. A video (a
 * shot's segments 1..k, joined) plays silent and looping at rest; opened, beside its beats or not, it waits with its controls, to play with
 * its sound and seek. A newer revision (1..k+1) loads where it was, playing on if it was.
 */
/** Where each run's glimpse last was, by its run and the revision it played: a newer revision of it starts there, however its box redrew. */
const glimpsedAt = new Map<string, { rev: number; at: number; playing: boolean }>()

function Glimpse({ glimpse, failed, open, playhead }: { glimpse: NonNullable<ElementView["glimpse"]>; failed: boolean; open: boolean; playhead: { current: Playhead | null } }) {
  const { client } = useConnection()
  const store = useCanvasStore()
  const [src, setSrc] = useState<string>()
  const element = useRef<HTMLVideoElement>(null)
  const resume = useRef<{ at: number; playing: boolean } | null>(null)
  // The revision its element plays: the newest may still be on its way.
  const playing = useRef(0)
  const key = `${store.projectId ?? ""}\0${glimpse.run}`
  useEffect(() => {
    if (!store.projectId) { setSrc(undefined); return }
    let live = true
    void client.transport.src(client.output({ project: store.projectId, run: glimpse.run }, glimpse.output, glimpse.index, glimpse.rev)).then((url) => {
      if (!live) return
      // Where it is now, else where it last was: its element redrawn, or a revision before still on its way.
      const el = element.current
      const was = glimpsedAt.get(key)
      resume.current = el?.currentSrc && !resume.current ? { at: el.currentTime, playing: !el.paused }
        : was && was.rev < glimpse.rev ? { at: was.at, playing: was.playing } : resume.current
      playing.current = glimpse.rev
      setSrc(url)
    })
    return () => {
      live = false
    }
  }, [client, store.projectId, key, glimpse.run, glimpse.output, glimpse.index, glimpse.rev])
  // Its new revision starts where the last one was: set before its metadata, it is never read at 0 s in between.
  useLayoutEffect(() => {
    const el = element.current
    if (el && src && resume.current) el.currentTime = resume.current.at
  }, [src])
  useEffect(() => {
    const el = element.current
    if (!el) return
    el.muted = !open
    if (open) el.pause()
    else void el.play().catch(() => {})
  }, [open])
  const heard = (el: HTMLVideoElement) => {
    playhead.current = open && !el.paused ? { run: glimpse.number, at: el.currentTime } : null
    if (el.readyState >= 1 && !resume.current) glimpsedAt.set(key, { rev: playing.current, at: el.currentTime, playing: !el.paused })
  }
  return (
    <div className={`glimpse ${failed ? "failed" : ""}`} data-label={glimpse.label ?? ""} data-rev={glimpse.rev}>
      {src &&
        (glimpse.media === "video" ? (
          <video
            ref={element}
            src={src}
            className={open ? "nodrag nopan nowheel" : undefined}
            muted={!open}
            loop={!open}
            autoPlay={!open}
            controls={open}
            playsInline
            preload="auto"
            onLoadedMetadata={(event) => {
              const was = resume.current
              resume.current = null
              if (!was) return
              const el = event.currentTarget
              el.currentTime = Math.min(was.at, el.duration || was.at)
              if (was.playing) void el.play().catch(() => {})
            }}
            onTimeUpdate={(event) => heard(event.currentTarget)}
            onPause={(event) => heard(event.currentTarget)}
            onSeeked={(event) => heard(event.currentTarget)}
          />
        ) : (
          <img src={src} alt="" draggable={false} />
        ))}
      {glimpse.label && <span className="glimpse-label">{glimpse.label}</span>}
    </div>
  )
}
