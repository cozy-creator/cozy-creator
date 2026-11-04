import { type ReactNode, useEffect, useRef, useState } from "react"
import { parseLink, Player as MachinePlayer } from "../../../player/cozy-webrtc.js"
import type { Client } from "../api/client"
import { holdMedia, playingHere } from "../api/events"
import type { Playback } from "../api/transport"
import { useConnection } from "../app/Connection"
import { useCanvasStore } from "./context"
import type { ElementView } from "./CanvasStore"
import { Icon, type IconName } from "../ui/Icon"
import { aspectOf, faceOf, type Shown, shownOf } from "./shown"
import { clock, lineOf } from "./text"

const STAGE: Record<string, string> = { waiting: "Waiting", reading: "Reading", sending: "Sending", adding: "Adding" }
const STRIP_FRAMES = 8

/**
 * At most two thumbnails asked at once. Calcifer makes few at a time, and `<img>`s waiting on it would fill the browser's six
 * connections to it, stalling every other call. An element scrolled away gives up its place.
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
  return (start: () => void): (() => void) => {
    let state: "waiting" | "running" | "done" = "waiting"
    const go = () => {
      state = "running"
      start()
    }
    waiting.push(go)
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
    })
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

/** A video's poster. Hovering scrubs a strip of its frames, one picture from Calcifer, so no `<video>` opens until it's clicked. */
function Poster({ shown, cardWidth, name, scrubbed }: { shown: Shown; cardWidth: number; name: string; scrubbed?: number }) {
  const store = useCanvasStore()
  const [hover, setAt] = useState<number | null>(null)
  const at = hover ?? (scrubbed !== undefined && shown.duration_ms ? (scrubbed * 1000) / shown.duration_ms : null)
  const frame = at === null ? 0 : Math.min(STRIP_FRAMES - 1, Math.floor(at * STRIP_FRAMES))
  return (
    <div
      className="scrub"
      onPointerMove={(event) => {
        const box = event.currentTarget.getBoundingClientRect()
        setAt(Math.max(0, Math.min(1, (event.clientX - box.left) / box.width)))
      }}
      onPointerLeave={() => setAt(null)}
    >
      <Thumb sha256={shown.sha256} cardWidth={cardWidth} onSize={(aspect) => store.setAspect(name, aspect)} onShown={() => store.showThumb(name)} />
      {at !== null && <Thumb sha256={shown.sha256} cardWidth={cardWidth} strip={STRIP_FRAMES} frame={frame} className="strip" />}
      {at !== null && shown.duration_ms ? <span className="tc">{clock(at * shown.duration_ms)}</span> : null}
    </div>
  )
}

/** What a click plays: a file here over Range, or from the machine that has it or is still making it. */
type Source = { path: string } | { run: number; output: string }

// A project's file, by its hash. Anything else from a state isn't followed: a remote Calcifer's answers are hostile.
const FILE = /^files\/[0-9a-f]{64}(\.\w+)?$/

function sourceOf(view: ElementView, shown: Shown | null): Source | null {
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

/** Played on click, one of the two live ones across tabs: a file Calcifer serves, by its API path, or one straight from a machine. */
export function Player({ field, path, machine, seek }: { field: string; path?: string; machine?: { run: number; output: string }; seek?: ElementView["seek"] }) {
  const { client, stream } = useConnection()
  const [state, setState] = useState<"idle" | "waiting" | "playing" | "unplayable">("idle")
  const [why, setWhy] = useState("This browser can't play it")
  const element = useRef<HTMLVideoElement & HTMLAudioElement>(null)
  const stopRef = useRef<() => void>(() => {})
  const wanted = useRef<number | null>(null)
  const asked = useRef(seek?.ask ?? 0)

  useEffect(() => () => stopRef.current(), [])

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

const leftWords = (fraction: number, seconds?: number) =>
  `${Math.round(fraction * 100)}%${seconds === undefined ? "" : `, ${seconds < 90 ? `${Math.max(1, Math.round(seconds))} s` : `${Math.round(seconds / 60)} min`} left`}`

/** How far it's got, on the picture: a veil that moves and never reflows, and the number or the stage. */
function Progress({ fraction, says }: { fraction?: number; says?: string }) {
  return (
    <div className={`prog ${fraction === undefined ? "unknown" : ""}`}>
      <i style={fraction === undefined ? undefined : { transform: `translateX(${Math.round(fraction * 100) - 100}%)` }} />
      {says && <b>{says}</b>}
    </div>
  )
}

export function Media({ view, name, cardWidth, children }: { view: ElementView; name: string; cardWidth: number; children?: ReactNode }) {
  const { client } = useConnection()
  const store = useCanvasStore()
  const { asset, placing, preview } = view
  const shown = shownOf(view)
  const field = shown?.field ?? placing?.media ?? asset?.kind
  const source = sourceOf(view, shown)
  const making = asset?.standing === "making"
  const fraction = making ? view.run?.progress?.done_fraction : undefined
  const here = shown && !shown.absent ? shown : null
  const words = lineOf(asset?.fields)
  const setAspect = (aspect: number) => store.setAspect(name, aspect)
  // An Edit with no film yet is its first clip, and with no clip, what it's for.
  const clips = asset?.timeline?.tracks[0]?.clips
  const firstClip = clips?.find((clip) => clip.sha256 && !clip.missing)?.sha256

  const slid = view.slid !== undefined && view.slid === shown?.number
  const blank = !preview && !shown && !firstClip
  const glyph = blank && !words && !clips && faceOf(view) === "edit" && !making && field ? GLYPHS[field] : undefined
  return (
    <div className={`media ${shown?.alpha ? "alpha" : ""} ${slid ? "slid" : ""} ${blank ? "blank" : ""}`} style={{ aspectRatio: aspectOf(view) }} onAnimationEnd={() => slid && store.patch(name, { slid: undefined })}>
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
      {here && field === "video" && <Poster shown={here} cardWidth={cardWidth} name={name} scrubbed={view.scrub} />}
      {here && field !== "video" && <Thumb sha256={here.sha256} cardWidth={cardWidth} onSize={setAspect} onShown={() => store.showThumb(name)} />}
      {shown?.absent && <span className="media-note">{shown.machine ? `On ${shown.machine}` : "On another computer"}</span>}
      {!shown && firstClip && <Thumb sha256={firstClip} cardWidth={cardWidth} />}
      {blank && words && <p className="words">{words}</p>}
      {blank && !words && clips && <span className="media-note">Drag videos and stills here</span>}
      {blank && !words && field === "file" && <span className="media-note">A file</span>}
      {glyph && (
        <span className="blank-glyph">
          <Icon name={glyph} />
        </span>
      )}
      {placing && <Progress fraction={placing.done} says={stageLine(view) ?? undefined} />}
      {making && <Progress fraction={fraction} says={fraction === undefined ? undefined : leftWords(fraction, view.run?.progress?.eta_s)} />}
      {source && field && store.projectId && (
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
