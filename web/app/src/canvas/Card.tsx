import { useQuery } from "@tanstack/react-query"
import type { NodeProps, Viewport } from "@xyflow/react"
import { useReactFlow } from "@xyflow/react"
import { type CSSProperties, type MouseEvent, memo, type PointerEvent, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react"
import { flushSync } from "react-dom"
import { machinesQuery } from "../api/queries"
import { useConnection } from "../app/Connection"
import { useWaitsOn } from "../panels/machines/activity"
import type { MainFile } from "../api/calcifer"
import { Icon } from "../ui/Icon"
import { say } from "../ui/notes"
import { useSignal } from "../ui/signal"
import { AssetEdit, InFrame, RenderButton, TextAsset } from "./AssetEdit"
import type { CanvasStore, CarriedFile, ElementNode, ElementView } from "./CanvasStore"
import { carryOnMove } from "./carry"
import { useCanvasStore, useElement } from "./context"
import { fieldsOf, shapesOf, snapped } from "./controls"
import { useDragScroll } from "./dragScroll"
import { downloadable, Media, Preview, preload, Thumb, usePreview, WalkedFrames } from "./Media"
import { NameField } from "./NameField"
import { ORBIT, orbitOf } from "./Orbit"
import { FPS, framesIn, pickupAt as landsOn, timecode } from "./ranges"
import { aspectOf, bodyBox, bodyWidth, faceOf, heightFor, isPicture, madeHere, makes, NARROWEST, paints, ratioOf, reshapes, shownOf, toolName, valuesOf, watched, widthFor } from "./shown"
import { chipWords, noHash, piecesOf, rowsIn, took } from "./text"
import { TimelineControl } from "./timeline/TimelineControl"
import { AddOns, Stack } from "./AddOns"
import { ImageVersions } from "./ImageVersions"
import { FocusFrame, useFocusResize } from "./FocusFrame"
import { Asking } from "./Asking"
import { readableSurface } from "./readableSurface"
import type { Box } from "./graph"

const DOTS: Record<string, string> = { current: "s-current", out_of_date: "s-current", making: "s-making", missing: "s-current", failed: "s-failed" }
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
  const mine = view.run?.number === run ? view.run : undefined
  const on = useWaitsOn(mine)
  if (on) return <Icon name={on.kind === "download" ? "download" : "package"} />
  const fraction = mine?.progress?.done_fraction
  return <>{fraction == null ? "…" : `${Math.round(fraction * 100)}%`}</>
}

/** An approved box shows nothing wrong unless what it reads changed: a failure of something tried since is on its other face. */
function dotOf(view: ElementView): string {
  // A file on its way to Calcifer says nothing: it's there already, as far as anyone looking can tell (cl-918).
  if (view.queued || view.asked !== undefined) return "s-making"
  const asset = view.asset
  if (asset?.standing === "making") return "s-making"
  if (asset?.failure && !(asset.pinned && faceOf(view) === "media")) return "s-failed"
  if (asset && shownOf(view)?.absent) return "s-missing"
  return DOTS[asset?.standing ?? "current"] ?? "s-missing"
}

/** Takes that landed beside an approved one and haven't been looked at (cl-928): a green dot left of its name. */
function UnseenDot({ count, noun }: { count: number; noun: string }) {
  const said = `${count} new ${noun}${count === 1 ? "" : "s"} you haven't looked at`
  return <span className="unseen-dot" role="img" aria-label={said} title={said} />
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

/** Each take or version, the newest first and what's still being made before that: a click previews it; a double tap approves an image.
 *  Beside an open box they're a column, and a drag up or down scrolls it; an Edit's are a row under it, which drags across. A drag any other way carries a
 *  take out. + first goes to its words: an Edit's, or the next take an image tool writes in the frame. An Edit's clips are what a take holds, so they stay
 *  with it unless it's approved. */
function Takes({ view, name, width, side }: { view: ElementView; name: string; width: number; side?: boolean }) {
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
  const making = asset.making ?? []
  const waiting = !making.length && (view.queued || view.asked !== undefined)
  const writes = paints(view, tools)
  // The newest take, once its tile's up, is made ready at the size it's looked at, before it's clicked.
  const { client } = useConnection()
  const newest = asset.prefixes?.at(-1)
  // Its next take is offered once it has any, made, on its way or failed: only a box with none hides it, its words being what's asked
  // (Paul, 11-12, cl-925 and cl-941). An Edit's + is its words.
  const any = versions.length > 0 || Boolean(asset.sha256) || Boolean(asset.timeline) || making.length > 0 || Boolean(waiting) || Boolean(asset.failure)
  const adds = (writes || (asset.kind === "video" && Boolean(asset.tool))) && any
  if (!adds && !versions.length && !view.joining && !making.length && !waiting) return null
  const words = () => {
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
    store.patch(name, { hereWords: true, continuing: undefined, continuingFps: undefined, watchRun: undefined, watchFrom: undefined })
  }
  const pick = (number: number) => {
    store.pickTake(name, number)
    if (onWords || watch) store.patch(name, { hereWords: undefined, watchFrom: undefined, watchRun: undefined })
  }
  const row = (
    <div
      ref={side ? undefined : list}
      className="takes nodrag nopan nowheel"
      onWheel={(event) => {
        if (!side) event.currentTarget.scrollLeft += event.deltaX || event.deltaY
      }}
    >
      {adds && (
        <button type="button" className={`take add ${view.hereWords ? "on" : ""}`} aria-label={writes ? "Its next take" : asset.timeline ? "Its words" : "New video"} title={writes ? "New image" : "New video"} onClick={words}>
          +{(writes || asset.kind === "video") && <small className="take-new">New · {tools.get(asset.tool ?? "")?.label}</small>}
        </button>
      )}
      {waiting && (
        <span className="take-at">
          <button type="button" className={`take making ${watch && !watch.run ? "on" : ""}`} title="Queued" onClick={() => store.watchTake(name)}>
            …
          </button>
          <button type="button" className="take-off" aria-label="Cancel what's queued" title="Cancel what's queued" onClick={() => store.cancelRender(name)}>
            <Icon name="close" />
          </button>
        </span>
      )}
      {making.map((run) => (
        <span key={run} className="take-at">
          <button type="button" className={`take making ${watch?.run === run ? "on" : ""}`} title={`Run ${run}`} onClick={() => store.watchTake(name, run)}>
            <TakeProgress view={view} run={run} />
          </button>
          <button type="button" className="take-off" aria-label={`Cancel run ${run}`} title={`Cancel run ${run}`} onClick={() => store.cancelRun(name, run)}>
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
            {asset.unseen?.includes(version.number) && <span className="unseen-dot take-unseen" role="img" aria-label="Not looked at yet" />}
          </button>
          <button type="button" className="take-off" aria-label={`Discard ${asset.noun} ${version.number}`} title={`Discard ${asset.noun} ${version.number}`} onClick={() => store.discardTake(name, version.number)}>
            <Icon name="close" />
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

/** The machines still held, by name: a take's latents on any other went with it. */
function useHeld(): Set<string> | null {
  const { client } = useConnection()
  const { data } = useQuery(machinesQuery(client))
  return data ? new Set(data.machines.filter((machine) => machine.state !== "ended" && machine.state !== "ending").map((machine) => machine.name)) : null
}

/**
 * Pick up from here: on one of its takes, or on another box's take let go on it. Scrub to a frame, and a render keeps that take up to where it can
 * really cut, and makes the rest again from its words, which stay as they are. The pickup goes with that one render, and is never written.
 */
function PickUp({ view, name }: { view: ElementView; name: string }) {
  const store = useCanvasStore()
  const tools = useSignal(store.tools)
  const held = useHeld()
  const [beatsFrom, setBeatsFrom] = useState<number | null>(null)
  const tool = tools.get(view.asset?.tool ?? "")
  const other = view.pickingUp
  const own = shownOf(view)
  const file = other ? store.fileOf(other.take, name) : own
  if (!makes(view) || !tool?.pickup || (view.hereWords && !other) || file?.field !== "video" || !file.duration_ms) return null
  const number = other ? other.number : own?.number
  if (number === undefined) return null
  const fps = file.fps ?? FPS
  const last = framesIn(file.duration_ms, fps)
  const frame = Math.min(last, Math.round((view.scrub ?? 0) * fps))
  const cut = landsOn(frame, last, tool.pickup)
  const go = (to: number) => {
    const at = Math.max(0, Math.min(last, to)) / fps
    store.patch(name, { scrub: at })
    if (!other) store.seek(name, at)
  }
  const says = cut === null ? `From ${timecode(tool.pickup.first, fps)} on` : cut !== frame ? `Lands on ${timecode(cut, fps)}` : ""
  const source = other ? store.get(other.from)?.asset : view.asset
  const latents = source?.prefixes?.find((version) => version.number === number)?.latents
  const gone = latents && held && !held.has(latents) ? latents : null
  // On the beats' clock: unset, the cut's own time on this box's take, and 0 on another's.
  const beats = tool.fields.find((field) => field.type === "group" && field.list && field.members?.some((member) => member.total))
  const fromDefault = other ? 0 : Math.round(((cut ?? 0) / fps) * 10) / 10
  const take = `<${other ? other.from : name}.${number}>`
  return (
    <div className="from-here" style={{ "--kept": `${((cut ?? 0) / last) * 100}%` } as CSSProperties} role="group" aria-label="Pick up from here">
      {other && (
        <div className="row pickup-of">
          <span className="muted">From {other.from}'s take {other.number}</span>
          <span className="sp" />
          <button type="button" className="iconb nodrag" aria-label="Not from that take" onClick={() => store.patch(name, { pickingUp: undefined, scrub: undefined })}>
            <Icon name="close" />
          </button>
        </div>
      )}
      {other && (
        <div className="pickup-frame" style={{ aspectRatio: file.width && file.height ? file.width / file.height : 16 / 9 }}>
          <WalkedFrames sha256={file.sha256} cardWidth={360} at={frame / last} />
        </div>
      )}
      <input className="nodrag nopan keeps" type="range" aria-label="Where to pick up" min={0} max={last} step={1} value={frame} onChange={(event) => go(Number(event.target.value))} />
      <div className="row">
        <button type="button" className="iconb nodrag" aria-label="A frame back" title="A frame back" disabled={frame <= 0} onClick={() => go(frame - 1)}>
          ‹
        </button>
        <span className="tc-inline">{timecode(frame, fps)}</span>
        <button type="button" className="iconb nodrag" aria-label="A frame on" title="A frame on" disabled={frame >= last} onClick={() => go(frame + 1)}>
          ›
        </button>
        {says && <span className="muted cut-says">{says}</span>}
        <span className="sp" />
        <button
          type="button"
          className="run nodrag"
          disabled={cut === null}
          title={cut === null ? undefined : `Keep up to ${timecode(cut, fps)} and render the rest again`}
          onClick={() => cut !== null && store.run(name, { pickup: { take, out_frame: cut, ...(beatsFrom === null ? {} : { beats_from: beatsFrom }) } })}
        >
          Pick up from here
        </button>
      </div>
      {beats && (
        <label className="slide beats-from">
          <span className="ctl-name">{beats.label} from</span>
          <input
            className="field nodrag"
            type="number"
            aria-label={`${beats.label} from, in seconds`}
            min={0}
            step={0.1}
            placeholder={String(fromDefault)}
            value={beatsFrom ?? ""}
            onChange={(event) => setBeatsFrom(event.target.value === "" ? null : Number(event.target.value))}
          />
          <span className="num">s</span>
        </label>
      )}
      {gone && <div className="status ood">This take's latents went with {gone}: the join may show</div>}
    </div>
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
  const label = nearest ?? override ?? (dynamic ? "Auto" : defaultShape)
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
        {hasOverride && <button type="button" className="ratio-clear nodrag nopan" aria-label={dynamic ? "Auto aspect ratio" : `Reset aspect ratio to ${defaultShape}`} title={dynamic ? "Use the reference image's aspect ratio" : `Reset to ${defaultShape}`} onPointerDown={(event) => event.stopPropagation()} onClick={(event) => { event.stopPropagation(); resetShape() }}><Icon name="close" /></button>}
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

/** The shape a box makes, by its corner: the one set, else Auto. Open, the one set has an × that takes it back to Auto. Being pulled, the middle says it. */
function ShapeLine({ name, view }: { name: string; view: ElementView }) {
  const store = useCanvasStore()
  const tools = useSignal(store.tools)
  const field = view.asset && fieldsOf(view.asset, tools).find((held) => held.type === "shape")
  if (!field || view.shape) return null
  const held = store.valueOf(name, field.field)
  const fixedRatio = field.ratios?.find((ratio): ratio is string => typeof ratio === "string" && ratioOf(ratio) !== undefined)
  const fallback = typeof field.default === "string" && ratioOf(field.default) ? field.default : fixedRatio ?? "1:1"
  if (typeof held !== "string" || !ratioOf(held)) return <span className="ratio-rest">{field.auto ? "Auto" : fallback}</span>
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
  const tool = asset?.tool ? tools.get(asset.tool) : undefined
  const at = versions.findIndex((version) => version.number === shown?.number)
  const shownVersion = versions.find((version) => version.number === shown?.number)
  const face = faceOf(view)
  const canFlip = asset?.kind !== "image" && makes(view)
  // On the canvas a box is its name, its picture or its text, its state and an X. All else comes with a click, so 300 boxes cost little.
  const editing = here && (asset?.kind === "image" || (asset?.kind === "video" && !asset.timeline) ? Boolean(view.hereWords) : asset?.timeline ? face === "edit" : Boolean(asset?.tool) || Boolean(view.hereWords))
  const parameterTool = focused && asset?.kind === "video" && Boolean(asset.tool) && !asset.timeline && editing
  const looking = !parameterTool && here && asset?.kind !== "image" && !view.hereWords && paints(view, tools)
  const hasFocusMedia = Boolean(shown || asset?.making?.length || view.queued || view.asked !== undefined)
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
    ? { width: "100%", "--focus-footer-room": `${asset?.kind === "image" ? editing ? 320 : 64 : asset?.kind === "video" && !editing ? 64 : asset?.tool ? 220 : 0}px` }
    : { width: here ? (store.surfaceOf(name)?.style.width ?? bodyWidth(view, width)) : bodyWidth(view, width), "--image-history-offset": `${widestImage / 2}px`, "--image-history-height": `${imageHeight}px` }
  return (
    <div
      ref={self}
      className={`card ${focused ? "focus-card" : ""} ${parameterTool ? "focus-parameter-tool focus-new-video" : ""} ${hasFocusMedia ? "has-focus-media" : ""} ${view.here && !view.hereWords && view.asset?.pinned && !shownOf(view)?.absent && shownOf(view)?.number === view.asset.approved ? "showing-approved" : ""} ${here ? "here" : ""} ${rendering ? "rendering" : ""} ${view.landing ? "landing" : ""} ${view.hot ? "hot" : ""} ${asset?.timeline && face === "edit" ? "tl-rests" : ""}`}
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
        {view.draft === undefined && unseen?.length ? <UnseenDot count={unseen.length} noun={asset?.noun ?? "take"} /> : null}
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
            {view.placing ? store.shownName(store.resolveName(name)) : view.name}
          </span>
        )}
        {view.draft === undefined && (asset ? asset.kind === "image" && shown?.number !== undefined : isPicture(view)) && <span className="node-version-label">v{asset ? shown?.number : 1}</span>}
        {/* What makes it, beside its name (Paul, 11-12: "I want to see what tools are attached"). */}
        {view.draft === undefined && !lite && asset?.tool && <span className="node-tool" title={asset.tool}>{tool ? toolName(tool) : asset.tool}</span>}
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
          // Open or not, its × deletes it, with Undo: Esc or a click off it leaves.
          <button
            type="button"
            className="iconb nodrag"
            aria-label="Delete"
            title="Delete"
            onClick={() => {
              store.leaveHere()
              store.remove([name])
            }}
          >
            <Icon name="close" />
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
            <button type="button" className="iconb nodrag" aria-label="Delete" title="Delete" onClick={() => store.remove([name])}>
              <Icon name="close" />
            </button>
          )
        )}
        <span className={`dot ${dotOf(view)}`} />
      </div>
      )}
      <FocusFrame enabled={focused} aspect={aspectOf(view)} fitted={asset?.kind !== "text" && !(asset?.timeline && editing)}>{asset?.timeline && editing ? (
        <TimelineControl name={name} />
      ) : (
        <Media view={view} name={name} cardWidth={here ? HERE_THUMB : bodyWidth(view, width)} restWidth={width}>
          {here && (asset?.kind === "text" && !asset.tool ? <TextAsset name={name} view={view} /> : !parameterTool ? <InFrame name={name} view={view} /> : null)}
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
      )}</FocusFrame>
      {rendering && <span className="render-wave" aria-hidden="true" />}
      {here && !lite && asset && (asset.kind === "image" || (asset.kind === "video" && !asset.timeline && !editing)) && !view.placing && <ImageVersions name={name} view={view} width={width} />}
      {!lite && asset && asset.kind !== "image" && !view.hereWords && !view.placing && !looking && <ShapeLine name={name} view={view} />}
      {view.refused && <div className="refused-line">{view.refused}</div>}
      {!here && view.asking && (
        <div className="rest-asking nodrag nopan nowheel">
          <Asking name={name} fit={view.asking.fit} />
        </div>
      )}
      {here && asset && asset.kind !== "text" && !asset.timeline && <Takes view={view} name={name} width={width} side />}
      {here && asset && (asset.kind !== "image" || editing) && !(asset.kind === "video" && !asset.timeline && !editing) && (
        <div className="here-under nodrag nopan nowheel">
          {parameterTool && <div className="authoring-ratio"><ShapeLine name={name} view={view} /></div>}
          <div className="here-all">
            {(asset.kind !== "text" || asset.tool) && <>
              {asset.timeline && <Takes view={view} name={name} width={width} />}
              {asset.kind !== "image" && !asset.timeline && (!view.hereWords || view.pickingUp) && <PickUp view={view} name={name} />}
              {asset.kind !== "image" && !asset.timeline && <OrbitToggle view={view} name={name} />}
              {asset.kind !== "image" && !editing && <Bar view={view} name={name} />}
              {/* Looking at a take an image tool made, only its description: its tool and words are the next take's, behind its +. */}
              {!looking && (asset.kind !== "image" || editing) && <AssetEdit name={name} view={view} editing={editing} prominent={parameterTool} head={asset.kind !== "image" ? <Stack name={name} /> : undefined} actions={editing && <RenderButton name={name} view={view} />} />}
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
  return (
    <>
      <CardBody name={data.name} width={data.width} snapshot={focused === data.name && entering ? resting.current : undefined} />
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
