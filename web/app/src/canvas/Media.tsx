import { useEffect, useRef, useState } from "react"
import { holdMedia, playingHere } from "../api/events"
import type { Playback } from "../api/transport"
import { useConnection } from "../app/Connection"
import { useCanvasStore } from "./context"
import type { ElementView } from "./CanvasStore"

const STAGE: Record<string, string> = { waiting: "Waiting", reading: "Reading", sending: "Sending", adding: "Adding" }

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

/** Thumbnails come in these widths, so a file has few sizes in Calcifer's cache. */
function thumbWidth(cardWidth: number): number {
  const wanted = cardWidth * Math.min(2, window.devicePixelRatio || 1)
  return wanted <= 256 ? 256 : wanted <= 512 ? 512 : 1024
}

function Thumb({ sha256, cardWidth, name }: { sha256: string; cardWidth: number; name: string }) {
  const { client } = useConnection()
  const store = useCanvasStore()
  const [src, setSrc] = useState<string>()
  const [failed, setFailed] = useState(false)
  const projectId = store.projectId
  const leaveLine = useRef(() => {})
  const width = thumbWidth(cardWidth)
  useEffect(() => {
    if (!projectId) return
    let live = true
    leaveLine.current = thumbLine(() => {
      void client.transport.src(client.thumb(projectId, sha256, width)).then((url) => {
        if (live) setSrc(url)
      })
    })
    return () => {
      live = false
      leaveLine.current()
    }
  }, [client, projectId, sha256, width])
  if (failed) return <span className="media-note">No preview</span>
  if (!src) return null
  return (
    <img
      src={src}
      alt=""
      draggable={false}
      onError={() => {
        leaveLine.current()
        setFailed(true)
      }}
      onLoad={(event) => {
        leaveLine.current()
        const { naturalWidth, naturalHeight } = event.currentTarget
        if (naturalWidth && naturalHeight) store.patch(name, { aspect: naturalWidth / naturalHeight })
        store.showThumb(name)
      }}
    />
  )
}

/** A file Calcifer serves, playing on a real URL with Range: one of the two live ones across tabs. */
export function Player({ field, path }: { field: string; path: string }) {
  const { client, stream } = useConnection()
  const [state, setState] = useState<"idle" | "waiting" | "playing" | "unplayable">("idle")
  const element = useRef<HTMLVideoElement & HTMLAudioElement>(null)
  const stopRef = useRef<() => void>(() => {})

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

  useEffect(() => {
    if (state !== "waiting") return
    const waiting = new AbortController()
    let release: (() => void) | null = null
    let playback: Playback | null = null
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
      .then((held) => {
        release = held
        const el = element.current
        if (!el) return stop()
        playback = client.transport.play(path, el)
        setState("playing")
        el.play().catch(() => {})
      })
      .catch(() => {})
  }, [state, client, path])

  if (state === "unplayable") return <span className="media-note">This browser can't play it</span>
  if (state === "idle") {
    return (
      <button type="button" className="play nodrag" aria-label="Play" onClick={() => setState("waiting")}>
        ▶
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
          if (el?.getAttribute("src") && el.error?.code !== MediaError.MEDIA_ERR_NETWORK && stream.health.get().state === "live") setState("unplayable")
        }}
      />
      <button type="button" className="stop nodrag" aria-label="Stop" onClick={() => stopRef.current()}>
        ×
      </button>
    </>
  )
}

export function Media({ view, name, cardWidth }: { view: ElementView; name: string; cardWidth: number }) {
  const { client } = useConnection()
  const store = useCanvasStore()
  const { asset, placing, landed, preview } = view
  const sha256 = asset?.sha256 ?? landed?.sha256
  const field = asset?.field ?? landed?.field ?? placing?.media
  const measured = asset?.width && asset.height ? asset.width / asset.height : undefined
  const aspect = view.aspect ?? measured ?? (field === "audio" ? 3 : 16 / 9)
  // A project's file, by its hash. Anything else from a state isn't followed: a remote Calcifer's answers are hostile.
  const path = asset?.path && /^files\/[0-9a-f]{64}(\.\w+)?$/.test(asset.path) ? asset.path : undefined

  return (
    <div className={`media ${field === "image" ? "alpha" : ""}`} style={{ aspectRatio: aspect }}>
      {preview && (
        <img
          src={preview}
          alt=""
          draggable={false}
          onLoad={(event) => {
            const { naturalWidth, naturalHeight } = event.currentTarget
            if (naturalWidth && naturalHeight) store.patch(name, { aspect: naturalWidth / naturalHeight })
          }}
        />
      )}
      {sha256 && <Thumb sha256={sha256} cardWidth={cardWidth} name={name} />}
      {!preview && !sha256 && <span className="media-note">{field === "file" ? "A file" : field ?? asset?.kind}</span>}
      {placing && (
        <div className="prog">
          <i style={{ width: `${Math.round(placing.done * 100)}%` }} />
        </div>
      )}
      {!placing && sha256 && path && (field === "video" || field === "audio") && (
        <Player field={field} path={client.file(store.projectId!, path)} />
      )}
    </div>
  )
}

export const stageLine = (view: ElementView): string | null => {
  const placing = view.placing
  if (!placing) return null
  const percent = placing.stage === "reading" || placing.stage === "sending" ? ` ${Math.round(placing.done * 100)}%` : ""
  return `${STAGE[placing.stage]}${percent}${placing.attempt ? ` · attempt ${placing.attempt}/5` : ""}`
}
