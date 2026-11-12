import { type CSSProperties, useEffect, useState } from "react"
import { createPortal } from "react-dom"
import { useConnection } from "../app/Connection"
import { Icon } from "../ui/Icon"
import { useSignal } from "../ui/signal"
import { useCanvasStore, useElement } from "./context"
import { Player, sourceOf } from "./Media"
import { shownOf } from "./shown"

const ASPECT = 16 / 9

/** The one thing ever drawn over the canvas: the file itself, large, and nothing to do with it. Esc, a click off it or its X shuts it, and the box is as it was. The arrows walk its takes in here alone. */
export function Viewer() {
  const store = useCanvasStore()
  const name = useSignal(store.viewing)
  return name ? <Viewing key={name} name={name} close={() => store.viewing.set(null)} /> : null
}

function Viewing({ name, close }: { name: string; close: () => void }) {
  const { client } = useConnection()
  const store = useCanvasStore()
  const view = useElement(name)
  const [number, setNumber] = useState(() => (view ? shownOf(view)?.number : undefined))
  const [src, setSrc] = useState<string>()
  const [natural, setNatural] = useState<number>()
  const held = view && { ...view, viewed: number }
  const shown = held ? shownOf(held) : null
  const row = [...(view?.asset?.prefixes ?? [])].reverse()
  const source = held && sourceOf(held, shown)
  const picture = shown?.field === "image" && !shown.absent ? shown.path : undefined
  const there = Boolean(view)
  useEffect(() => {
    if (!there) close()
  }, [there, close])
  useEffect(() => {
    let live = true
    setSrc(undefined)
    if (picture && store.projectId) void client.transport.src(client.file(store.projectId, picture)).then((url) => live && setSrc(url))
    return () => {
      live = false
    }
  }, [client, store, picture])
  useEffect(() => {
    const key = (event: KeyboardEvent) => {
      if (event.key === "Escape") close()
      else if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
        const next = row[row.findIndex((version) => version.number === number) + (event.key === "ArrowLeft" ? -1 : 1)]
        if (next?.main) setNumber(next.number)
      } else return
      event.preventDefault()
      event.stopPropagation()
    }
    window.addEventListener("keydown", key, true)
    return () => window.removeEventListener("keydown", key, true)
  })
  if (!view || !shown) return null
  const aspect = natural ?? (shown.width && shown.height ? shown.width / shown.height : ASPECT)
  return createPortal(
    <div className="viewer" role="dialog" aria-label={`${name} large`} onPointerDown={(event) => event.target === event.currentTarget && close()}>
      <div className="media viewer-file" style={{ "--aspect": aspect } as CSSProperties}>
        {src && <img src={src} alt={name} onLoad={(event) => setNatural(event.currentTarget.naturalWidth / event.currentTarget.naturalHeight)} />}
        {source && shown.field === "video" && store.projectId && (
          <Player key={JSON.stringify(source)} auto field="video" path={"path" in source ? client.file(store.projectId, source.path) : undefined} machine={"run" in source ? { ...source, project: store.projectId } : undefined} />
        )}
      </div>
      <button type="button" className="iconb viewer-close" aria-label="Close" title="Close" onClick={close}>
        <Icon name="close" />
      </button>
    </div>,
    document.body,
  )
}
