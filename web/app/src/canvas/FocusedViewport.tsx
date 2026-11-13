import { useEffect, useLayoutEffect, useRef, useState } from "react"
import { useCanvasStore, useElement } from "./context"
import { FocusedCard } from "./Card"
import { Icon } from "../ui/Icon"
import { LET_GO, type LetGo } from "./carry"
import type { CarriedFile } from "./CanvasStore"
import { NameField } from "./NameField"
import { failureOf, makingRun, shownOf, watched } from "./shown"
import { RunDone, RunFace, useArriving } from "./RunFace"
import { StateDot, viewState } from "./StateDot"
import { noHash } from "./text"
import { useSignal } from "../ui/signal"
import { pressBeganOutside } from "../ui/press"
import "./FocusedViewport.css"

/** What's on a focused item rather than its background. */
const OWN = ".media, .tl, .tl-player, .tl-transport, .focus-heading, .focus-warnings, .run-face, .here-side, .here-under, .image-controls, .focus-image-zoom, button, input, textarea, select, [contenteditable]"

/** A workspace for one item. The Wander camera stays behind it, unchanged. */
export function FocusedViewport({ name, onClose, onCopyVersion }: { name: string; onClose(): void; onCopyVersion(file: CarriedFile, x: number, y: number): void }) {
  const store = useCanvasStore()
  const view = useElement(name)
  const element = useRef<HTMLElement>(null)
  const entering = useSignal(store.focusEntering)
  // Drawn once it has something to show: a box on its way in has nothing yet.
  const shown = Boolean(view?.asset || view?.entity)
  useLayoutEffect(() => {
    if (!entering || !shown) return
    const frame = requestAnimationFrame(() => {
      const viewer = element.current?.querySelector<HTMLElement>(".focus-viewer")
      const room = viewer?.clientWidth && viewer.clientHeight ? viewer : element.current?.querySelector<HTMLElement>(".focus-body")
      const rect = room?.getBoundingClientRect()
      store.enterFocus(rect ? { x: rect.left, y: rect.top, width: rect.width, height: rect.height } : null)
    })
    return () => cancelAnimationFrame(frame)
  }, [entering, shown, name, store])
  useEffect(() => {
    if (!entering && store.get(name)?.hereWords) element.current?.querySelector<HTMLElement>(".focus-body [contenteditable]:not([contenteditable='false']), .focus-body textarea")?.focus({ preventScroll: true })
  }, [entering, name, store])
  useEffect(() => {
    const held = element.current
    if (!held) return
    const drop = (event: Event) => {
      const { load, x, y } = (event as LetGo).detail
      const onAsset = (event.target as Element).closest(".media, .here-under, .image-controls")
      const target = store.get(name)?.asset
      if (onAsset && target?.kind === "image" && target.tool && load.name) { event.stopPropagation(); store.takeInto(name, [load.name]) }
      else if (onAsset && target?.kind === "image" && target.tool && load.take) { event.stopPropagation(); store.takePicture(name, load.take) }
      else if (load.take) { event.stopPropagation(); onCopyVersion(load.take, x, y) }
      else if (load.tool) { event.stopPropagation(); store.putTool(name, load.tool, load.operation) }
    }
    held.addEventListener(LET_GO, drop)
    return () => held.removeEventListener(LET_GO, drop)
  }, [name, store, onCopyVersion])
  // Made, its file still on its way here: it says so as a render on its way does.
  const arriving = useArriving(view)
  // The run its render's row last spoke of: once its take lands the row stays, saying so (Paul, 11-12).
  const going = view?.asset && (view.asset.standing === "making" || Boolean(view.queued) || view.asked !== undefined) ? makingRun(view) : arriving
  const [spoke, setSpoke] = useState<{ name: string; run: number } | null>(null)
  useEffect(() => {
    if (going !== undefined) setSpoke({ name, run: going })
  }, [going, name])
  if (!view?.asset && !view?.entity) return null
  const version = shownOf(view)?.number
  // An Edit is never written as a take: what it shows is its cut, its versions joined from it (cl-957).
  const writing = Boolean(view.hereWords) && !watched(view) && !view.asset?.timeline
  // A render on its way says how between its name and its picture, large, and covers none of it (Paul, 11-12).
  const onItsWay = view.asset?.standing === "making" || Boolean(view.queued) || view.asked !== undefined
  // What its runs said (a slow run, a hot GPU: cl-913), as an open box's head says it: a ! that shows the lines.
  const warnings = view.warnings ?? []
  return (
    <section ref={element} className="focus-viewport" aria-label="Focused asset" data-focused-asset={name} data-entering={entering || undefined}
      onClick={(event) => {
        // Its background closes it: pressed and let go there, never a selection or a drag from what's on it.
        const own = (element: Element) => Boolean(element.closest(OWN))
        if (!own(event.target as Element) && pressBeganOutside(own)) onClose()
      }}
      onContextMenu={(event) => {
        if ((event.target as Element).closest("input, textarea, [contenteditable]")) return
        event.preventDefault()
        store.menu.set({ name, x: event.clientX, y: event.clientY })
      }}>
      <header className="focus-heading">
        <button type="button" className="tb" aria-label="Back to canvas" onClick={onClose}><Icon name="back" />Back to canvas</button>
        <div className="focus-title">
          {view.draft === undefined && <StateDot state={viewState(view)} unseen={view.asset?.unseen?.length ?? 0} noun={view.asset?.noun ?? "take"} />}
          {view.draft !== undefined ? <NameField name={name} draft={view.draft} onCancel={() => store.patch(name, { draft: undefined })} /> : <button type="button" className="focus-name" aria-label="Rename node" title="Rename" onClick={() => store.openName(name)}>{view.name}</button>}
          {/* Which take it is, a picture's or a video's: being written, "New take", unmistakably; none with no takes (cl-925). */}
          {(view.asset?.kind === "image" || view.asset?.kind === "video") && (version !== undefined || Boolean(view.asset.prefixes?.length) || Boolean(view.asset.sha256) || Boolean(view.asset.making?.length)) && <span className={`focus-version-label ${writing ? "writing" : ""}`} aria-label="Take">{writing ? "New take" : version === undefined ? "Rendering" : `v${version}`}</span>}
          {warnings.length > 0 && (
            <button type="button" className="iconb warned" aria-label="Warnings" aria-expanded={Boolean(view.warned)} title={noHash(warnings.join("\n"))} onClick={() => store.patch(name, { warned: !view.warned })}>
              !
            </button>
          )}
        </div>
        <button type="button" className="iconb" aria-label="Close focus" title="Close focus" onClick={onClose}><Icon name="close" /></button>
      </header>
      {view.asset?.rendering == null && !view.asset?.timeline && !failureOf(view) && (onItsWay || arriving !== undefined ? <RunFace view={view} name={name} number={onItsWay ? makingRun(view) : arriving} full above /> : spoke?.name === name && <RunDone view={view} number={spoke.run} />)}
      {view.warned && warnings.length > 0 && (
        <div className="warned-lines focus-warnings">
          {warnings.map((warning) => (
            <p key={warning}>{noHash(warning)}</p>
          ))}
        </div>
      )}
      <div className="focus-body"><FocusedCard name={name} /></div>
    </section>
  )
}
