import type { NodeProps } from "@xyflow/react"
import { useReactFlow } from "@xyflow/react"
import { type CSSProperties, type MouseEvent, memo, type PointerEvent, useCallback, useEffect, useRef, useState } from "react"
import { useConnection } from "../app/Connection"
import { useWaitsOn } from "../panels/machines/activity"
import type { MainFile, Tool } from "../types"
import { Icon } from "../ui/Icon"
import { say } from "../ui/notes"
import { useSignal } from "../ui/signal"
import { AssetEdit, InFrame, RenderButton, TextAsset } from "./AssetEdit"
import type { CarriedFile, ElementNode, ElementView } from "./CanvasStore"
import { carryOnMove } from "./carry"
import { useCanvasStore, useElement } from "./context"
import { controlsOf, shapesOf, snapped } from "./controls"
import { useDragScroll } from "./dragScroll"
import { Media, Preview, preload, Thumb, usePreview } from "./Media"
import { MentionText } from "./MentionText"
import { NameField } from "./NameField"
import { aspectOf, faceOf, fieldsOf, madeHere, makes, NARROWEST, paints, reshapes, shownOf, watched } from "./shown"
import { chipWords, clock, noHash, piecesOf, rowsIn, took } from "./text"
import { TimelineControl } from "./timeline/TimelineControl"
import { AddOns, Stack } from "./AddOns"

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
  if (asset.field && !asset.tool) return "file"
  return asset.kind === "video" ? "shot" : asset.kind
}

/** How far a take being made is: its package installing or its weights arriving show as such, never as its own percentage. */
function TakeProgress({ view, run }: { view: ElementView; run: number }) {
  const mine = view.run?.number === run ? view.run : undefined
  const on = useWaitsOn(mine)
  if (on) return <Icon name={on.kind === "download" ? "download" : "package"} />
  const fraction = mine?.progress?.done_fraction
  return <>{fraction === undefined ? "…" : `${Math.round(fraction * 100)}%`}</>
}

/** An approved box shows nothing wrong unless what it reads changed: a failure of something tried since is on its other face. */
function dotOf(view: ElementView): string {
  if (view.placing || view.queued || view.asked !== undefined) return "s-making"
  const asset = view.asset
  if (asset?.standing === "making") return "s-making"
  if (asset?.failure && !(asset.pinned && faceOf(view) === "media")) return "s-failed"
  if (asset && shownOf(view)?.absent) return "s-missing"
  return DOTS[asset?.standing ?? "current"] ?? "s-missing"
}

/** A button on a card does its one thing, and a field takes the click: neither selects it. */
export function buttonsOnly(event: MouseEvent) {
  if ((event.target as Element).closest("button, input, select, .mention-text")) event.stopPropagation()
}

type Version = NonNullable<NonNullable<ElementView["asset"]>["prefixes"]>[number]

/** A take carried out: it lands as a box of its own, or as `<name.N>` in words. It's picked up from its tile or the big one, and an Edit's from its row, as its big tile is its clips. */
function taken(store: ReturnType<typeof useCanvasStore>, name: string, asset: NonNullable<ElementView["asset"]>, version: Version & { main: NonNullable<Version["main"]> }): CarriedFile {
  const description = store.valueOf(name, "description")
  return { sha256: version.main.sha256, field: version.main.field, name: `${asset.name}_${version.number}`, description: typeof description === "string" ? description : undefined, from: asset.name, number: version.number }
}

/** An Edit's clips to choose from: a click puts one in, one can be carried to a place, and the row drags across. A prompt reads pictures by name. */
function Pictures({ name }: { name: string }) {
  const store = useCanvasStore()
  useSignal(store.names)
  const row = useDragScroll({
    axis: "x",
    lift(pressed) {
      const from = pressed.closest<HTMLElement>(".picture")
      return from?.dataset.name ? { load: { name: from.dataset.name }, from } : null
    },
  })
  const held = store.namesLike("", name).flatMap((other) => {
    const view = store.get(other)
    const shown = view && shownOf(view)
    return shown && !shown.absent && (shown.field === "image" || shown.field === "video") ? [{ other, sha256: shown.sha256 }] : []
  })
  if (!held.length) return null
  return (
    <div ref={row} className="pictures" aria-label="The project's pictures">
      {held.map(({ other, sha256 }) => (
        <button
          key={other}
          type="button"
          className="picture nodrag"
          title={other}
          aria-label={`Put ${other} in`}
          data-name={other}
          onClick={(event) => {
            const place = event.currentTarget.closest(".card")?.querySelector("[data-drop-target]")
            const target = place && store.dropTargets.get(place)
            if (target) target.drop([other], 0, 0)
            else store.takeInto(name, [other])
          }}
        >
          <Thumb sha256={sha256} cardWidth={120} />
        </button>
      ))}
    </div>
  )
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

/** Each take or version, the newest first and what's still being made before that: a click looks at it, a double tap approves it or, approved, unapproves it.
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
  const adds = writes || (asset.timeline && asset.tool && shownOf(view) && faceOf(view) !== "edit")
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
    store.focusing = name
    store.patch(name, { hereWords: true })
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
        <button type="button" className={`take add ${view.hereWords ? "on" : ""}`} aria-label={writes ? "Its next take" : "Its words"} title={writes ? "Write its next take" : "Its words"} onClick={words}>
          +{writes && <small className="take-new">New · {tools.get(asset.tool ?? "")?.label}</small>}
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
        <span key={version.number} className="take-at">
          <button
            type="button"
            className={`take ${shown?.number === version.number ? "on" : ""} ${asset.pinned && version.number === (asset.approved ?? asset.version) ? "approved" : ""} ${view.slid === version.number ? "fresh" : ""}`}
            aria-label={`${asset.noun} ${version.number}`}
            title={[`${asset.noun} ${version.number}`, version.execution_ms ? took(version.execution_ms) : "", version.number === asset.approved ? "approved" : ""].filter(Boolean).join(", ")}
            data-take={version.number}
            onClick={() => pick(version.number)}
            onDoubleClick={() => store.toggleApproved(name, version.number)}
          >
            {version.main && !version.main.absent ? (
              <TakeThumb main={version.main} width={width} onShown={version === newest && version.main.field === "image" ? () => preload(client, store.projectId, version.main!.sha256) : undefined} />
            ) : null}
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
    <div ref={list} className="here-side nodrag nopan nowheel">
      {row}
    </div>
  ) : (
    row
  )
}

/** The one thing to do with what's being looked at: approve it, or unapprove what's approved. */

/** A take's frames a second. */
const FPS = 24

const frameCode = (frame: number) => `${clock((frame / FPS) * 1000)}.${String(frame % FPS).padStart(2, "0")}`

/** The frame a new take really starts from, asked for `frame` of a take of `last` frames, as its tool declares. Null: not this early. */
function cutAt(frame: number, last: number, cuts: Tool["cuts"]): number | null {
  if (!cuts) return Math.min(frame, last)
  if (frame < cuts.first) return null
  return Math.min(last, cuts.offset + Math.round((frame - cuts.offset) / cuts.every) * cuts.every)
}

/** On a take of what can go on from one: scrub to a point, a frame at a time, and a new take holds all before where it can really cut and carries on from there. Its words stay as they are. */
function FromHere({ view, name }: { view: ElementView; name: string }) {
  const store = useCanvasStore()
  const tools = useSignal(store.tools)
  const shown = shownOf(view)
  const tool = tools.get(view.asset?.tool ?? "")
  if (!makes(view) || !tool?.takes || shown?.field !== "video" || shown.number === undefined || !shown.duration_ms) return null
  const last = Math.round((shown.duration_ms / 1000) * FPS)
  const frame = Math.min(last, Math.round((view.scrub ?? 0) * FPS))
  const cut = cutAt(frame, last, tool.cuts)
  const go = (to: number) => {
    const held = Math.max(0, Math.min(last, to)) / FPS
    store.patch(name, { scrub: held })
    store.seek(name, held)
  }
  const says = cut === null ? `From ${frameCode(tool.cuts?.first ?? 0)} on` : tool.cuts && cut !== frame ? `Cuts at ${frameCode(cut)}` : ""
  return (
    <div className="from-here" style={{ "--kept": `${((cut ?? 0) / last) * 100}%` } as CSSProperties}>
      <input className="nodrag nopan keeps" type="range" aria-label="Where to keep it up to" min={0} max={last} step={1} value={frame} onChange={(event) => go(Number(event.target.value))} />
      <div className="row">
        <button type="button" className="iconb nodrag" aria-label="A frame back" title="A frame back" disabled={frame <= 0} onClick={() => go(frame - 1)}>
          ‹
        </button>
        <span className="tc-inline">{frameCode(frame)}</span>
        <button type="button" className="iconb nodrag" aria-label="A frame on" title="A frame on" disabled={frame >= last} onClick={() => go(frame + 1)}>
          ›
        </button>
        {says && <span className="muted cut-says">{says}</span>}
        <span className="sp" />
        <button type="button" className="run nodrag" disabled={!cut} title={cut ? `Keep up to ${frameCode(cut)} and make the rest again` : undefined} onClick={() => cut && store.run(name, { take: shown.number, in_s: cut / FPS })}>
          New take from here
        </button>
      </div>
    </div>
  )
}

/** The rows of a group that adds up, each as wide as it lasts, or a cut's pieces. Read only: a row goes to the box's words, a piece plays from there. */
function Bar({ view, name }: { view: ElementView; name: string }) {
  const store = useCanvasStore()
  const tools = useSignal(store.tools)
  const group = view.asset && controlsOf(view.asset, tools).find((control) => control.members?.some((member) => member.total))
  const total = group?.members?.find((member) => member.total)
  const rows = group ? rowsIn(fieldsOf(view)[group.field]) : []
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
  const pieces = piecesOf(fieldsOf(view))
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

function Pull() {
  const store = useCanvasStore()
  const moving = useSignal(store.transferring)
  return <div className="status">{moving ? "Pulling" : <Act label="Pull" onClick={() => void store.pull()} />}</div>
}

/** What it says under its media: how far an import or a run is, and what's wrong and what to do about it. Never a hash. */
function Status({ view, name }: { view: ElementView; name: string }) {
  const store = useCanvasStore()
  const will = store.nameWillBe(name)
  if (will) return <div className="status">Will be {will}</div>
  const asset = view.asset
  if (!asset) return null
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
        {stalled.usd_micros_per_hour !== undefined && ` · ${usdPerHour(stalled.usd_micros_per_hour)}`} <Act label="Cancel" onClick={() => store.cancelRender(asset.name)} />
      </div>
    )
  }
  if (view.queued || view.asked !== undefined) return null
  // What's at rest says nothing here: a failure is its toast, and a file that isn't here can be pulled.
  const shown = shownOf(view)
  if (shown?.absent && !shown.machine) return <Pull />
  return null
}

/** What this asset is, in words of its own: it reaches what mentions it. Always open, under its label. */
function Description({ name, view, rule }: { name: string; view: ElementView; rule: boolean }) {
  const store = useCanvasStore()
  const held = store.valueOf(name, "description")
  if (!view.asset) return null
  return (
    <div className="desc-field">
      {rule && <hr className="here-rule" />}
      <span className="block-label">Description</span>
      <MentionText
        asset={name}
        place="description"
        value={typeof held === "string" ? held : ""}
        label="Description"
        missing={[]}
        focus={false}
        onDraft={(words) => store.setDraft(name, "description", words)}
        onSave={() => void store.save(name)}
      />
    </div>
  )
}

/** The corner, pulled to size a box. Showing no picture, or its words, a box takes its tool's shape from the pull, both ways: the nearest it can make shows in the middle, and it settles there when let go. Showing a picture, it only scales. */
function Corner({ name, view, width }: { name: string; view: ElementView; width: number }) {
  const store = useCanvasStore()
  const flow = useReactFlow()
  const tools = useSignal(store.tools)
  const control = view.asset && reshapes(view) ? controlsOf(view.asset, tools).find((held) => held.type === "shape") : undefined
  const from = useRef<{ x: number; y: number; w: number; h: number } | null>(null)
  const pulled = useRef<{ w: number; h: number } | null>(null)
  const [nearest, setNearest] = useState<string | null>(null)
  const moved = (event: PointerEvent) => {
    if (!from.current) return
    const zoom = flow.getZoom()
    const dx = (event.clientX - from.current.x) / zoom
    const dy = (event.clientY - from.current.y) / zoom
    if (!control) {
      const w = Math.min(WIDEST, Math.max(NARROWEST, from.current.w * Math.max(1 + dx / from.current.w, 1 + dy / from.current.h)))
      pulled.current = { w, h: w / aspectOf(view) }
      return store.setWidth(name, w, false)
    }
    const w = Math.min(WIDEST, Math.max(NARROWEST, from.current.w + dx))
    const h = Math.min(WIDEST, Math.max(LOWEST, from.current.h + dy))
    pulled.current = { w, h }
    store.drawShape(name, w, h)
    setNearest(snapped(w / h, shapesOf(control)))
  }
  const done = () => {
    if (!from.current) return
    from.current = null
    const size = pulled.current
    pulled.current = null
    setNearest(null)
    if (size && control) store.setShape(name, control, size.w, snapped(size.w / size.h, shapesOf(control)))
    else if (size) store.setWidth(name, size.w)
    if (!store.inPlace) return
    store.inPlace.pulled = false
    requestAnimationFrame(() => store.fitHere(true))
  }
  return (
    <>
      {nearest && (
        <div className="ratio-live" style={{ fontSize: 30 / flow.getZoom() }}>
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
          event.currentTarget.setPointerCapture(event.pointerId)
          from.current = { x: event.clientX, y: event.clientY, w: width, h: width / aspectOf(view) }
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
  const control = view.asset && controlsOf(view.asset, tools).find((held) => held.type === "shape")
  if (!control || view.shape) return null
  const held = store.valueOf(name, control.field)
  if (typeof held !== "string" || !held) return <span className="ratio-rest">Auto</span>
  return (
    <span className="ratio-rest">
      {held}
      {view.here && (
        <button type="button" className="ratio-clear nodrag" aria-label="Auto shape" title="Back to its auto shape" onClick={() => store.unshape(name, control.field)}>
          <Icon name="close" />
        </button>
      )}
    </span>
  )
}

const CardBody = memo(function CardBody({ name, width }: { name: string; width: number }) {
  const store = useCanvasStore()
  const view = useElement(name)
  const here = Boolean(view?.here)
  // Far out, only what can be seen: its picture, its name and its state.
  const lite = useSignal(store.far) && !here
  const tools = useSignal(store.tools)
  const naming = useCallback(() => store.openName(name), [store, name])
  const pressedAt = useRef<{ x: number; y: number } | null>(null)
  const self = useRef<HTMLDivElement>(null)
  const cancel = useCallback(() => {
    if (store.get(name)?.placing) store.cancel(store.resolveName(name))
    else store.patch(name, { draft: undefined })
  }, [store, name])
  // Its first text has taken the keys.
  useEffect(() => {
    if (store.focusing === name) store.focusing = null
  })
  // However it or what's under it grows, or the window, all of it stays in reach.
  useEffect(() => {
    if (!here || !self.current) return
    const check = () => store.fitHere(true)
    const watch = new ResizeObserver(check)
    watch.observe(self.current)
    const under = self.current.querySelector(".here-all")
    if (under) watch.observe(under)
    window.addEventListener("resize", check)
    return () => {
      watch.disconnect()
      window.removeEventListener("resize", check)
    }
  }, [here, store])
  if (!view) return null

  const asset = view.asset
  const versions = asset?.prefixes ?? []
  const shown = shownOf(view)
  const at = versions.findIndex((version) => version.number === shown?.number)
  const shownVersion = versions.find((version) => version.number === shown?.number)
  const face = faceOf(view)
  const canFlip = makes(view)
  // On the canvas a box is its name, its picture or its text, its state and an X. All else comes with a click, so 300 boxes cost little.
  const editing = here && (asset?.timeline ? face === "edit" : Boolean(asset?.tool) || Boolean(view.hereWords))
  const looking = here && !view.hereWords && paints(view, tools)
  // An approved box says nothing but what's asked of it: what's wrong with its words is on its controls.
  // Looking at a take, nothing of its tool: its warnings and state are the next take's too.
  const warned = view.warnings?.length && !(asset?.pinned && canFlip && face === "media") && !looking
  return (
    <div
      ref={self}
      className={`card ${view.here && !view.hereWords && view.asset?.pinned && shownOf(view)?.number === (view.asset.approved ?? view.asset.version) ? "showing-approved" : ""} ${here ? "here" : ""} ${view.landing ? "landing" : ""} ${view.hot ? "hot" : ""} ${asset?.timeline && face === "edit" ? "tl-rests" : ""}`}
      data-asset={view.name}
      data-kind={kindOf(view)}
      onClick={buttonsOnly}
      onAnimationEnd={(event) => {
        if (event.target === event.currentTarget && view.landing) store.patch(name, { landing: undefined })
      }}
    >
      <div className="head">
        {view.draft !== undefined ? (
          <NameField name={name} draft={view.draft} onCancel={cancel} />
        ) : (
          <span
            className="nm"
            title={view.name}
            onPointerDown={(event) => {
              pressedAt.current = { x: event.clientX, y: event.clientY }
            }}
            onClick={(event) => {
              // A tap renames. A drag by its name only moved it, and with Shift, Ctrl or Cmd it selects, as anywhere on the box.
              if (event.shiftKey || event.metaKey || event.ctrlKey) return
              event.stopPropagation()
              const from = pressedAt.current
              if (from && Math.hypot(event.clientX - from.x, event.clientY - from.y) < 5) naming()
            }}
          >
            {view.placing ? store.shownName(store.resolveName(name)) : view.name}
          </span>
        )}
        {view.draft === undefined && !lite && <span className="sp" />}
        {view.draft === undefined && here && versions.length > 1 && at >= 0 && (
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
        ) : view.placing && view.placing.stage !== "adding" ? (
          <button type="button" className="iconb nodrag" aria-label="Cancel" onClick={cancel}>
            <Icon name="close" />
          </button>
        ) : (
          view.draft === undefined &&
          asset &&
          !lite && (
            <button type="button" className="iconb nodrag" aria-label="Delete" title="Delete" onClick={() => store.remove([name])}>
              <Icon name="close" />
            </button>
          )
        )}
        <span className={`dot ${dotOf(view)}`} />
      </div>
      {asset?.timeline && editing ? (
        <TimelineControl name={name} />
      ) : (
        <Media view={view} name={name} cardWidth={here ? HERE_THUMB : width} restWidth={width}>
          {here && (asset?.kind === "text" ? <TextAsset name={name} view={view} /> : <InFrame name={name} view={view} />)}
          {here && !view.hereWords && asset && shownVersion?.main && (
            <div
              className="take-out nodrag nopan"
              onPointerDown={(event) => {
                const tile = self.current?.querySelector<HTMLElement>(`.take[data-take="${shownVersion.number}"]`)
                carryOnMove(event.nativeEvent, { take: taken(store, name, asset, { ...shownVersion, main: shownVersion.main! }) }, tile ?? event.currentTarget)
              }}
            />
          )}
          {asset && !view.placing && !lite && <Corner name={name} view={view} width={width} />}
        </Media>
      )}
      {!lite && asset && !view.placing && !looking && <ShapeLine name={name} view={view} />}
      {view.refused && <div className="refused-line">{view.refused}</div>}
      {here && asset && asset.kind !== "text" && !asset.timeline && <Takes view={view} name={name} width={width} side />}
      {here && asset && asset.kind !== "text" && (
        <div className="here-under nodrag nopan nowheel">
          <div className="here-all">
            {asset.timeline && <Takes view={view} name={name} width={width} />}
            {asset.timeline && editing && <Pictures name={name} />}
            {!asset.timeline && !view.hereWords && <FromHere view={view} name={name} />}
            {!editing && <Bar view={view} name={name} />}
            {/* Looking at a take an image tool made, only its description: its tool and words are the next take's, behind its +. */}
            {!looking && (
              <AssetEdit name={name} view={view} editing={editing} head={<Stack name={name} />} actions={editing && <RenderButton name={name} view={view} />} />
            )}
            {!view.hereWords && <TakeName view={view} />}
            {/* Writing an image tool's next take, its description isn't asked: it's for what's looked at. */}
            {!(view.hereWords && paints(view, tools)) && <Description name={name} view={view} rule={Boolean(asset.tool) && !looking} />}
            {view.warned && warned ? (
              <div className="warned-lines">
                {view.warnings!.map((warning) => (
                  <p key={warning}>{noHash(warning)}</p>
                ))}
              </div>
            ) : null}
            {!looking && <Status view={view} name={name} />}
          </div>
        </div>
      )}
    </div>
  )
})

/** Thin, as React Flow renders it: `data` is only the name. Element types are `nodeTypes`, looks are `CardBody`. */
export function Card({ data, width }: NodeProps<ElementNode>) {
  return (
    <>
      <CardBody name={data.name} width={width ?? 240} />
      <AddOns name={data.name} />
    </>
  )
}
