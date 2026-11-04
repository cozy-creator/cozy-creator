import { useEffect, useRef, useState } from "react"
import { holdMedia } from "../api/events"
import type { Playback } from "../api/transport"
import { useConnection } from "../app/Connection"
import { useCanvasStore } from "./context"
import type { ElementView } from "./CanvasStore"

const STAGE: Record<string, string> = { waiting: "Waiting", reading: "Reading", sending: "Sending", adding: "Adding" }

/** Thumbnails come in these widths, so a file has few sizes in Calcifer's cache. */
function thumbWidth(cardWidth: number): number {
  const wanted = cardWidth * Math.min(2, window.devicePixelRatio || 1)
  return wanted <= 256 ? 256 : wanted <= 512 ? 512 : 1024
}

function Thumb({ sha256, cardWidth, name }: { sha256: string; cardWidth: number; name: string }) {
  const { client } = useConnection()
  const store = useCanvasStore()
  const [src, setSrc] = useState<string>()
  const projectId = store.projectId
  useEffect(() => {
    if (!projectId) return
    let live = true
    void client.transport.src(client.thumb(projectId, sha256, thumbWidth(cardWidth))).then((url) => {
      if (live) setSrc(url)
    })
    return () => {
      live = false
    }
  }, [client, projectId, sha256, cardWidth])
  if (!src) return null
  return (
    <img
      src={src}
      alt=""
      draggable={false}
      onLoad={(event) => {
        const { naturalWidth, naturalHeight } = event.currentTarget
        if (naturalWidth && naturalHeight) store.patch(name, { aspect: naturalWidth / naturalHeight })
        store.showThumb(name)
      }}
    />
  )
}

/** Files with a name in this tab that are playing, oldest first: a third stops the first. */
const playingHere: (() => void)[] = []

function Player({ field, path }: { field: string; path: string }) {
  const { client } = useConnection()
  const store = useCanvasStore()
  const [state, setState] = useState<"idle" | "waiting" | "playing" | "unplayable">("idle")
  const element = useRef<HTMLVideoElement & HTMLAudioElement>(null)
  const stopRef = useRef<() => void>(() => {})

  useEffect(() => () => stopRef.current(), [])

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
        const id = store.projectId
        if (!el || !id) return stop()
        playback = client.transport.play(client.file(id, path), el)
        setState("playing")
        el.play().catch(() => {})
      })
      .catch(() => {})
  }, [state, client, store, path])

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
          if (element.current?.getAttribute("src")) setState("unplayable")
        }}
      />
      <button type="button" className="stop nodrag" aria-label="Stop" onClick={() => stopRef.current()}>
        ×
      </button>
    </>
  )
}

export function Media({ view, name, cardWidth }: { view: ElementView; name: string; cardWidth: number }) {
  const store = useCanvasStore()
  const { asset, placing, landed, preview } = view
  const sha256 = asset?.sha256 ?? landed?.sha256
  const field = asset?.field ?? landed?.field ?? placing?.media
  const measured = asset?.width && asset.height ? asset.width / asset.height : undefined
  const aspect = view.aspect ?? measured ?? (field === "audio" ? 3 : 16 / 9)
  const path = asset?.path

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
        <Player field={field} path={path} />
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
