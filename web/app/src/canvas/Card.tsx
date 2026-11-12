import { useQuery } from "@tanstack/react-query"
import type { NodeProps, Viewport } from "@xyflow/react"
import { useReactFlow } from "@xyflow/react"
import { type CSSProperties, type MouseEvent, memo, type PointerEvent, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react"
import { flushSync } from "react-dom"
import { runQuery } from "../api/queries"
import { useConnection } from "../app/Connection"
import { useWaitsOn } from "../panels/machines/activity"
import type { MainFile, Tool, ToolField } from "../api/calcifer"
import { Icon } from "../ui/Icon"
import { say } from "../ui/notes"
import { useSignal } from "../ui/signal"
import { AssetEdit, InFrame, RenderButton, TextAsset } from "./AssetEdit"
import type { CanvasStore, CarriedFile, ElementNode, ElementView } from "./CanvasStore"
import { carryOnMove } from "./carry"
import { useCanvasStore, useElement } from "./context"
import { fieldsOf, shapesOf, shapeWords, snapped } from "./controls"
import { useDragScroll } from "./dragScroll"
import { downloadable, Media, Preview, preload, Thumb, usePreview } from "./Media"
import { NameField } from "./NameField"
import { ORBIT, orbitOf } from "./Orbit"
import { bare, ContinueFrom, LeadIn, Segments, VideoTransport } from "./Segments"
import { aspectOf, bodyBox, bodyWidth, faceOf, heightFor, isPicture, madeHere, makes, makingRun, NARROWEST, paints, ratioOf, reshapes, shownOf, takingOf, toolName, valuesOf, watched, widthFor } from "./shown"
import { chipWords, noHash, piecesOf, rowsIn, took } from "./text"
import { VideoEditor } from "./timeline/VideoEditor"
import { AddOns } from "./AddOns"
import { ToolLine } from "./ToolLine"
import { FocusFrame, useFocusResize } from "./FocusFrame"
import { Asking } from "./Asking"
import { readableSurface } from "./readableSurface"
import { RunFace, RunMeter, useArriving } from "./RunFace"
import { StateDot, viewState } from "./StateDot"
import type { Box } from "./graph"

const WIDEST = 1600
const LOWEST = 90
/** How wide a picture is asked for while the canvas is drawn in on it. */
const HERE_THUMB = 960

function kindOf(view: ElementView): string {
  if (view.placing || view.landed) return "file"
  const asset = view.asset
  if (!asset) return ""
  if (madeHere(asset.tool)) return asset.kind === "video" ? "edit" : "frame"
  if (asset.field && !asset.tool) return asset.kind
  return asset.kind === "video" ? "shot" : asset.kind
}

/** How far a take being made is: its package installing or its weights arriving show as such, never as its own percentage. */
function TakeProgress({ view, run }: { view: ElementView; run: number }) {
  // Its own run, each tile (cl-945): the box follows only the one its frame is on.
  const { client } = useConnection()
  const project = useCanvasStore().projectId
  const framed = view.run?.number === run
  const { data } = useQuery({ ...runQuery(client, project ?? "", run), enabled: Boolean(project) && !framed })
  const mine = framed ? view.run : data
  const on = useWaitsOn(mine)
  if (on) return <Icon name={on.kind === "download" ? "download" : "package"} />
  const fraction = mine?.progress?.done_fraction
  const said = fraction == null ? "…" : `${Math.round(fraction * 100)}%`
  // What it has made so far plays on its tile, silent (cl-907): a pick of the tile plays it with its sound.
  const glimpse = view.glimpse?.media === "video" && view.glimpse.number === run ? view.glimpse : undefined
  return glimpse ? <TileGlimpse glimpse={glimpse} said={said} /> : <>{said}</>
}

function TileGlimpse({ glimpse, said }: { glimpse: NonNullable<ElementView["glimpse"]>; said: string }) {
  const { client } = useConnection()
  const store = useCanvasStore()
  const [src, setSrc] = useState<string>()
  useEffect(() => {
    if (!store.projectId) { setSrc(undefined); return }
    let live = true
    void client.transport.src(client.output({ project: store.projectId, run: glimpse.run }, glimpse.output, glimpse.index, glimpse.rev)).then((url) => live && setSrc(url))
    return () => {
      live = false
    }
  }, [client, store.projectId, glimpse.run, glimpse.output, glimpse.index, glimpse.rev])
  return (
    <>
      {src && <video className="take-glimpse" src={src} muted loop autoPlay playsInline preload="metadata" />}
      <span className="take-said">{said}</span>
    </>
  )
}

/** A button on a card does its one thing, and a field takes the click: neither selects it. */
export function buttonsOnly(event: MouseEvent) {
  if ((event.target as Element).closest("button, input, select, .mention-text")) event.stopPropagation()
}

type Version = NonNullable<NonNullable<ElementView["asset"]>["prefixes"]>[number]

/** A take carried out: it lands as a box of its own, or as `<name.N>` in words. It's picked up from its tile or the big one, and an Edit's from its row, as its big tile is its clips. */
function taken(store: ReturnType<typeof useCanvasStore>, name: string, asset: NonNullable<ElementView["asset"]>, version: Version & { main: NonNullable<Version["main"]> }): CarriedFile {
  const description = store.valueOf(name, "description")
  return { sha256: version.main.sha256, field: version.main.field, name: `${asset.name}_${version.number}`, description: typeof description === "string" ? description : undefined, from: asset.name, number: version.number, ...(asset.kind === "image" ? { copy: true } : {}) }
}

/** The take shown, as words mention it: `<Kyogre_3.2>`, its one name. A click copies it; dragged onto a box or into a prompt it's a mention,
 *  as a box dragged is. */
function TakeName({ view }: { view: ElementView }) {
  const number = shownOf(view)?.number
  if (!view.asset || number === undefined) return null
  const name = `${view.name}.${number}`
  return (
    <div className="take-name">
      <button
        type="button"
        className="chip nodrag"
        title={`Copy <${name}>`}
        onPointerDown={(event) => carryOnMove(event.nativeEvent, { name }, event.currentTarget)}
        onClick={() => void navigator.clipboard?.writeText(`<${name}>`).then(() => say(`Copied <${name}>`), () => say(`<${name}>`))}
      >
        {chipWords(name)}
      </button>
    </div>
  )
}

/** A take on the row: a video's preview plays while the pointer rests on it. */
function TakeThumb({ main, width, onShown }: { main: MainFile; width: number; onShown?: () => void }) {
  const { on, hover } = usePreview(main.field === "video")
  return (
    <span className="take-thumb" {...hover}>
      <Thumb sha256={main.sha256} cardWidth={width} onShown={onShown} />
      {on && <Preview sha256={main.sha256} />}
    </span>
  )
}

/** Its next take, offered once it has any, made, on its way or failed: only a box with none hides it, its words being what's asked (Paul,
 *  11-12, cl-925 and cl-941). An Edit's + is its words. */
function nextTake(store: CanvasStore, view: ElementView, name: string, tools: Map<string, Tool>) {
  const asset = view.asset!
  const making = asset.making ?? []
  const waiting = !making.length && (view.queued || view.asked !== undefined)
  const writes = paints(view, tools)
  const any = Boolean(asset.prefixes?.length) || Boolean(asset.sha256) || Boolean(asset.timeline) || takingOf(view).length > 0 || making.length > 0 || Boolean(waiting) || Boolean(asset.failure)
  const adds = (writes || (asset.kind === "video" && Boolean(asset.tool))) && any
  const write = () => {
    // Already on them, the caret goes back in.
    const prompt = view.hereWords ? document.querySelector<HTMLElement>(`.card.here[data-asset="${CSS.escape(name)}"] .in-frame .mention-text`) : null
    if (writes && prompt) {
      prompt.focus()
      window.getSelection()?.selectAllChildren(prompt)
      window.getSelection()?.collapseToEnd()
      return
    }
    if (writes) return store.writeNext(name)
    store.focusFit?.()
    store.focusing = name
    store.patch(name, { hereWords: true, continuing: undefined, continuingFps: undefined, watch: undefined })
  }
  const label = writes ? "Its next take" : asset.timeline ? "Its words" : "New video"
  return { adds, write, label, title: writes ? "New image" : "New video", tool: tools.get(asset.tool ?? "")?.label, on: Boolean(view.hereWords), word: writes || asset.kind === "video" }
}

function NewTake({ next }: { next: ReturnType<typeof nextTake> }) {
  return (
    <button type="button" className={`take add ${next.on ? "on" : ""}`} aria-label={next.label} title={next.title} onClick={next.write}>
      +{next.word && <small className="take-new">New · {next.tool}</small>}
    </button>
  )
}

/** In Focus, on the title row above the frame's right edge, the same for every tool (Paul, 11-12): New, then Render, never hidden while a
 *  take renders, so another can be queued. */
function FocusActions({ view, name }: { view: ElementView; name: string }) {
  const store = useCanvasStore()
  const tools = useSignal(store.tools)
  if (!view.asset?.tool) return null
  const next = nextTake(store, view, name, tools)
  return (
    <div className="focus-actions nodrag nopan">
      {next.adds && (
        <button type="button" className={`tb take add focus-new ${next.on && !watched(view) ? "on" : ""}`} aria-label={next.label} title={next.title} onClick={next.write}>
          <Icon name="plus" />
          {next.label === "Its words" ? "Its words" : "New take"}
        </button>
      )}
      <RenderButton name={name} view={view} />
    </div>
  )
}

/** Each take or version, the newest first and what's still being made before that: a click previews it; a double tap approves an image.
 *  Beside an open box they're a column, and a drag up or down scrolls it; an Edit's are a row under it, which drags across. A drag any other way carries a
 *  take out. + first goes to its words: an Edit's, or the next take an image tool writes in the frame. An Edit's clips are what a take holds, so they stay
 *  with it unless it's approved. */
function Takes({ view, name, width, side, focused = false }: { view: ElementView; name: string; width: number; side?: boolean; focused?: boolean }) {
  const store = useCanvasStore()
  const tools = useSignal(store.tools)
  const asset = view.asset!
  const list = useDragScroll({
    axis: side ? "y" : "x",
    lift(pressed) {
      const from = pressed.closest<HTMLElement>(".take[data-take]")
      const version = from && asset.prefixes?.find((held) => held.number === Number(from.dataset.take))
      return from && version?.main ? { load: { take: taken(store, name, asset, { ...version, main: version.main }) }, from } : null
    },
  })
  const onWords = view.hereWords && !(asset.timeline && !asset.pinned)
  const watch = watched(view)
  // Rendered from its words, what's being made is what's looked at.
  const shown = onWords || watch ? null : shownOf(view)
  const versions = [...(asset.prefixes ?? [])].reverse()
  // Each take on its way by its id, from the click to its version (cl-945); a run no take names, and what's asked with no take named
  // (Render all, before Calcifer lists it), as they come.
  const pending = takingOf(view)
  const making = (asset.making ?? []).filter((run) => !pending.some((take) => take.run === run))
  const waiting = !pending.length && !making.length && (view.queued || view.asked !== undefined)
  // The newest take, once its tile's up, is made ready at the size it's looked at, before it's clicked.
  const { client } = useConnection()
  const newest = asset.prefixes?.at(-1)
  const next = nextTake(store, view, name, tools)
  if (!next.adds && !versions.length && !view.joining && !pending.length && !making.length && !waiting) return null
  const pick = (number: number) => {
    store.pickTake(name, number)
    if (onWords || watch) store.patch(name, { hereWords: undefined, watch: undefined })
  }
  const row = (
    <div
      ref={side ? undefined : list}
      className="takes nodrag nopan nowheel"
      onWheel={(event) => {
        if (!side) event.currentTarget.scrollLeft += event.deltaX || event.deltaY
      }}
    >
      {next.adds && !focused && <NewTake next={next} />}
      {waiting && (
        <span className="take-at">
          <button type="button" className="take making" title="Queued">
            …
          </button>
          <button type="button" className="take-off" aria-label="Cancel what's queued" title="Cancel what's queued" onClick={() => store.cancelRender(name)}>
            <Icon name="trash" />
          </button>
        </span>
      )}
      {pending.map((take) => {
        // One that failed stays, red, its error a click away, until it's deleted (cl-954).
        const failed = take.failure ? take.failure.message.replace(`${name}: `, "") : undefined
        const state = failed !== undefined ? "failed" : take.run !== undefined ? "running" : "queued"
        return (
          <span key={take.id} className="take-at">
            <button type="button" className={`take making ${state === "failed" ? "failed" : ""} ${watch?.id === take.id ? "on" : ""}`} data-take-id={take.id} data-run={take.run} data-state={state} title={failed !== undefined ? `Didn't render: ${failed}` : take.run !== undefined ? `Run ${take.run}` : "Queued"} onClick={() => store.watchTake(name, take.id)}>
              <span className={`state-dot ${state}`} role="img" aria-label={state === "failed" ? "Failed" : state === "running" ? "Rendering" : "Queued"} />
              {failed !== undefined ? <span className="take-failed">Failed</span> : take.run !== undefined ? <TakeProgress view={view} run={take.run} /> : "…"}
            </button>
            {failed !== undefined ? (
              <button type="button" className="take-off" aria-label="Delete this failed take" title="Delete" onClick={() => store.cancelTake(name, take)}>
                <Icon name="trash" />
              </button>
            ) : (
              <button type="button" className="take-off" aria-label={take.run !== undefined ? `Cancel run ${take.run}` : "Cancel what's queued"} title={take.run !== undefined ? `Cancel run ${take.run}` : "Cancel this take"} onClick={() => store.cancelTake(name, take)}>
                <Icon name="close" />
              </button>
            )}
          </span>
        )
      })}
      {making.map((run) => (
        <span key={run} className="take-at">
          <button type="button" className="take making" title={`Run ${run}`}>
            <TakeProgress view={view} run={run} />
          </button>
          <button type="button" className="take-off" aria-label={`Cancel run ${run}`} title={`Cancel run ${run}`} onClick={() => store.cancelTake(name, { id: "", run })}>
            <Icon name="close" />
          </button>
        </span>
      ))}
      {versions.map((version) => (
        <span key={version.number} className={`take-at ${asset.kind === "image" ? "image-version-entry" : ""}`}>
          {asset.kind === "image" && <span className="variant-number" aria-hidden="true">v{version.number}</span>}
          <button
            type="button"
            className={`take ${shown?.number === version.number ? "on" : ""} ${asset.pinned && version.number === asset.approved ? "approved" : ""} ${view.slid === version.number ? "fresh" : ""}`}
            aria-label={`${asset.noun} ${version.number}`}
            aria-pressed={shown?.number === version.number}
            title={[`${asset.noun} ${version.number}`, version.execution_ms ? took(version.execution_ms) : "", version.pickup ? `picked up from ${version.pickup.take.slice(1, -1)} at frame ${version.pickup.out_frame}` : "", version.number === asset.approved ? "approved" : "", asset.unseen?.includes(version.number) ? "not looked at yet" : ""].filter(Boolean).join(", ")}
            data-take={version.number}
            data-take-id={version.id || undefined}
            style={asset.kind === "image" ? { height: 100, width: Math.min(220, 100 * (version.main?.width && version.main.height ? version.main.width / version.main.height : 1.5)), aspectRatio: "auto" } : undefined}
            onClick={() => pick(version.number)}
            onDoubleClick={() => store.toggleApproved(name, version.number)}
            onContextMenu={(event) => {
              if (!version.main || !downloadable(version.main)) return
              event.preventDefault()
              event.stopPropagation()
              store.menu.set({ name, x: event.clientX, y: event.clientY, number: version.number })
            }}
          >
            {version.main && !version.main.absent ? (
              <TakeThumb main={version.main} width={width} onShown={version === newest && version.main.field === "image" ? () => preload(client, store.projectId, version.main!.sha256) : undefined} />
            ) : null}
            {asset.unseen?.includes(version.number) && <span className="take-unseen" role="img" aria-label="Not looked at yet" />}
          </button>
          <button type="button" className="take-off" aria-label={`Discard ${asset.noun} ${version.number}`} title={`Discard ${asset.noun} ${version.number}`} onClick={() => store.discardTake(name, version.number)}>
            <Icon name="trash" />
          </button>
        </span>
      ))}
      {Array.from({ length: view.joining ?? 0 }, (_, i) => (
        <span key={i} className="take making">
          …
        </span>
      ))}
    </div>
  )
  return side ? (
    <div ref={list} className={`here-side ${asset.kind === "image" ? "image-history" : ""} nodrag nopan nowheel`}>
      {row}
    </div>
  ) : (
    row
  )
}

/** Orbit viewing for the take shown, as its file says or as a person turns it on (Paul, 11-11): on, a drag turns the video and no play
 *  button plays it. It's what the file says of itself, kept with the take, so it's undone like any edit and puts nothing out of date. */
function OrbitToggle({ view, name }: { view: ElementView; name: string }) {
  const store = useCanvasStore()
  const shown = shownOf(view)
  if (!shown || shown.field !== "video" || view.hereWords || !view.asset) return null
  const on = Boolean(orbitOf(shown))
  const which = shown.number === undefined ? name : `${view.asset.noun} ${shown.number} of ${name}`
  const flip = (want: boolean) => {
    const rest = Object.fromEntries(Object.entries(shown.metadata ?? {}).filter(([key]) => key !== "orbit"))
    const metadata = want ? { ...rest, orbit: ORBIT } : rest
    store.setMetadata(name, shown.number, metadata, `Turned ${want ? "on" : "off"} orbit viewing for ${which}`)
  }
  return (
    <label className="row orbit-toggle">
      <input className="nodrag" type="checkbox" role="switch" aria-label="Orbit: drag to turn" checked={on} onChange={(event) => flip(event.target.checked)} />
      <span>Orbit</span>
      <span className="muted">{on ? "Drag to turn" : "Plays"}</span>
    </label>
  )
}

/** The rows of a group that adds up, each as wide as it lasts, or a cut's pieces. Read only: a row goes to the box's words, a piece plays from there. */
function Bar({ view, name }: { view: ElementView; name: string }) {
  const store = useCanvasStore()
  const tools = useSignal(store.tools)
  const group = view.asset && fieldsOf(view.asset, tools).find((field) => field.type === "group" && field.members?.some((member) => member.total))
  const total = group?.members?.find((member) => member.total)
  const rows = group ? rowsIn(valuesOf(view)[group.field]) : []
  if (total && rows.length) {
    const lasts = (row: Record<string, unknown>) => Number(row[total.field]) || Number(total.default) || 1
    return (
      <div className="beats">
        {rows.map((row, i) => (
          <button key={i} type="button" className="nodrag" style={{ flexGrow: lasts(row) }} onClick={() => store.patch(name, { hereWords: true })}>
            {lasts(row)}
            {total.unit ?? ""}
          </button>
        ))}
      </div>
    )
  }
  const pieces = piecesOf(valuesOf(view))
  if (!pieces.length) return null
  const lasts = pieces.map((piece) => {
    const held = store.get(piece)
    return (held && shownOf(held)?.duration_ms) || 0
  })
  return (
    <div className="beats">
      {pieces.map((piece, i) => (
        <button
          key={i}
          type="button"
          className="nodrag"
          style={{ flexGrow: lasts[i] || 1 }}
          title={piece}
          onClick={() => store.seek(name, lasts.slice(0, i).reduce((sum, ms) => sum + ms, 0) / 1000)}
        >
          {piece}
        </button>
      ))}
    </div>
  )
}

function Act({ label, onClick }: { label: string; onClick: () => void }) {
  return (
    <button type="button" className="act nodrag" onClick={onClick}>
      {label}
    </button>
  )
}

function usdPerHour(micros: number): string {
  return `$${(micros / 1_000_000).toFixed(2)}/h`
}

/** What it says under its media: how far an import or a run is, and what's wrong and what to do about it. Never a hash. */
function Status({ view, name }: { view: ElementView; name: string }) {
  const store = useCanvasStore()
  const will = store.nameWillBe(name)
  if (will) return <div className="status">Will be {will}</div>
  const asset = view.asset
  if (!asset) return null
  if (view.asking) return <Asking name={name} fit={view.asking.fit} />
  if (view.waiting) {
    return (
      <div className="status mak">
        Waits for a machine <Act label="Machines" onClick={store.askMachine} />
      </div>
    )
  }
  // How far it's got is on its picture, in one line, and an Edit's film on its own bar: here, only what's to be done about it.
  if (asset.standing === "making") {
    const stalled = view.run?.stalled
    if (!stalled) return null
    const since = took(Math.max(0, Date.now() - stalled.since_ms))
    return (
      <div className="status bad">
        Stalled {since} on {stalled.machine}
        {stalled.usd_micros_per_hour != null && ` · ${usdPerHour(stalled.usd_micros_per_hour)}`} <Act label="Cancel" onClick={() => store.cancelRender(asset.name)} />
      </div>
    )
  }
  if (view.queued || view.asked !== undefined) return null
  // A failure is its toast; unavailable media is labelled by the media view.
  return null
}

/** The corner changes output shape freely within its tool's bounds, or to its declared ratios. Focus fits the result only once the pull ends. */
function Corner({ name, view, width, focused = false }: { name: string; view: ElementView; width: number; focused?: boolean }) {
  const store = useCanvasStore()
  const focusResize = useFocusResize()
  const flow = useReactFlow()
  const tools = useSignal(store.tools)
  const control = view.asset && reshapes(view) ? fieldsOf(view.asset, tools).find((held) => held.type === "shape") : undefined
  const validRatio = (ratio: unknown): ratio is string => typeof ratio === "string" && ratio.split(":").length === 2 && ratio.split(":").map(Number).every((value) => value > 0 && Number.isFinite(value))
  const ratios = (control?.ratios ?? []).filter(validRatio)
  const dynamic = Boolean(control?.auto) && ratios.length !== 1
  const fallback = control?.default
  const defaultShape = ratios.length === 1 ? ratios[0] : validRatio(fallback) ? fallback : ratios[0] ?? "1:1"
  const held = control ? store.valueOf(name, control.field) : undefined
  const override = validRatio(held) ? held : null
  const hasOverride = held !== undefined && held !== null && held !== ""
  const badge = Boolean(control && view.hereWords && (view.asset?.kind === "image" || view.asset?.kind === "video"))
  const from = useRef<{ x: number; y: number; w: number; h: number; pixels?: { w: number; h: number }; inverse: number; owner: CanvasStore["inPlace"] } | null>(null)
  const pulled = useRef<{ w: number; h: number; shape?: string } | null>(null)
  const [nearest, setNearest] = useState<string | null>(null)
  const label = nearest ?? override ?? (dynamic ? autoWords(view) : defaultShape)
  // Pulled, it snaps to the nearest ratio its tool makes: its own list, else the usual ones within its bounds; never an arbitrary A:B (Paul, 11-12).
  const shapeFor = (width: number, height: number) => {
    const allowed = ratios.length ? ratios : control ? shapesOf(control) : []
    if (allowed.length) return snapped(width / height, allowed)
    let w = Math.max(1, Math.round(width)), h = Math.max(1, Math.round(height))
    const ratio = Math.min(control?.max ?? Infinity, Math.max(control?.min ?? 0, w / h))
    if (ratio !== w / h) { w = Math.round(ratio * 1_000_000); h = 1_000_000 }
    let a = w, b = h
    while (b) [a, b] = [b, a % b]
    return `${w / a}:${h / a}`
  }
  const moved = (event: PointerEvent) => {
    const start = from.current
    if (!start) return
    flushSync(() => {
      const [dx, dy] = [(event.clientX - start.x) * start.inverse, (event.clientY - start.y) * start.inverse]
      if (!control) {
        const w = Math.min(WIDEST, Math.max(NARROWEST, start.w * Math.max(1 + dx / bodyWidth(view, start.w), 1 + dy / start.h)))
        pulled.current = { w, h: w / aspectOf(view) }
        return store.setWidth(name, w, false)
      }
      const w = Math.min(WIDEST, Math.max(NARROWEST, start.w + dx))
      const requestedHeight = Math.min(WIDEST, Math.max(LOWEST, start.h + dy))
      const shape = shapeFor(w, requestedHeight)
      const [rw, rh] = shape.split(":").map(Number)
      const h = w * rh / rw
      pulled.current = { w, h, shape }
      store.drawShape(name, w, h)
      if (focused && start.pixels) focusResize?.move(start.pixels.w * w / start.w, start.pixels.h * h / start.h)
      setNearest(shape)
    })
  }
  const done = () => {
    const start = from.current
    if (!start) return
    flushSync(() => {
      const owner = start.owner
      from.current = null
      const size = pulled.current
      pulled.current = null
      setNearest(null)
      if (size && control) store.setShape(name, control, focused ? undefined : size.w, size.shape ?? shapeFor(size.w, size.h))
      else if (size) store.setWidth(name, size.w)
      if (owner && store.inPlace === owner) owner.pulled = false
      if (focused) { focusResize?.end(); return }
      if (!owner || store.inPlace !== owner) return
      requestAnimationFrame(() => {
        if (store.inPlace === owner) store.fitHere(true)
      })
    })
  }
  const resetShape = () => {
    if (!control) return
    flushSync(() => {
      from.current = null
      pulled.current = null
      setNearest(null)
      store.patch(name, { shape: undefined })
      if (dynamic) store.unshape(name, control.field)
      else void store.setField(name, control.field, defaultShape)
      focusResize?.end()
    })
  }
  return (
    <>
      {badge && <span className="ratio-rest" data-output-aspect={label} title="Output aspect ratio">
        {label}
        {hasOverride && <button type="button" className="ratio-clear nodrag nopan" aria-label={dynamic ? "Auto aspect ratio" : `Reset aspect ratio to ${defaultShape}`} title={dynamic ? "Back to Auto" : `Reset to ${defaultShape}`} onPointerDown={(event) => event.stopPropagation()} onClick={(event) => { event.stopPropagation(); resetShape() }}><Icon name="close" /></button>}
      </span>}
      {nearest && !badge && (
        <div className="ratio-live">
          <span>{nearest}</span>
        </div>
      )}
      <div
        className="corner nodrag nopan"
        role="slider"
        aria-label="Its size"
        aria-valuenow={Math.round(width)}
        tabIndex={-1}
        onClick={(event) => event.stopPropagation()}
        onPointerDown={(event) => {
          if (event.button !== 0) return
          event.stopPropagation()
          const pose = store.poseOf(name)
          if (!pose) return
          event.currentTarget.setPointerCapture(event.pointerId)
          // Canvas pixels per screen pixel: the zoom's, or focused, the frame's against its box.
          const frame = event.currentTarget.closest(".media")?.getBoundingClientRect()
          const inverse = focused && frame?.width ? pose.w / frame.width : 1 / flow.getZoom()
          from.current = { x: event.clientX, y: event.clientY, w: pose.w, h: bodyBox(view, pose).w / aspectOf(view), inverse, pixels: focused && frame ? { w: frame.width, h: frame.height } : undefined, owner: store.inPlace }
          if (focused && control) focusResize?.start()
          if (store.inPlace) store.inPlace.pulled = true
        }}
        onPointerMove={moved}
        onPointerUp={done}
        onPointerCancel={done}
        onLostPointerCapture={done}
      />
    </>
  )
}

/** Auto, as what it makes: `Auto · 9:16 from first frame` (se-128). */
const autoWords = (view: ElementView) => (view.auto ? `Auto · ${shapeWords(view.auto)}${view.autoFrom ? ` from ${view.autoFrom}` : ""}` : "Auto")

/** What a shape field makes with none set, past Auto: its default, else its first ratio, else 1:1. */
function shapeFallback(field: ToolField): string {
  const fixedRatio = field.ratios?.find((ratio): ratio is string => typeof ratio === "string" && ratioOf(ratio) !== undefined)
  return typeof field.default === "string" && ratioOf(field.default) ? field.default : fixedRatio ?? "1:1"
}

/** The shape a box makes, by its corner: the one set, else Auto. Open, the one set has an × that takes it back to Auto. Being pulled, the middle says it. */
function ShapeLine({ name, view }: { name: string; view: ElementView }) {
  const store = useCanvasStore()
  const tools = useSignal(store.tools)
  const field = view.asset && fieldsOf(view.asset, tools).find((held) => held.type === "shape")
  if (!field || view.shape) return null
  const held = store.valueOf(name, field.field)
  const fallback = shapeFallback(field)
  if (typeof held !== "string" || !ratioOf(held)) return <span className="ratio-rest">{field.auto ? autoWords(view) : fallback}</span>
  return (
    <span className="ratio-rest">
      {held}
      {view.here && (
        <button type="button" className="ratio-clear nodrag" aria-label={field.auto ? "Auto shape" : `Reset aspect ratio to ${fallback}`} title={field.auto ? "Use automatic aspect ratio" : `Reset to ${fallback}`} onClick={() => field.auto ? store.unshape(name, field.field) : void store.setField(name, field.field, fallback)}>
          <Icon name="close" />
        </button>
      )}
    </span>
  )
}

/** The shape it makes as set, else its picture's, else what its tool makes unset (Auto): what a tile says under its picture (cl-932). */
function useShapeSaid(view: ElementView, name: string): string | null {
  const store = useCanvasStore()
  const tools = useSignal(store.tools)
  if (!view.asset || view.shape) return null
  const field = fieldsOf(view.asset, tools).find((held) => held.type === "shape")
  const held = field ? store.valueOf(name, field.field) : undefined
  if (typeof held === "string" && ratioOf(held)) return held
  const shown = shownOf(view)
  if (shown?.width && shown.height) return shapeWords(shown.width / shown.height)
  if (!field) return null
  return field.auto ? autoWords(view) : shapeFallback(field)
}

/** Under a box at rest, as Paul drew it (cl-932, 11-12): its tool and its shape; then, while it renders, cl-898's two lines with Cancel and
 *  the whole job's meter. Nothing else: what it can do is in Focus. It hangs under it, in the room it keeps clear (`underOf`). */
/** The run a box at rest says how goes, while it's on its way or its result is: `going`, and the run's number once there is one. */
function useGoing(view: ElementView): { going: boolean; number?: number } {
  const arriving = useArriving(view)
  const asset = view.asset
  const onItsWay = asset?.standing === "making" || Boolean(view.queued) || view.asked !== undefined
  return { going: Boolean(asset && !view.placing && (onItsWay || arriving !== undefined) && asset.rendering == null), number: onItsWay ? makingRun(view) : arriving }
}

function Under({ view, name, lite }: { view: ElementView; name: string; lite: boolean }) {
  const store = useCanvasStore()
  const tools = useSignal(store.tools)
  const { going, number } = useGoing(view)
  const shape = useShapeSaid(view, name)
  const asset = view.asset
  if (!asset || view.placing) return null
  const tool = asset.tool ? tools.get(asset.tool) : undefined
  // What makes it (Paul, 11-12: "I want to see what tools are attached"): once, here, as "MiniMax H3 · Shot".
  const named = asset.tool ? (tool ? toolName(tool) : asset.tool) : null
  // A take that went on from another box's says which (cl-951).
  const shown = shownOf(view)
  const made = asset.prefixes?.find((version) => version.number === shown?.number)?.pickup
  const of = made ? store.mentionOf(bare(made.take), name) : null
  const source = of && of.name !== store.resolveName(name) ? of.name : null
  const line = !lite && (named || shape || source)
  if (!line && !going) return null
  return (
    <div className="tile-under">
      {line && (
        <div className="tile-line">
          {named && <span className="node-tool" title={asset.tool!}>{named}</span>}
          {source && <span className="continues-rest" title={`Continues ${made!.take}`}>from {source}</span>}
          <span className="sp" />
          {shape && <span className="ratio-rest">{shape}</span>}
        </div>
      )}
      {going && <RunFace view={view} name={name} number={number} />}
    </div>
  )
}

/** Along the top of a box at rest, how far its whole job is (Paul, 11-12). */
function TileMeter({ view }: { view: ElementView }) {
  const { going, number } = useGoing(view)
  return going ? <RunMeter view={view} number={number} /> : null
}

/** A name with a chance to break after each _, so far out a long one wraps between its words; its text is the same (cl-958). */
const breakable = (name: string) => name.split(/(?<=_)/).flatMap((part, at) => (at ? [<wbr key={at} />, part] : [part]))

const CardBody = memo(function CardBody({ name, width, focused = false, snapshot }: { name: string; width: number; focused?: boolean; snapshot?: ElementView }) {
  const store = useCanvasStore()
  const flow = useReactFlow<ElementNode>()
  const live = useElement(name)
  const view = snapshot ?? live
  const here = Boolean(view?.here)
  const owner = here ? store.inPlace : null
  // Far out, only what can be seen: its picture, its name and its state.
  const lite = useSignal(store.far) && !here
  const tools = useSignal(store.tools)
  const namePress = useRef<{ x: number; y: number } | null>(null)
  const self = useRef<HTMLDivElement>(null)
  const cancel = useCallback(() => {
    if (store.get(name)?.placing) store.cancel(store.resolveName(name))
    else store.patch(name, { draft: undefined })
  }, [store, name])
  // Its first text has taken the keys.
  useEffect(() => {
    if (store.focusing === name) store.focusing = null
  })
  // One geometry owner for the real card, including the lower panel and its takes.
  useLayoutEffect(() => {
    if (focused || !here || owner?.name !== name || !self.current) return
    const card = self.current
    const id = store.idOf(name)
    if (!id) return
    const original = Array.from(card.style, (name) => [name, card.style.getPropertyValue(name), card.style.getPropertyPriority(name)] as const)
    const apply = (style: CSSProperties) => {
      for (const [property, value] of Object.entries(style)) {
        const name = property.replace(/[A-Z]/g, (letter) => `-${letter.toLowerCase()}`)
        if (card.style.getPropertyValue(name) !== String(value)) card.style.setProperty(name, String(value))
      }
    }
    const previous = store.surfaceOf(name)
    let under = card.querySelector<HTMLElement>(".here-all")
    const dimensions = () => `${card.offsetWidth}:${card.offsetHeight}:${under?.offsetWidth}:${under?.scrollHeight}`
    let drawn = ""
    let project: ReturnType<typeof readableSurface> | undefined
    const update = (room?: Box, at?: Viewport) => {
      const name = card.dataset.asset
      if (!name) return null
      if (store.inPlace !== owner) return null
      // React Flow freezes its drag offset from this envelope. Its hidden lower panel must not redefine that offset mid-gesture.
      if (store.isDragging(name)) return store.surfaceOf(name) ?? null
      const current = store.get(name)
      const pose = store.poseOf(name)
      if (!current?.here || !pose || !card.isConnected) return null
      if (!card.offsetWidth || !card.offsetHeight) return null
      if (!room && !store.inPlace?.fitted && !store.inPlace?.moved && !store.inPlace?.pulled) return null
      if (!project) {
        if (previous) apply(previous.style)
        project = readableSurface(card)
      }
      const viewport = at ?? flow.getViewport()
      const projection = project(current, pose, viewport, room)
      if (!at) {
        apply(projection.style)
        store.presentSurface(id, projection)
        drawn = dimensions()
      }
      return room && projection.view === viewport ? null : projection
    }
    const unbind = store.bindSurface(id, update)
    let scheduled = 0
    let alive = true
    let observed = new Set<Element>()
    const watch = new ResizeObserver(() => {
      if (dimensions() !== drawn) schedule()
    })
    const observe = () => {
      if (!alive || !card.isConnected) return
      // These children keep their intrinsic height inside the scrolling panel (flex-shrink: 0).
      // The card/aperture and capped panel are our outputs, not independent measurement inputs.
      const next = new Set(under?.children ?? [])
      for (const part of observed) if (!next.has(part)) watch.unobserve(part)
      for (const part of next) if (!observed.has(part)) watch.observe(part)
      observed = next
    }
    const schedule = () => {
      if (!alive || !card.isConnected || store.inPlace !== owner) return
      if (scheduled) return
      scheduled = requestAnimationFrame(() => {
        scheduled = 0
        if (!alive || !card.isConnected || store.inPlace !== owner) return
        update()
        store.fitHere(true)
        observe()
      })
    }
    // Our own projection changes these boxes too. Only new content dimensions need another pass.
    const check = () => {
      if (dimensions() !== drawn) schedule()
    }
    drawn = dimensions()
    const contents = new MutationObserver(() => {
      const next = card.querySelector<HTMLElement>(".here-all")
      if (next !== under) {
        under = next
        schedule()
      } else {
        check()
        if ([...(under?.children ?? [])].some((part) => !observed.has(part))) schedule()
      }
    })
    contents.observe(card, { childList: true, characterData: true, subtree: true })
    window.addEventListener("resize", schedule)
    document.fonts.addEventListener("loadingdone", schedule)
    // React Flow can mount a culled card while delivering its own size observations.
    // Publish our changed dimensions on the next frame, never from that delivery.
    const first = requestAnimationFrame(() => {
      if (!alive || !card.isConnected || store.inPlace !== owner) return
      // First compute the fit before a full envelope can cull a card mounted for measurement.
      if (store.inPlace?.fitted || store.inPlace?.moved || store.inPlace?.pulled) update()
      store.fitHere(true)
      observe()
    })
    return () => {
      alive = false
      cancelAnimationFrame(first)
      cancelAnimationFrame(scheduled)
      watch.disconnect()
      contents.disconnect()
      window.removeEventListener("resize", schedule)
      document.fonts.removeEventListener("loadingdone", schedule)
      unbind()
      const width = card.style.width
      const priority = card.style.getPropertyPriority("width")
      // Restore through CSSOM: a raw style attribute is refused by the native page's CSP.
      for (const name of Array.from(card.style)) card.style.removeProperty(name)
      for (const [name, value, priority] of original) card.style.setProperty(name, value, priority)
      if (width) card.style.setProperty("width", width, priority)
    }
  }, [here, focused, name, store, flow, owner])
  // What it shows has been looked at: its dot goes (cl-928).
  const showing = view ? shownOf(view)?.number : undefined
  const unseen = view?.asset?.unseen
  useEffect(() => {
    if (showing !== undefined && unseen?.includes(showing)) store.markSeen(name, showing)
  }, [store, name, showing, unseen])
  if (!view) return null

  const asset = view.asset
  const versions = asset?.prefixes ?? []
  const shown = shownOf(view)
  const at = versions.findIndex((version) => version.number === shown?.number)
  const shownVersion = versions.find((version) => version.number === shown?.number)
  const face = faceOf(view)
  const canFlip = asset?.kind !== "image" && makes(view)
  // On the canvas a box is its name, its picture or its text, its state and an X. All else comes with a click, so 300 boxes cost little.
  const editing = here && (asset?.kind === "image" || (asset?.kind === "video" && !asset.timeline) ? Boolean(view.hereWords) : asset?.timeline ? face === "edit" : Boolean(asset?.tool) || Boolean(view.hereWords))
  const looking = here && asset?.kind !== "image" && !view.hereWords && paints(view, tools)
  // An Edit in Focus is a video editor: its cut playing in the frame, its timeline under it, its versions beside it (cl-957).
  const cut = focused && Boolean(asset?.timeline)
  // Any other video in Focus plays on the same transport and track, its segments on it (cl-965).
  const plays = focused && asset?.kind === "video" && !asset.timeline && !editing
  // Its fields show under it, Render first (cl-938).
  const fielded = here && Boolean(asset?.tool) && (asset?.kind !== "image" || editing) && !(asset?.kind === "video" && !asset.timeline && !editing) && !looking
  // An approved box says nothing but what's asked of it: what's wrong with its words is on its controls.
  // Looking at a take, nothing of its tool: its warnings and state are the next take's too.
  const warned = view.warnings?.length && !(asset?.pinned && canFlip && face === "media") && !looking
  // At rest, a render on its way breathes around its edge (cl-928): its face says how far, in it.
  const rendering = !here && !focused && Boolean(asset?.standing === "making" || view.queued || view.asked !== undefined)
  const firstImage = versions.find((version) => version.number === 1)?.main ?? versions[0]?.main
  const originalAspect = firstImage?.width && firstImage.height ? firstImage.width / firstImage.height : aspectOf(view)
  const imageHeight = width / originalAspect
  const widestImage = Math.max(originalAspect, ...versions.map((version) => version.main?.width && version.main.height ? version.main.width / version.main.height : originalAspect)) * imageHeight
  const style: CSSProperties & Record<`--${string}`, string | number> = focused
    ? { width: "100%", "--focus-footer-room": `${!fielded || cut ? 0 : (asset?.kind === "image" || asset?.kind === "video") && !asset.timeline ? 320 : 220}px` }
    : { width: here ? (store.surfaceOf(name)?.style.width ?? bodyWidth(view, width)) : bodyWidth(view, width), "--image-history-offset": `${widestImage / 2}px`, "--image-history-height": `${imageHeight}px` }
  return (
    <div
      ref={self}
      className={`card ${focused ? "focus-card" : ""} ${view.here && !view.hereWords && view.asset?.pinned && !shownOf(view)?.absent && shownOf(view)?.number === view.asset.approved ? "showing-approved" : ""} ${here ? "here" : ""} ${rendering ? "rendering" : ""} ${view.landing ? "landing" : ""} ${view.hot ? "hot" : ""} ${cut ? "edits" : ""} ${plays ? "plays" : ""}`}
      data-asset={view.name}
      data-kind={kindOf(view)}
      style={style}
      onClick={buttonsOnly}
      onAnimationEnd={(event) => {
        if (event.target === event.currentTarget && view.landing) store.patch(name, { landing: undefined })
      }}
    >
      {!focused && (
      <div className="head">
        {/* What it's doing, left of its name (Paul, 11-12): grey queued, blue running, green new and not looked at, red failed. */}
        {view.draft === undefined && <StateDot state={viewState(view)} unseen={unseen?.length ?? 0} noun={asset?.noun ?? "take"} />}
        {view.draft !== undefined ? (
          <NameField name={name} draft={view.draft} onCancel={cancel} />
        ) : (
          <span
            className="nm"
            title={view.name}
            onPointerDown={(event) => { namePress.current = { x: event.clientX, y: event.clientY } }}
            onClick={(event) => {
              if (event.shiftKey || event.metaKey || event.ctrlKey) return
              event.stopPropagation()
              const from = namePress.current
              if (from && Math.hypot(event.clientX - from.x, event.clientY - from.y) < 5) store.openName(name)
            }}
          >
            {view.placing ? store.shownName(store.resolveName(name)) : breakable(view.name)}
          </span>
        )}
        {view.draft === undefined && (asset ? (here ? asset.kind === "image" : asset.kind !== "text") && shown?.number !== undefined : isPicture(view)) && <span className="node-version-label">v{asset ? shown?.number : 1}</span>}
        {view.draft === undefined && !lite && <span className="sp" />}
        {view.draft === undefined && here && asset?.kind !== "image" && versions.length > 1 && at >= 0 && (
          <span className="count">
            {at + 1}/{versions.length}
          </span>
        )}
        {view.draft === undefined && here && warned ? (
          <button type="button" className="iconb warned nodrag" aria-label="Warnings" title={noHash(view.warnings!.join("\n"))} onClick={() => store.patch(name, { warned: !view.warned })}>
            !
          </button>
        ) : null}
        {here ? (
          // Open or not, its trash can deletes it, with Undo (cl-939): Esc or a click off it leaves.
          <button
            type="button"
            className="iconb nodrag"
            aria-label="Delete"
            title="Delete this box"
            onClick={() => {
              store.leaveHere()
              store.remove([name])
            }}
          >
            <Icon name="trash" />
          </button>
        ) : view.placing ? (
          // Its × stays as it was dropped: past the point of no return, the moment Calcifer is being asked, it does nothing.
          <button type="button" className="iconb nodrag" aria-label="Cancel" onClick={() => view.placing?.stage !== "adding" && cancel()}>
            <Icon name="close" />
          </button>
        ) : (
          view.draft === undefined &&
          (asset || view.landed) &&
          !lite && (
            <button type="button" className="iconb nodrag" aria-label="Delete" title="Delete this box" onClick={() => store.remove([name])}>
              <Icon name="trash" />
            </button>
          )
        )}
      </div>
      )}
      {focused && !view.placing && !cut && <FocusActions view={view} name={name} />}
      {cut ? <VideoEditor name={name} /> : <FocusFrame enabled={focused} aspect={aspectOf(view)} fitted={asset?.kind !== "text"} zooms={!plays} under={plays ? <VideoTransport name={name} view={view} /> : undefined}>
        <Media view={view} name={name} cardWidth={here ? HERE_THUMB : bodyWidth(view, width)} restWidth={width}>
          {here && (asset?.kind === "text" && !asset.tool ? <TextAsset name={name} view={view} /> : <InFrame name={name} view={view} />)}
          {here && editing && view.continuing && <LeadIn name={name} view={view} />}
          {here && !focused && !view.hereWords && asset && shownVersion?.main && (
            <div
              className="take-out nodrag nopan"
              onPointerDown={(event) => {
                const tile = self.current?.querySelector<HTMLElement>(`.take[data-take="${shownVersion.number}"]`)
                carryOnMove(event.nativeEvent, asset.kind === "image" ? { name } : { take: taken(store, name, asset, { ...shownVersion, main: shownVersion.main! }) }, tile ?? event.currentTarget)
              }}
            />
          )}
          {asset && !view.placing && !lite && (!focused || view.hereWords) && <Corner name={name} view={view} width={width} focused={focused} />}
        </Media>
      </FocusFrame>}
      {plays && <Segments name={name} view={view} />}
      {rendering && <span className="render-wave" data-state={viewState(view) ?? undefined} aria-hidden="true" />}
      {/* Watched, not written: orbit viewing turns on and off here too (xs-180), as in a take being written. */}
      {focused && asset?.kind === "video" && !asset.timeline && !editing && <OrbitToggle view={view} name={name} />}
      {here && !lite && asset?.tool && !view.placing && <ToolLine name={name} view={view} width={width} />}
      {here && !lite && asset && asset.kind !== "image" && !view.hereWords && !view.placing && !looking && <ShapeLine name={name} view={view} />}
      {!here && !focused && <Under view={view} name={name} lite={lite} />}
      {!here && !focused && <TileMeter view={view} />}
      {view.refused && <div className="refused-line">{view.refused}</div>}
      {!here && view.asking && (
        <div className="rest-asking nodrag nopan nowheel">
          <Asking name={name} fit={view.asking.fit} />
        </div>
      )}
      {here && asset && asset.kind !== "text" && (!asset.timeline || cut) && <Takes view={view} name={name} width={width} side focused={focused} />}
      {here && asset && !cut && (asset.kind !== "image" || editing) && !(asset.kind === "video" && !asset.timeline && !editing) && (
        <div className="here-under nodrag nopan nowheel">
          <div className="here-all">
            {(asset.kind !== "text" || asset.tool) && <>
              {editing && asset.kind === "video" && !asset.timeline && <ContinueFrom name={name} view={view} />}
              {asset.kind !== "image" && !asset.timeline && <OrbitToggle view={view} name={name} />}
              {asset.kind !== "image" && !editing && <Bar view={view} name={name} />}
              {/* Looking at a take an image tool made, only its description: its tool and words are the next take's, behind its +. */}
              {/* With no tool, a picture or a video asks nothing and has nothing to render (Paul, 11-12). */}
              {!looking && (asset.kind !== "image" || editing) && Boolean(asset.tool) && <AssetEdit name={name} view={view} editing={editing} focused={focused} actions={editing && !focused && <RenderButton name={name} view={view} />} />}
              {asset.kind !== "image" && !view.hereWords && <TakeName view={view} />}
              {view.warned && warned ? (
                <div className="warned-lines">
                  {view.warnings!.map((warning) => (
                    <p key={warning}>{noHash(warning)}</p>
                  ))}
                </div>
              ) : null}
              {!looking && <Status view={view} name={name} />}
            </>}
          </div>
        </div>
      )}
    </div>
  )
})

/** Thin, as React Flow renders it: identity and body width come from one geometry publication. */
export function Card({ data }: NodeProps<ElementNode>) {
  const store = useCanvasStore()
  const focused = useSignal(store.focusedName)
  const entering = useSignal(store.focusEntering)
  const view = useElement(data.name)
  const resting = useRef(view)
  if (focused !== data.name) resting.current = view
  if (focused === data.name && !entering) return <div className="focus-placeholder" style={{ width: view ? bodyWidth(view, data.width) : data.width, height: view ? heightFor(view, data.width) : 1 }} />
  // While the canvas draws in on it, the box as it rested: the open box is Focus's alone.
  const atRest = focused === data.name && entering && resting.current ? { ...resting.current, here: undefined, hereWords: undefined } : undefined
  return (
    <>
      <CardBody name={data.name} width={data.width} snapshot={atRest} />
      <AddOns name={data.name} />
    </>
  )
}

export function FocusedCard({ name }: { name: string }) {
  const store = useCanvasStore()
  const view = useElement(name)
  if (!view) return null
  return <CardBody key={store.idOf(name) ?? name} name={name} width={store.poseOf(name)?.w ?? widthFor(view)} focused />
}
