import { type PointerEvent, type ReactNode, useEffect, useRef, useState } from "react"
import { parseLink, Player as MachinePlayer } from "../../../player/cozy-webrtc.js"
import type { Client } from "../api/client"
import { holdMedia, playingHere } from "../api/events"
import type { Playback } from "../api/transport"
import { useConnection } from "../app/Connection"
import { leftWords } from "../panels/format"
import { fractionOf, packageWords, useInstalling, useWaitsOn } from "../panels/machines/activity"
import { long, workWords } from "../panels/runs/words"
import type { Run, Tool } from "../types"
import { useCanvasStore, useElement } from "./context"
import type { ElementView } from "./CanvasStore"
import { controlsOf } from "./controls"
import { Icon, type IconName } from "../ui/Icon"
import { useSignal } from "../ui/signal"
import { aspectOf, faceOf, fieldsOf, type Shown, shownOf } from "./shown"
import { chipWords, clock, lineOf, withMentions } from "./text"
import { isSelf } from "./versions"

const STAGE: Record<string, string> = { waiting: "Waiting", reading: "Reading", sending: "Sending", adding: "Adding" }
const STRIP_FRAMES = 8
/** How long the pointer rests on a video before its preview plays: one passing over on its way somewhere starts nothing. */
const PREVIEW_AFTER_MS = 250

/**
 * At most two thumbnails asked at once. Calcifer makes few at a time, and `<img>`s waiting on it would fill the browser's six
 * connections to it, stalling every other call. An element scrolled away gives up its place. What a person is looking at right now,
 * `now`, goes at once, past the line.
 */
function line(limit: number) {
  let running = 0
  const waiting: (() => void)[] = []
  const next = () => {
    while (running < limit && waiting.length) {
      running += 1
      waiting.shift()!()
    }
  }
  return (start: () => void, now = false): (() => void) => {
    let state: "waiting" | "running" | "done" = "waiting"
    const go = () => {
      state = "running"
      start()
    }
    if (now) {
      running += 1
      go()
    } else waiting.push(go)
    next()
    return () => {
      if (state === "waiting") waiting.splice(waiting.indexOf(go), 1)
      if (state === "running") {
        running -= 1
        next()
      }
      state = "done"
    }
  }
}

const thumbLine = line(2)
/** What's been up: coming back on screen is no arrival, and fades nothing. */
const arrived = new Set<string>()

/** A just-landed take's picture, at the size an open box draws it, asked once and decoded ahead, so a click on it shows it at once. */
const preloaded = new Set<string>()
export function preload(client: Client, projectId: string | null, sha256: string, cardWidth: number) {
  const key = `${sha256}:${thumbWidth(cardWidth)}`
  if (!projectId || preloaded.has(key)) return
  preloaded.add(key)
  void client.transport.src(client.thumb(projectId, sha256, thumbWidth(cardWidth))).then((url) => {
    const img = new Image()
    img.src = url
    img.decode().then(() => arrived.add(key), () => preloaded.delete(key))
  })
}

/** Thumbnails come in these widths, so a file has few sizes in Calcifer's cache. */
function thumbWidth(cardWidth: number): number {
  const wanted = cardWidth * Math.min(2, window.devicePixelRatio || 1)
  return wanted <= 256 ? 256 : wanted <= 512 ? 512 : 1024
}

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
  const [src, setSrc] = useState<string>()
  const [failed, setFailed] = useState(false)
  const projectId = store.projectId
  const leaveLine = useRef(() => {})
  const width = exact ? cardWidth : thumbWidth(cardWidth)
  useEffect(() => {
    if (!projectId) return
    let live = true
    leaveLine.current = thumbLine(() => {
      void client.transport.src(client.thumb(projectId, sha256, width, strip)).then((url) => {
        if (live) setSrc(url)
      })
    }, now)
    return () => {
      live = false
      leaveLine.current()
    }
  }, [client, projectId, sha256, width, strip])
  // A strip that can't be made leaves the poster under it as it was.
  if (failed) return strip ? null : <span className="media-note">No preview</span>
  if (!src) return null
  return (
    <img
      src={src}
      alt=""
      className={`${className ?? ""} ${arrived.has(`${sha256}:${width}`) ? "" : "arrives"}`}
      draggable={false}
      style={frame === undefined ? undefined : { transform: `translateX(${(-frame * 100) / (strip ?? 1)}%)` }}
      onError={() => {
        leaveLine.current()
        setFailed(true)
      }}
      onLoad={(event) => {
        leaveLine.current()
        const { naturalWidth, naturalHeight } = event.currentTarget
        if (naturalWidth && naturalHeight) onSize?.(naturalWidth / naturalHeight)
        arrived.add(`${sha256}:${width}`)
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

// A project's file, by its hash. Anything else from a state isn't followed: a remote Calcifer's answers are hostile.
const FILE = /^files\/[0-9a-f]{64}(\.\w+)?$/

export function sourceOf(view: ElementView, shown: Shown | null): Source | null {
  const asset = view.asset
  if (!asset || view.placing) return null
  if (asset.making?.length && asset.kind === "video" && view.viewed === undefined) return { run: Math.min(...asset.making), output: "video" }
  if (shown?.field !== "video" && shown?.field !== "audio") return null
  if (!shown.absent) return FILE.test(shown.path) ? { path: shown.path } : null
  return shown.machine && shown.run ? { run: shown.run, output: shown.output } : null
}

/** Straight from the machine: `play` says where and signs a cap for this output alone, and the same player as `cozy run play` follows it. */
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

/** How far it's got, on the picture: a veil that moves and never reflows, and the number or the stage. */
function Progress({ fraction, says, phase }: { fraction?: number; says?: string; phase?: string }) {
  return (
    <div className={`prog ${fraction === undefined ? "unknown" : ""} ${phase ?? ""}`}>
      <i style={fraction === undefined ? undefined : { transform: `translateX(${Math.round(fraction * 100) - 100}%)` }} />
      {says && <b>{says}</b>}
    </div>
  )
}

const installsOf = (tool?: Tool) => tool && (tool.installs ?? tool.package)

/** A render on its way: its package installing and its weights arriving say so, from what its machine lists, and the veil after them is
 *  its own work's. That says where it is once it's worth it: `Generating image 3 / 8, 25 s left`. Before its run is sent, local code
 *  installing for it says so too. */
function Making({ run, pkg, making, waits }: { run?: Run; pkg?: string; making: boolean; waits: string | null }) {
  const on = useWaitsOn(run)
  // Waiting for what it reads, it says that, and nothing of a machine's work for another box.
  const installing = useInstalling(run || waits ? undefined : pkg)
  if (waits && !on) return <WaitsFor name={waits} />
  const before = on ?? installing
  if (before?.kind === "install") return <Progress phase="install" says={`Installing ${packageWords(before.subject)}…`} />
  if (before?.kind === "download") {
    const fraction = fractionOf(before)
    const left = before.eta_s === undefined ? "" : `, ${leftWords(before.eta_s)}`
    return <Progress phase="download" fraction={fraction} says={`Waiting on download… ${fraction === undefined ? "" : `${Math.round(fraction * 100)}%`}${left}`} />
  }
  if (!making) return null
  // A run that ended says no more of how far: it's all there, on its way here.
  if (!run || run.status === "completed") return <Progress fraction={run ? 1 : undefined} />
  return <Progress fraction={run.progress?.done_fraction} says={long(run, Date.now()) ? workWords(run) || run.waiting : undefined} />
}

/** The picture in a box's frame, one take's alone: its small thumbnail at once, the one its tile and its box at rest already have, scaled up,
 *  and its full size over it as soon as that's decoded. Another take's picture is never what stands in. */
function FramePicture({ sha256, cardWidth, restWidth, onSize, onShown }: { sha256: string; cardWidth: number; restWidth: number; onSize: (aspect: number) => void; onShown: () => void }) {
  const [full, setFull] = useState(false)
  const larger = thumbWidth(cardWidth) > thumbWidth(restWidth)
  return (
    <>
      {larger && !full && <Thumb sha256={sha256} cardWidth={restWidth} now />}
      <Thumb
        sha256={sha256}
        cardWidth={cardWidth}
        now={larger}
        onSize={onSize}
        onShown={() => {
          setFull(true)
          onShown()
        }}
      />
    </>
  )
}

/** A box that waits for one it reads: which, and how far that one's got while it's made. */
function WaitsFor({ name }: { name: string }) {
  const store = useCanvasStore()
  const read = useElement(store.mentionOf(name, "")?.name ?? name)
  const run = read?.asset?.making?.length ? read.run : undefined
  const words = run && run.status !== "completed" ? workWords(run) : ""
  return <Progress fraction={run?.progress?.done_fraction} says={`Waits for <${name}>${words ? `: ${words}` : ""}`} />
}

export function Media({ view, name, cardWidth, restWidth = cardWidth, children }: { view: ElementView; name: string; cardWidth: number; restWidth?: number; children?: ReactNode }) {
  const { client } = useConnection()
  const store = useCanvasStore()
  const { asset, placing, preview } = view
  // Being worked on, a take can be played and scrubbed, unless its words are over it.
  const open = Boolean(view.here && !view.hereWords)
  const shown = shownOf(view)
  const field = shown?.field ?? placing?.media ?? asset?.kind
  const source = sourceOf(view, shown)
  const making = asset?.standing === "making"
  const here = shown && !shown.absent ? shown : null
  // On the canvas: an opened box plays the file itself.
  const { on: previewing, hover } = usePreview(field === "video" && Boolean(here) && !view.here)
  const tools = useSignal(store.tools)
  const words = lineOf(fieldsOf(view), asset ? controlsOf(asset, tools) : [])
  const setAspect = (aspect: number) => store.setAspect(name, aspect)
  // An Edit with no film yet is its first clip, and with no clip, what it's for.
  const clips = asset?.timeline?.tracks[0]?.clips
  const firstClip = clips?.find((clip) => clip.sha256 && !clip.missing)?.sha256

  const slid = view.slid !== undefined && view.slid === shown?.number
  const blank = !preview && !shown && !firstClip
  const glyph = blank && !words && !clips && faceOf(view) === "edit" && !making && field ? GLYPHS[field] : undefined
  return (
    <div className={`media ${shown?.alpha ? "alpha" : ""} ${slid ? "slid" : ""} ${blank ? "blank" : ""}`} style={{ aspectRatio: aspectOf(view) }} onAnimationEnd={() => slid && store.patch(name, { slid: undefined })} {...hover}>
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
      {here && field !== "video" && <FramePicture key={here.sha256} sha256={here.sha256} cardWidth={cardWidth} restWidth={restWidth} onSize={setAspect} onShown={() => store.showThumb(name)} />}
      {shown?.absent && <span className="media-note">{shown.machine ? `On ${shown.machine}` : "On another computer"}</span>}
      {!shown && firstClip && <Thumb sha256={firstClip} cardWidth={cardWidth} />}
      {blank && words && (
        <p className="words">
          {withMentions(words).map((part, i) =>
            part.mention ? (
              <span key={i} className={`chip ${asset?.missing?.includes(part.text.slice(1, -1)) || (store.mentionOf(part.text.slice(1, -1), name)?.gone ?? isSelf(part.text.slice(1, -1))) ? "missing" : ""}`}>
                {chipWords(part.text.slice(1, -1))}
              </span>
            ) : (
              part.text
            ),
          )}
        </p>
      )}
      {blank && !words && clips && <span className="media-note">Drag videos and stills here</span>}
      {blank && !words && field === "file" && <span className="media-note">A file</span>}
      {glyph && (
        <span className="blank-glyph">
          <Icon name={glyph} />
        </span>
      )}
      {(making || view.queued || view.asked !== undefined) && view.viewed === undefined && <div className="media-making" />}
      {placing && <Progress fraction={placing.done} says={stageLine(view) ?? undefined} />}
      {(making || view.queued || view.asked !== undefined) && <Making run={view.run} pkg={installsOf(asset?.tool ? tools.get(asset.tool) : undefined)} making={making} waits={making ? null : store.waitsFor(name)} />}
      {open && source && field && store.projectId && (
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

export const stageLine = (view: ElementView): string | null => {
  const placing = view.placing
  if (!placing) return null
  const percent = placing.stage === "reading" || placing.stage === "sending" ? ` ${Math.round(placing.done * 100)}%` : ""
  return `${STAGE[placing.stage]}${percent}${placing.attempt ? ` · attempt ${placing.attempt}/5` : ""}`
}
