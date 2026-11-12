import { useEffect, useRef, useState } from "react"
import { parseLink, play } from "../../../../player/cozy-webrtc.js"
import { useConnection } from "../../app/Connection"
import { Player } from "../../canvas/Media"
import type { OutputItem, Run } from "../../api/calcifer"
import type { Client } from "../../api/client"
import { Failure, useAsk } from "../common"
import { bytes, span } from "../format"

const mediaKind = (item: OutputItem) => {
  const [kind] = item.media_type.split("/")
  return kind === "image" || kind === "video" || kind === "audio" ? kind : "file"
}

/** Whether its bytes landed in Calcifer, so its route serves them. */
const landed = (item: OutputItem) => Boolean(item.url)
type OutputProps = { run: Run; item: OutputItem; project: string | null }
const outputPath = (client: Client, { project, run, item }: OutputProps) => project
  ? client.output({ project, run: run.id }, item.name, item.index)
  : client.maintenanceOutput(run.id, item.name, item.index)

/** A URL for a tag that can't send a header. Only an API path is ever followed. */
function useSrc(path: string | null): string | undefined {
  const { client } = useConnection()
  const [src, setSrc] = useState<string>()
  useEffect(() => {
    if (!path) return setSrc(undefined)
    let live = true
    void client.transport.src(path).then((url) => {
      if (live) setSrc(url)
    })
    return () => {
      live = false
    }
  }, [client, path])
  return src
}

function Picture({ run, item, project }: OutputProps) {
  const { client } = useConnection()
  const full = outputPath(client, { run, item, project })
  const [useThumb, setUseThumb] = useState(Boolean(project && item.sha256))
  const src = useSrc(useThumb ? client.thumb(project!, item.sha256!, 512) : full)
  if (!src) return null
  return <img src={src} alt={item.name} onError={() => setUseThumb(false)} />
}

/** A file served here. Its poster is a thumbnail, when its project can make one. */
function Landed({ run, item, project, kind }: OutputProps & { kind: "video" | "audio" }) {
  const { client } = useConnection()
  const poster = useSrc(kind === "video" && project && item.sha256 ? client.thumb(project, item.sha256, 512) : null)
  const aspect = item.width && item.height ? item.width / item.height : kind === "audio" ? 6 : 16 / 9
  return (
    <div className="media" style={{ aspectRatio: aspect }}>
      {poster && <img src={poster} alt="" />}
      <Player field={kind} path={outputPath(client, { run, item, project })} />
    </div>
  )
}

/** Straight from its machine over WebRTC: a video that hasn't landed here, or never will. */
function FromMachine({ run, item, project }: OutputProps) {
  const { client } = useConnection()
  const ask = useAsk()
  const video = useRef<HTMLVideoElement>(null)
  const player = useRef<ReturnType<typeof play> | null>(null)
  const [status, setStatus] = useState("")
  const [open, setOpen] = useState(false)

  useEffect(() => () => player.current?.close(), [])

  const start = () =>
    ask.run(async () => {
      const answer = await (project ? client.play({ project, run: run.id }, item.name, item.index) : client.maintenancePlay(run.id, item.name, item.index))
      setOpen(true)
      const el = video.current
      if (!el) return
      player.current?.close()
      // Its link's fragment is all that's read of what a machine's Calcifer said, and it's checked as it's read.
      player.current = play(el, parseLink(new URL(answer.url).hash))
      player.current.addEventListener("status", (event) => {
        const { state, detail } = (event as CustomEvent<{ state: string; detail: string }>).detail
        setStatus(state === "error" ? detail : state === "connected" ? "" : `${state === "connecting" ? "Connecting to its machine" : "Reconnecting"}…`)
      })
      el.addEventListener("canplay", () => el.play().catch(() => {}), { once: true })
    })

  return (
    <>
      <video ref={video} className={open ? "video" : "unseen"} controls muted playsInline />
      {!open && (
        <button type="button" className="tb" disabled={ask.busy} onClick={() => void start()}>
          Play from {run.machine ?? "its machine"}
        </button>
      )}
      {status && <p className="muted">{status}</p>}
      <Failure error={ask.error} lead="Can't play it from its machine" />
    </>
  )
}

function Output({ run, item, project }: OutputProps) {
  const { client } = useConnection()
  const kind = mediaKind(item)
  const here = landed(item)
  const name = item.label || (item.index ? `${item.name} ${item.index}` : item.name)
  const save = useSrc(outputPath(client, { run, item, project }))
  const facts = [
    item.length ? bytes(item.length) : "",
    item.width && item.height ? `${item.width}×${item.height}` : "",
    item.duration_ms ? span(item.duration_ms / 1000) : "",
  ].filter(Boolean)
  return (
    <figure className="output">
      {!item.checkpoint && here && kind === "image" && <Picture run={run} item={item} project={project} />}
      {!item.checkpoint && here && (kind === "video" || kind === "audio") && <Landed run={run} item={item} project={project} kind={kind} />}
      {!item.checkpoint && !here && kind === "video" && run.machine && !item.export_error && <FromMachine run={run} item={item} project={project} />}
      {!item.checkpoint && !here && kind !== "video" && !item.export_error && <p className="muted">Not here yet.</p>}
      {item.checkpoint !== undefined && <p className="muted">A checkpoint in its machine's store, with no file here.</p>}
      {item.export_error && <p className="warn">It won't land here: {item.export_error}</p>}
      <figcaption>
        <span className="nm">{name}</span>
        <small>{facts.join(" · ")}</small>
        <span className="sp" />
        {here && save && (
          <a className="more" href={save} download>
            Save
          </a>
        )}
      </figcaption>
    </figure>
  )
}

/** What a person is told of: what stays on its machine is a package's own bytes. */
export function Outputs({ run, project }: { run: Run; project: string | null }) {
  const items = run.output?.filter((item) => !item.stays) ?? []
  if (!items.length) return null
  return (
    <div className="outputs">
      {items.map((item) => (
        <Output key={item.id} run={run} item={item} project={project} />
      ))}
    </div>
  )
}
