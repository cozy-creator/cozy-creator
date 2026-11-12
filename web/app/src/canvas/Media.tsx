import { useQuery } from "@tanstack/react-query"
import { type CSSProperties, type PointerEvent, type ReactNode, useEffect, useLayoutEffect, useRef, useState } from "react"
import { parseLink, Player as MachinePlayer } from "../../../player/cozy-webrtc.js"
import type { Client } from "../api/client"
import { runFullQuery, runQuery, runsQuery } from "../api/queries"
import { holdMedia, playingHere } from "../api/events"
import type { Playback } from "../api/transport"
import { useConnection } from "../app/Connection"
import { leftWords } from "../panels/format"
import { fractionOf, packageWords, useInstalling, useWaitsOn } from "../panels/machines/activity"
import { long, workWords } from "../panels/runs/words"
import type { MainFile, Run, Tool } from "../api/calcifer"
import { useCanvasStore, useElement } from "./context"
import type { ElementView } from "./CanvasStore"
import { fieldsOf } from "./controls"
import { Icon, type IconName } from "../ui/Icon"
import { useSignal } from "../ui/signal"
import { aspectOf, faceOf, makes, makingRun, type Shown, shownOf, valueIn, valuesOf, watched } from "./shown"
import { chipWords, clock, lineOf, mentionsIn, withMentions } from "./text"
import { isSelf } from "./versions"
import { OrbitView, orbitOf } from "./Orbit"
import { BufferedImage } from "./BufferedImage"

const STAGE: Record<string, string> = { waiting: "Waiting", reading: "Reading", sending: "Sending", adding: "Adding" }
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
const preloaded = new Set<string>()
export function preload(client: Client, projectId: string | null, sha256: string) {
  const key = `${sha256}:${FULL}`
  if (!projectId || preloaded.has(key)) return
  preloaded.add(key)
  void client.transport.src(client.thumb(projectId, sha256, FULL)).then((url) => {
    const img = new Image()
    img.src = url
    img.decode().then(() => arrived.add(key), () => preloaded.delete(key))
  })
}

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
  const width = exact ? cardWidth : Math.max(thumbWidth(cardWidth, zoom), strip ? 0 : (widest.get(sha256) ?? 0))
  const path = projectId ? client.thumb(projectId, sha256, width, strip) : undefined
  useEffect(() => {
    if (!path) return
    let live = true
    const request = ++nextRequest.current
    const leave = thumbLine(path, (complete) => {
      void client.transport.src(path).then((src) => {
        if (!live) return
        setFailedPath(undefined)
        setImage({ path, src, request, complete, live: () => live })
      }, () => {
        if (!live) return
        leave()
        setFailedPath(path)
      })
    }, now)
    return () => {
      live = false
      leave()
    }
  }, [client, path])
  // A strip that can't be made leaves the poster under it as it was.
  if (path && failedPath === path) return strip ? null : <span className="media-note">No preview</span>
  if (!image || failedPath === image.path) return null
  return (
    <img
      key={image.request}
      src={image.src}
      alt=""
      className={`${className ?? ""} ${arrived.has(`${sha256}:${width}`) ? "" : "arrives"}`}
      draggable={false}
      style={frame === undefined ? undefined : { transform: `translateX(${(-frame * 100) / (strip ?? 1)}%)` }}
      onError={() => {
        if (image.path !== path || !image.live()) return
        image.complete()
        setFailedPath(image.path)
      }}
      onLoad={(event) => {
        if (image.path !== path || !image.live()) return
        image.complete()
        const { naturalWidth, naturalHeight } = event.currentTarget
        if (naturalWidth && naturalHeight) onSize?.(naturalWidth / naturalHeight)
        arrived.add(`${sha256}:${width}`)
        if (!strip) widest.set(sha256, Math.max(width, widest.get(sha256) ?? 0))
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

/** Previews that can't play, by hash: a hover doesn't ask again. */
const unpreviewable = new Set<string>()

/**
 * A video's preview over its poster: a few seconds from the poster's frame on, silent and looping, which Calcifer made once as the file
 * landed. Nothing loads before the hover, it shows from its first frame, and it goes with the hover or with its tile.
 */
export function Preview({ sha256 }: { sha256: string }) {
  const { client } = useConnection()
  const store = useCanvasStore()
  const [src, setSrc] = useState<string>()
  const [playing, setPlaying] = useState(false)
  const element = useRef<HTMLVideoElement>(null)
  const projectId = store.projectId
  useEffect(() => {
    if (!projectId || unpreviewable.has(sha256)) return
    let live = true
    void client.transport.src(client.preview(projectId, sha256)).then((url) => {
      if (live) setSrc(url)
    })
    return () => {
      live = false
    }
  }, [client, projectId, sha256])
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
        unpreviewable.add(sha256)
        setSrc(undefined)
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
    if (run) return { run, output: "video" }
  }
  if (shown?.field !== "video" && shown?.field !== "audio") return null
  if (!shown.absent) return FILE.test(shown.path) ? { path: shown.path } : null
  return shown.machine && shown.run ? { run: shown.run, output: shown.output } : null
}

/** Straight from the machine: `play` says where and signs a cap for this output alone, and the shared browser player follows it. */
async function fromMachine(client: Client, source: { run: number; output: string }, el: HTMLMediaElement, failed: (why: string) => void): Promise<Playback> {
  const [name, index] = source.output.split("/")
  const answer = await client.play(String(source.run), name, Number(index ?? 0))
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
export function Player({ field, path, machine, seek, auto }: { field: string; path?: string; machine?: { run: number; output: string }; seek?: ElementView["seek"]; auto?: boolean }) {
  const { client, stream } = useConnection()
  const [state, setState] = useState<"idle" | "waiting" | "playing" | "unplayable">("idle")
  const [why, setWhy] = useState("This browser can't play it")
  const element = useRef<HTMLVideoElement & HTMLAudioElement>(null)
  const stopRef = useRef<() => void>(() => {})
  const wanted = useRef<number | null>(null)
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
        playback = path !== undefined ? client.transport.play(path, el) : await fromMachine(client, machine!, el, failed)
        if (waiting.signal.aborted) return playback.stop()
        setState("playing")
        if (wanted.current !== null) el.currentTime = wanted.current
        wanted.current = null
        el.play().catch(() => {})
      })
      .catch((error: unknown) => {
        if (!waiting.signal.aborted) failed(error instanceof Error ? error.message : String(error))
      })
  }, [state, client, path, machine?.run, machine?.output])

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
        onEnded={() => stopRef.current()}
        onError={() => {
          const el = element.current
          // Bytes that didn't come are Calcifer's to fix by coming back. Bytes this browser can't decode, it can't.
          if (path !== undefined && el?.getAttribute("src") && el.error?.code !== MediaError.MEDIA_ERR_NETWORK && stream.health.get().state === "live") setState("unplayable")
        }}
      />
      <button type="button" className="stop nodrag" aria-label="Stop" onClick={() => stopRef.current()}>
        <Icon name="close" />
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

const installsOf = (tool?: Tool) => tool && (tool.installs ?? tool.package)
const settingUp = (run: Run) => run.kind === "update" || run.target.startsWith("install ")
const sentence = (said: string) => said.charAt(0).toUpperCase() + said.slice(1)

/** How far a render on its way has got, and the one line it says. */
interface Way {
  veil?: boolean
  fraction?: number
  phase?: string
  says?: string
  rendering?: boolean
}

/** A render on its way, in one line, the most specific first: an Edit rendering here; what it reads and waits for, with that one's work; its
 *  package installing or its weights arriving, from what its machine lists, before its run is sent too; its counted steps; its place in a
 *  line; what it's doing; its machine getting ready; else queued. Its own work says so once it's worth it: `Denoise steps: 3/8, 25 s left`.
 *  The veil is its run's, while it's made. The take looked at may be one asked after the first going: it's that one's. */
function OnItsWay({ view, name, number }: { view: ElementView; name: string; number?: number }) {
  const { client } = useConnection()
  const store = useCanvasStore()
  const tools = useSignal(store.tools)
  const asset = view.asset
  const making = asset?.standing === "making"
  const later = number !== undefined && number !== view.run?.number
  const { data } = useQuery({ ...runQuery(client, number ?? 0), enabled: later })
  const run = later ? data : view.run
  const on = useWaitsOn(run)
  // Waiting for what it reads, it says that, and nothing of a machine's work for another box.
  const waits = making ? null : store.waitsFor(name)
  const installing = useInstalling(run || waits ? undefined : installsOf(asset?.tool ? tools.get(asset.tool) : undefined))
  const read = useElement(store.mentionOf(waits ?? name, "")?.name ?? name)
  const going = useQuery({ ...runsQuery(client, { status: "in_progress" }, 100), enabled: !run })
  const lined = useQuery({ ...runsQuery(client, { status: "queued" }, 100), enabled: !run })
  if (asset?.rendering != null) return <Progress fraction={asset.rendering} says={`Rendering ${Math.round(asset.rendering * 100)}%`} rendering />
  if (waits && !on) {
    const theirs = read?.asset?.making?.length ? read.run : undefined
    const words = theirs && theirs.status !== "completed" ? workWords(theirs) : ""
    return <Progress fraction={theirs?.progress?.done_fraction ?? undefined} says={`Waits for <${waits}>${words ? `: ${words}` : ""}`} />
  }
  const before = on ?? installing
  if (before?.kind === "install") return <Progress phase="install" says={`Installing ${packageWords(before.subject)}…`} />
  if (before?.kind === "download") {
    const fraction = fractionOf(before)
    const left = before.eta_s == null ? "" : `, ${leftWords(before.eta_s)}`
    return <Progress phase="download" fraction={fraction} says={`Waiting on download… ${fraction === undefined ? "" : `${Math.round(fraction * 100)}%`}${left}`} />
  }
  // A run that ended says no more of how far: it's all there, on its way here.
  if (run?.status === "completed") return <Progress fraction={1} />
  const work = run && long(run, Date.now()) ? workWords(run) : ""
  const counted = Boolean(run?.progress?.total || run?.progress?.bytes_total)
  const machine = !run && [...(going.data?.runs ?? []), ...(lined.data?.runs ?? [])].find(settingUp)?.machine
  const says =
    (counted && work) ||
    (run?.queue_position !== undefined && `Queued, number ${run.queue_position} in line`) ||
    (run?.waiting && sentence(run.waiting)) ||
    work ||
    (machine && `Getting ${machine} ready`) ||
    ((!run || run.status === "queued") && "Queued")
  return <Progress veil={making} fraction={run?.progress?.done_fraction ?? undefined} says={says || undefined} />
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

/** A take being made, with nothing of it yet: where its picture will be, the words its run was asked, read only, as they were written in its
 *  frame, over what they mention, until it lands. Asked and not sent yet, it has no run: the box's own words are what's sent. */
function MadeFrom({ view, name, run, cardWidth }: { view: ElementView; name: string; run?: number; cardWidth: number }) {
  const { client } = useConnection()
  const store = useCanvasStore()
  const tools = useSignal(store.tools)
  const { data, isError } = useQuery({ ...runFullQuery(client, run ?? 0, 0), enabled: run !== undefined })
  const control = view.asset && fieldsOf(view.asset, tools).find((held) => held.type === "prompt")
  if (!control) return null
  const own = valueIn(view, control.field)
  const words = run === undefined || isError ? (typeof own === "string" ? own : null) : askedIn(data?.request?.input, control.field)
  if (!words?.trim()) return null
  // What they mention under how far it's got, and the words over it.
  return (
    <>
      <Ghosts name={name} view={view} mentions={mentionsIn(words)} cardWidth={cardWidth} />
      <div className="made-from">
        <Said words={words} view={view} name={name} />
      </div>
    </>
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
  const pendingImage = asset?.kind === "image" && Boolean(watch && watch.number === undefined)
  const bufferedImage = asset?.kind === "image" ? shown ?? shownOf({ ...view, watchRun: undefined, watchFrom: undefined }) : null
  const field = shown?.field ?? placing?.media ?? asset?.kind
  const source = sourceOf(view, shown)
  const making = asset?.standing === "making"
  const busy = (making || Boolean(view.queued) || view.asked !== undefined) && (Boolean(watch && watch.number === undefined) || !shown)
  const run = busy ? makingRun(view) : undefined
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
  const imageAt = imageVersions.findIndex((version) => version.main?.sha256 === bufferedImage?.sha256)
  const neighbors = [1, -1, 2, -2].flatMap((offset) => {
    const next = imageVersions[imageAt + offset]?.main
    return next && !next.absent ? [next.sha256] : []
  })
  const neighborPaths = Object.fromEntries(imageVersions.flatMap((version) => version.main && !version.main.absent ? [[version.main.sha256, version.main.path]] : []))

  const slid = view.slid !== undefined && view.slid === shown?.number
  const blank = !preview && !shown && !firstClip
  // Its take being made, with nothing of it yet: its frame says what it's made from.
  const madeFrom = blank && busy && makes(view) && !asset?.timeline && !view.hereWords && view.viewed === undefined && !view.glimpse
  const glyph = blank && !words && !clips && faceOf(view) === "edit" && !making && field ? GLYPHS[field] : undefined
  return (
    <div className={`media ${asset?.kind === "image" ? "image-asset" : ""} ${pendingImage ? "pending-image" : ""} ${asset?.kind === "image" && view.hereWords ? "image-editing" : ""} ${shown?.alpha ? "alpha" : ""} ${slid ? "slid" : ""} ${blank ? "blank" : ""}`} style={{ aspectRatio: aspectOf(view) }} onAnimationEnd={() => slid && store.patch(name, { slid: undefined })} {...hover}>
      <div className="media-pixels" data-image-reference={asset?.kind === "image" && shown && !view.hereWords ? "" : undefined}>
        {preview && (
          <img
            src={preview}
            alt=""
            draggable={false}
            onLoad={(event) => {
              const { naturalWidth, naturalHeight } = event.currentTarget
              if (naturalWidth && naturalHeight) setAspect(naturalWidth / naturalHeight)
            }}
          />
        )}
        {here && field === "video" && <Poster shown={here} cardWidth={cardWidth} name={name} scrubbed={view.scrub} previewing={previewing} />}
        {bufferedImage && field === "image" && <BufferedImage sha256={bufferedImage.sha256} originalPath={bufferedImage.path} neighborPaths={neighborPaths} neighbors={neighbors} cardWidth={cardWidth} onSize={setAspect} onShown={() => store.showThumb(name)} />}
        {here && field !== "video" && field !== "image" && <FramePicture key={here.sha256} sha256={here.sha256} open={Boolean(view.here)} restWidth={restWidth} onSize={setAspect} onShown={() => store.showThumb(name)} />}
        {shown?.absent && <span className="media-note">{shown.machine ? `On ${shown.machine}` : "On another computer"}</span>}
        {!shown && firstClip && <Thumb sha256={firstClip} cardWidth={cardWidth} />}
      </div>
      {blank && words && !madeFrom && <Said words={words} view={view} name={name} />}
      {blank && !words && clips && <span className="media-note">Drag videos and stills here</span>}
      {blank && !words && field === "file" && <span className="media-note">A file</span>}
      {glyph && (
        <span className="blank-glyph">
          <Icon name={glyph} />
        </span>
      )}
      {view.glimpse && !shown && view.viewed === undefined && (making || asset?.failure) && <Glimpse glimpse={view.glimpse} failed={!making} />}
      {busy && view.viewed === undefined && <div className="media-making" />}
      {madeFrom && <MadeFrom view={view} name={name} run={run} cardWidth={cardWidth} />}
      {placing && <Progress fraction={placing.done} says={stageLine(view) ?? undefined} />}
      {busy && <OnItsWay view={view} name={name} number={run} />}
      {asset?.kind === "image" && asset.tool && !view.hereWords && !pendingImage && shown && <button type="button" className="image-edit-action nodrag nopan" aria-label="Edit image" onClick={(event) => { event.stopPropagation(); store.beginImageEdit(name) }}>Edit</button>}
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
          machine={"run" in source ? source : undefined}
          seek={view.seek}
        />
      )}
      {children}
    </div>
  )
}

/** What a render has shown so far, as it lands: over the box, and kept dimmed when it fails. */
function Glimpse({ glimpse, failed }: { glimpse: NonNullable<ElementView["glimpse"]>; failed: boolean }) {
  const { client } = useConnection()
  const [src, setSrc] = useState<string>()
  useEffect(() => {
    let live = true
    void client.transport.src(client.output(glimpse.run, glimpse.output, glimpse.index)).then((url) => {
      if (live) setSrc(url)
    })
    return () => {
      live = false
    }
  }, [client, glimpse.run, glimpse.output, glimpse.index])
  return (
    <div className={`glimpse ${failed ? "failed" : ""}`} data-label={glimpse.label ?? ""}>
      {src && (glimpse.media === "video" ? <video src={src} muted loop autoPlay playsInline /> : <img src={src} alt="" draggable={false} />)}
      {glimpse.label && <span className="glimpse-label">{glimpse.label}</span>}
    </div>
  )
}

export const stageLine = (view: ElementView): string | null => {
  const placing = view.placing
  if (!placing) return null
  const percent = placing.stage === "reading" || placing.stage === "sending" ? ` ${Math.round(placing.done * 100)}%` : ""
  return `${STAGE[placing.stage]}${percent}${placing.attempt ? ` · attempt ${placing.attempt}/5` : ""}`
}
