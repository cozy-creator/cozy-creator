import { type PointerEvent, type ReactNode, useEffect, useRef, useState } from "react"
import { parseLink, Player as MachinePlayer } from "../../../player/cozy-webrtc.js"
import type { Client } from "../api/client"
import { holdMedia, playingHere } from "../api/events"
import type { Playback } from "../api/transport"
import { useConnection } from "../app/Connection"
import { useCanvasStore } from "./context"
import type { ElementView } from "./CanvasStore"
import { fieldsOf } from "./controls"
import { Icon, type IconName } from "../ui/Icon"
import { useSignal } from "../ui/signal"
import { aspectOf, faceOf, type Shown, shownOf } from "./shown"
import { clock, lineOf, withMentions } from "./text"
import { isSelf } from "./versions"

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

/** Where the pointer is across what's under it, 0 to 1, for walking a video's frames. A finger has nothing to follow, and a held button is a drag. */
export function useWalk() {
  const [at, setAt] = useState<number | null>(null)
  return {
    at,
    walk(event: PointerEvent<HTMLElement>) {
      if (event.pointerType === "touch" || event.buttons) return
      const box = event.currentTarget.getBoundingClientRect()
      setAt(Math.max(0, Math.min(1, (event.clientX - box.left) / box.width)))
    },
    stop: () => setAt(null),
  }
}

/** A video's frames walked across, one picture from Calcifer, so no `<video>` opens until it's clicked. */
export function WalkedFrames({ sha256, cardWidth, at }: { sha256: string; cardWidth: number; at: number }) {
  return <Thumb sha256={sha256} cardWidth={cardWidth} strip={STRIP_FRAMES} frame={Math.min(STRIP_FRAMES - 1, Math.floor(at * STRIP_FRAMES))} className="strip" />
}

/** A video's poster, and its frames under the pointer. */
function Poster({ shown, cardWidth, name, scrubbed, hover }: { shown: Shown; cardWidth: number; name: string; scrubbed?: number; hover: number | null }) {
  const store = useCanvasStore()
  const at = hover ?? (scrubbed !== undefined && shown.duration_ms ? (scrubbed * 1000) / shown.duration_ms : null)
  return (
    <div className="scrub">
      <Thumb sha256={shown.sha256} cardWidth={cardWidth} onSize={(aspect) => store.setAspect(name, aspect)} onShown={() => store.showThumb(name)} />
      {at !== null && <WalkedFrames sha256={shown.sha256} cardWidth={cardWidth} at={at} />}
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
  // Being worked on, a take can be played and scrubbed, unless its words are over it.
  const open = Boolean(view.here && !view.hereWords)
  const { at: hover, walk, stop } = useWalk()
  const shown = shownOf(view)
  const field = shown?.field ?? placing?.media ?? asset?.kind
  const source = sourceOf(view, shown)
  const making = asset?.standing === "making"
  // A run that ended says no more of how far: it's all there, on its way here.
  const fraction = making ? (view.run?.status === "completed" ? 1 : view.run?.progress?.done_fraction) : undefined
  const here = shown && !shown.absent ? shown : null
  const tools = useSignal(store.tools)
  const words = lineOf(asset?.fields, asset ? fieldsOf(asset, tools) : [])
  const setAspect = (aspect: number) => store.setAspect(name, aspect)
  // An Edit with no film yet is its first clip, and with no clip, what it's for.
  const clips = asset?.timeline?.tracks[0]?.clips
  const firstClip = clips?.find((clip) => clip.sha256 && !clip.missing)?.sha256

  const slid = view.slid !== undefined && view.slid === shown?.number
  const blank = !preview && !shown && !firstClip
  const glyph = blank && !words && !clips && faceOf(view) === "edit" && !making && field ? GLYPHS[field] : undefined
  return (
    <div className={`media ${shown?.alpha ? "alpha" : ""} ${slid ? "slid" : ""} ${blank ? "blank" : ""}`} style={{ aspectRatio: aspectOf(view) }} onAnimationEnd={() => slid && store.patch(name, { slid: undefined })}
      onPointerMove={field === "video" && !view.hereWords ? walk : undefined}
      onPointerLeave={stop}
    >
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
      {here && field === "video" && <Poster shown={here} cardWidth={cardWidth} name={name} scrubbed={view.scrub} hover={hover} />}
      {here && field !== "video" && <Thumb sha256={here.sha256} cardWidth={cardWidth} onSize={setAspect} onShown={() => store.showThumb(name)} />}
      {shown?.absent && <span className="media-note">{shown.machine ? `On ${shown.machine}` : "On another computer"}</span>}
      {!shown && firstClip && <Thumb sha256={firstClip} cardWidth={cardWidth} />}
      {blank && words && (
        <p className="words">
          {withMentions(words).map((part, i) =>
            part.mention ? (
              <span key={i} className={`chip ${asset?.missing?.includes(part.text.slice(1, -1)) || (store.mentionOf(part.text.slice(1, -1), name)?.gone ?? isSelf(part.text.slice(1, -1))) ? "missing" : ""}`}>
                {part.text.slice(1, -1)}
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
      {view.glimpse && view.viewed === undefined && (making || asset?.failure) && <Glimpse glimpse={view.glimpse} failed={!making} />}
      {(making || view.queued || view.asked !== undefined) && view.viewed === undefined && <div className="media-making" />}
      {placing && <Progress fraction={placing.done} says={stageLine(view) ?? undefined} />}
      {making && <Progress fraction={fraction} says={fraction === undefined ? undefined : leftWords(fraction, view.run?.progress?.eta_s)} />}
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
