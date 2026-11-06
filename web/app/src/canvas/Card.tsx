import { useQuery } from "@tanstack/react-query"
import type { NodeProps } from "@xyflow/react"
import { useReactFlow } from "@xyflow/react"
import { type CSSProperties, type DragEvent, type MouseEvent, memo, type PointerEvent, useCallback, useEffect, useRef, useState } from "react"
import { runsQuery } from "../api/queries"
import { useConnection } from "../app/Connection"
import type { MainFile, Run, Tool } from "../types"
import { Icon } from "../ui/Icon"
import { useSignal } from "../ui/signal"
import { AssetEdit, InFrame, RenderButton, TextAsset } from "./AssetEdit"
import type { ElementNode, ElementView } from "./CanvasStore"
import { useCanvasStore, useElement } from "./context"
import { controlsOf, shapesOf, snapped } from "./controls"
import { DRAGGED_FILE, DRAGGED_NAME } from "./drag"
import { Media, Thumb, useWalk, WalkedFrames } from "./Media"
import { MentionText } from "./MentionText"
import { NameField } from "./NameField"
import { aspectOf, faceOf, madeHere, makes, shownOf } from "./shown"
import { clock, noHash, piecesOf, rowsIn, took } from "./text"
import { TimelineControl } from "./timeline/TimelineControl"
import { AddOns, Stack } from "./AddOns"

const DOTS: Record<string, string> = { current: "s-current", out_of_date: "s-ood", making: "s-making", missing: "s-missing", failed: "s-failed" }
const WIDEST = 1600
const NARROWEST = 160
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

const settingUp = (run: Run) => run.kind === "update" || run.target.startsWith("install ")

/** A box waiting its turn, or for what its machine must have first: its update, and the package's install. */
function Waiting({ view }: { view: ElementView }) {
  const { client } = useConnection()
  const going = useQuery(runsQuery(client, { status: "in_progress" }, 100))
  const queued = useQuery(runsQuery(client, { status: "queued" }, 100))
  const said = view.run?.waiting
  if (said) return <div className="status mak">{said[0].toUpperCase() + said.slice(1)}</div>
  const machine = [...(going.data?.runs ?? []), ...(queued.data?.runs ?? [])].find(settingUp)?.machine
  if (machine) return <div className="status mak">Getting {machine} ready</div>
  return view.queued || view.asked !== undefined ? <div className="status mak">Queued</div> : null
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

/** A take carried out: it lands as a box of its own, or as `<name.N>` in words. It's picked up from the big tile, and an Edit's from its row, as its big tile is its clips. */
function takeOut(event: DragEvent, store: ReturnType<typeof useCanvasStore>, name: string, asset: NonNullable<ElementView["asset"]>, version: NonNullable<NonNullable<ElementView["asset"]>["prefixes"]>[number]) {
  const description = store.valueOf(name, "description")
  const file = { sha256: version.main?.sha256, field: version.main?.field, name: `${asset.name}_${version.number}`, description: typeof description === "string" ? description : undefined, from: asset.name, number: version.number }
  event.dataTransfer.setData(DRAGGED_FILE, JSON.stringify(file))
  event.dataTransfer.effectAllowed = "copy"
}

/** An Edit's clips to choose from: a click puts one in, and one can be dragged to a place. A prompt reads pictures by name. */
function Pictures({ name }: { name: string }) {
  const store = useCanvasStore()
  useSignal(store.names)
  const held = store.namesLike("", name).flatMap((other) => {
    const view = store.get(other)
    const shown = view && shownOf(view)
    return shown && !shown.absent && (shown.field === "image" || shown.field === "video") ? [{ other, sha256: shown.sha256 }] : []
  })
  if (!held.length) return null
  return (
    <div className="pictures" aria-label="The project's pictures">
      {held.map(({ other, sha256 }) => (
        <button
          key={other}
          type="button"
          className="picture nodrag"
          title={other}
          aria-label={`Put ${other} in`}
          draggable
          onDragStart={(event) => event.dataTransfer.setData(DRAGGED_NAME, other)}
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

/** A take on the row: a video's frames walk by under the pointer. */
function TakeThumb({ main, width }: { main: MainFile; width: number }) {
  const { at, walk, stop } = useWalk()
  return (
    <span className="take-thumb" onPointerMove={main.field === "video" ? walk : undefined} onPointerLeave={stop}>
      <Thumb sha256={main.sha256} cardWidth={width} />
      {at !== null && <WalkedFrames sha256={main.sha256} cardWidth={width} at={at} />}
    </span>
  )
}

/** Each take or version, side by side, the newest first and what's still being made before that: a press looks at it, and held down the pointer slides between them. + at the far left goes back to its words. An Edit's clips are what a take holds, so they stay with it unless it's approved. */
function Takes({ view, name, width }: { view: ElementView; name: string; width: number }) {
  const store = useCanvasStore()
  const asset = view.asset!
  const onWords = view.hereWords && !(asset.timeline && !asset.pinned)
  const shown = onWords ? null : shownOf(view)
  const versions = [...(asset.prefixes ?? [])].reverse()
  const making = asset.making ?? []
  const waiting = !making.length && (view.queued || view.asked !== undefined)
  const adds = asset.tool && shownOf(view) && !(asset.timeline && faceOf(view) === "edit")
  if (!adds && !versions.length && !view.joining && !making.length && !waiting) return null
  const words = () => {
    store.focusing = name
    store.patch(name, { hereWords: true })
  }
  const pick = (number: number) => {
    store.view(name, number === asset.version ? undefined : number)
    if (onWords) store.patch(name, { hereWords: undefined })
  }
  return (
    <div
      className="takes nodrag nopan nowheel"
      onPointerMove={(event) => {
        // Held down, the pointer slides from take to take.
        if (!event.buttons) return
        const over = document.elementFromPoint(event.clientX, event.clientY)?.closest<HTMLElement>("[data-take]")
        const number = Number(over?.dataset.take)
        if (number && number !== shown?.number) pick(number)
      }}
      onWheel={(event) => {
        event.currentTarget.scrollLeft += event.deltaX || event.deltaY
      }}
    >
      {adds && (
        <button type="button" className={`take add ${view.hereWords ? "on" : ""}`} aria-label="Its words" title="Its words" onClick={words}>
          +
        </button>
      )}
      {waiting && (
        <span className="take making" title="Queued">
          …
        </span>
      )}
      {making.map((run) => (
        <span key={run} className="take making" title={`Run ${run}`}>
          {view.run?.number === run && view.run.progress?.done_fraction !== undefined ? `${Math.round(view.run.progress.done_fraction * 100)}%` : "…"}
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
            draggable={Boolean(asset.timeline && version.main)}
            onDragStart={(event) => version.main && takeOut(event, store, name, asset, version)}
            onPointerDown={() => pick(version.number)}
            onClick={() => pick(version.number)}
          >
            {version.main && !version.main.absent ? <TakeThumb main={version.main} width={width} /> : null}
            <span className="take-no">{version.number}</span>
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
}

/** The one thing to do with what's being looked at: approve it, or unapprove what's approved. */
function Approve({ view, name }: { view: ElementView; name: string }) {
  const store = useCanvasStore()
  const asset = view.asset
  if (!asset || (!asset.tool && !asset.prefixes?.length)) return null
  const shown = shownOf(view)
  const approved = Boolean(asset.pinned) && (asset.approved === undefined ? view.viewed === undefined : shown?.number === asset.approved)
  if (approved) {
    return (
      <button type="button" className="act nodrag" onClick={() => store.unpin(name)}>
        Unapprove
      </button>
    )
  }
  if (shown?.number === undefined) return null
  return (
    <button type="button" className="act nodrag" onClick={() => store.pin(name, shown.number!)}>
      Approve
    </button>
  )
}

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
  const rows = group ? rowsIn(view.asset?.fields?.[group.field]) : []
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
  const pieces = piecesOf(view.asset?.fields)
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
  const askable = Boolean(asset.tool)
  if (view.waiting) {
    return (
      <div className="status mak">
        Waits for a machine <Act label="Machines" onClick={store.askMachine} />
      </div>
    )
  }
  if (asset.standing === "making") {
    const run = view.run
    const stalled = run?.stalled
    if (stalled) {
      const since = took(Math.max(0, Date.now() - stalled.since_ms))
      return (
        <div className="status bad">
          Stalled {since} on {stalled.machine}
          {stalled.usd_micros_per_hour !== undefined && ` · ${usdPerHour(stalled.usd_micros_per_hour)}`} <Act label="Cancel" onClick={() => store.cancelRender(asset.name)} />
        </div>
      )
    }
    if (run?.queue_position !== undefined) return <div className="status mak">Queued, number {run.queue_position} in line</div>
    // How far it's got is on its picture, and an Edit's film on its own bar.
    return <Waiting view={view} />
  }
  if (view.queued || view.asked !== undefined) return <Waiting view={view} />
  const approvedFace = asset.pinned && faceOf(view) === "media"
  if (asset.failure && !approvedFace) {
    const reason = noHash(asset.failure.message).replace(`${asset.name}: `, "")
    return (
      <div className="status bad">
        {reason} {askable && <Act label="Try again" onClick={() => store.run(name)} />}
        {asset.failure.remedy && <div className="next-line">{noHash(asset.failure.remedy)}</div>}
        {asset.failure.next?.map((line) => (
          <div className="next-line" key={line}>
            {noHash(line)}
          </div>
        ))}
      </div>
    )
  }
  const shown = shownOf(view)
  if (shown?.absent && !shown.machine) return <Pull />
  if (view.viewed !== undefined) {
    const version = asset.prefixes?.find((held) => held.number === view.viewed)
    return (
      <div className="status">
        {asset.noun} {view.viewed}
        {version?.execution_ms ? `, ${took(version.execution_ms)}` : ""} <Act label="Back" onClick={() => store.view(name)} />
      </div>
    )
  }
  if (asset.standing === "out_of_date") {
    const changed = asset.changed?.length ? `Out of date: ${asset.changed.join(", ")} changed` : noHash(asset.status)
    return (
      <div className="status ood">
        {changed} {askable && <Act label={`New ${asset.noun}`} onClick={() => store.run(name)} />}
      </div>
    )
  }
  if (asset.standing === "missing" && !makes(view)) {
    return (
      <div className="status">
        {noHash(asset.status)} {askable && <Act label={`New ${asset.noun}`} onClick={() => store.run(name)} />}
      </div>
    )
  }
  return null
}

/** What this asset is, in words of its own, whichever way it was made. It reaches what mentions it. It opens on a click, so a small picture isn't crowded; a caption opens it too, and writes itself like anything typed. */
function Description({ name, view }: { name: string; view: ElementView }) {
  const store = useCanvasStore()
  const [open, setOpen] = useState(false)
  const held = store.valueOf(name, "description")
  const text = typeof held === "string" ? held : ""
  const typed = view.drafts !== undefined && "description" in view.drafts
  if (!view.asset) return null
  if (!open && !typed) {
    const shown = shownOf(view)
    return (
      <div className="desc-row">
        <button type="button" className={`desc-line nodrag ${text ? "" : "empty"}`} title={text || "Say what this is"} onClick={() => setOpen(true)}>
          {text || "Describe"}
        </button>
        {shown && !shown.absent && shown.field !== "audio" && (
          <button type="button" className={`act nodrag ${view.captioning ? "going" : ""}`} disabled={view.captioning} title="Ask a model what this is" onClick={() => store.caption(name)}>
            {view.captioning ? "Captioning" : "Caption"}
          </button>
        )}
      </div>
    )
  }
  return (
    <div className="desc-open">
      <MentionText
        asset={name}
        place="description"
        value={text}
        label="What this is"
        missing={[]}
        focus={open}
        onDraft={(words) => store.setDraft(name, "description", words)}
        onSave={() => {
          if (open) void store.save(name)
          setOpen(false)
        }}
      />
    </div>
  )
}

/** The corner, pulled to size a box. A box whose tool has a shape takes it from the pull, both ways: the nearest it can make shows in the middle, and it settles there when let go. */
function Corner({ name, view, width }: { name: string; view: ElementView; width: number }) {
  const store = useCanvasStore()
  const flow = useReactFlow()
  const tools = useSignal(store.tools)
  const control = view.asset && faceOf(view) === "edit" && (!view.here || view.hereWords) ? controlsOf(view.asset, tools).find((held) => held.type === "shape") : undefined
  const from = useRef<{ x: number; y: number; w: number; h: number } | null>(null)
  const pulled = useRef<{ w: number; h: number } | null>(null)
  const [nearest, setNearest] = useState<string | null>(null)
  const moved = (event: PointerEvent) => {
    if (!from.current) return
    const zoom = flow.getZoom()
    const w = Math.min(WIDEST, Math.max(NARROWEST, from.current.w + (event.clientX - from.current.x) / zoom))
    const h = Math.min(WIDEST, Math.max(LOWEST, from.current.h + (event.clientY - from.current.y) / zoom))
    pulled.current = { w, h }
    if (control) {
      store.drawShape(name, w, h)
      setNearest(snapped(w / h, shapesOf(control)))
    } else store.setWidth(name, w, false)
  }
  const done = () => {
    if (!from.current) return
    from.current = null
    const size = pulled.current
    pulled.current = null
    setNearest(null)
    if (store.inPlace) store.inPlace.pulled = false
    if (!size) return
    if (control) store.setShape(name, control.field, size.w, snapped(size.w / size.h, shapesOf(control)))
    else store.setWidth(name, size.w)
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
      />
    </>
  )
}

/** The shape a box makes, under its corner, in the words of its name. Being pulled, the middle says it. */
function ShapeLine({ name, view }: { name: string; view: ElementView }) {
  const store = useCanvasStore()
  const tools = useSignal(store.tools)
  const control = view.asset && controlsOf(view.asset, tools).find((held) => held.type === "shape")
  if (!control || view.shape) return null
  const held = store.valueOf(name, control.field) ?? control.default
  return <span className="ratio-rest">{typeof held === "string" ? held : "Auto"}</span>
}

const CardBody = memo(function CardBody({ name, width }: { name: string; width: number }) {
  const store = useCanvasStore()
  const view = useElement(name)
  const here = Boolean(view?.here)
  // Far out, only what can be seen: its picture, its name and its state.
  const lite = useSignal(store.far) && !here
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
  const editing = here && (asset?.timeline ? face === "edit" : Boolean(view.hereWords))
  // An approved box says nothing but what's asked of it: what's wrong with its words is on its controls.
  const warned = view.warnings?.length && !(asset?.pinned && canFlip && face === "media")
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
        {view.draft === undefined && here && versions.length > 1 && (
          <span className="count">
            {at + 1}/{versions.length}
          </span>
        )}
        {view.draft === undefined && here && warned ? (
          <button type="button" className="iconb warned nodrag" aria-label="Warnings" title={noHash(view.warnings!.join("\n"))} onClick={() => store.patch(name, { warned: !view.warned })}>
            !
          </button>
        ) : null}
        {here && (
          <button
            type="button"
            className="iconb nodrag"
            aria-label="More for this box"
            title="More"
            onClick={(event) => {
              const at = event.currentTarget.getBoundingClientRect()
              store.menu.set({ name, x: at.left, y: at.bottom, full: true })
            }}
          >
            ⋯
          </button>
        )}
        {here ? (
          <button type="button" className="iconb nodrag" aria-label="Close" title="Close" onClick={store.leaveHere}>
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
            <button type="button" className="iconb nodrag on-hover" aria-label="Delete" title="Delete" onClick={() => store.remove([name])}>
              <Icon name="close" />
            </button>
          )
        )}
        <span className={`dot ${dotOf(view)}`} />
      </div>
      {asset?.timeline && editing ? (
        <TimelineControl name={name} />
      ) : (
        <Media view={view} name={name} cardWidth={here ? HERE_THUMB : width}>
          {here && (asset?.kind === "text" ? <TextAsset name={name} view={view} /> : <InFrame name={name} view={view} />)}
          {here && !view.hereWords && asset && shownVersion?.main && <div className="take-out nodrag nopan" draggable onDragStart={(event) => takeOut(event, store, name, asset, shownVersion)} />}
          {asset && !view.placing && !lite && <Corner name={name} view={view} width={width} />}
        </Media>
      )}
      {!here && !lite && asset && !view.placing && <ShapeLine name={name} view={view} />}
      {!here && !lite && (view.queued || view.asked !== undefined || asset?.standing === "making") && (
        <div className="rest-waiting">
          <Waiting view={view} />
        </div>
      )}
      {here && asset && asset.kind !== "text" && (
        <div className="here-under nodrag nopan nowheel">
          <div className="here-all">
            <Takes view={view} name={name} width={width} />
            {asset.timeline && editing && <Pictures name={name} />}
            {!asset.timeline && !view.hereWords && <FromHere view={view} name={name} />}
            {!editing && <Bar view={view} name={name} />}
            <AssetEdit
              name={name}
              view={view}
              editing={editing}
              actions={
                <>
                  {editing && <RenderButton name={name} view={view} />}
                  {(!view.hereWords || asset.timeline) && <Approve view={view} name={name} />}
                  <Stack name={name} />
                </>
              }
            >
              {editing && <Description name={name} view={view} />}
            </AssetEdit>
            {!editing && <Description name={name} view={view} />}
            {view.warned && warned ? (
              <div className="warned-lines">
                {view.warnings!.map((warning) => (
                  <p key={warning}>{noHash(warning)}</p>
                ))}
              </div>
            ) : null}
            <Status view={view} name={name} />
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
