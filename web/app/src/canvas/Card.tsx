import { useQuery } from "@tanstack/react-query"
import type { NodeProps } from "@xyflow/react"
import { useReactFlow } from "@xyflow/react"
import { type CSSProperties, type DragEvent, type MouseEvent, memo, type PointerEvent, useCallback, useEffect, useRef, useState } from "react"
import { runsQuery } from "../api/queries"
import { useConnection } from "../app/Connection"
import type { Run, Tool } from "../types"
import { Icon } from "../ui/Icon"
import { useSignal } from "../ui/signal"
import { AssetEdit, Conflict, InFrame, RenderButton, TextAsset } from "./AssetEdit"
import type { ElementNode, ElementView } from "./CanvasStore"
import { useCanvasStore, useElement } from "./context"
import { controlsOf } from "./controls"
import { DRAGGED_FILE } from "./drag"
import { Media, Thumb } from "./Media"
import { MentionText } from "./MentionText"
import { NameField } from "./NameField"
import { faceOf, madeHere, makes, shownOf } from "./shown"
import { clock, noHash, piecesOf, rowsIn, took } from "./text"
import { TimelineControl } from "./timeline/TimelineControl"

const DOTS: Record<string, string> = { current: "s-current", out_of_date: "s-ood", making: "s-making", missing: "s-missing", failed: "s-failed" }
const WIDEST = 1600
const NARROWEST = 160

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
function Waiting() {
  const { client } = useConnection()
  const going = useQuery(runsQuery(client, { status: "in_progress" }, 100))
  const queued = useQuery(runsQuery(client, { status: "queued" }, 100))
  const machine = [...(going.data?.runs ?? []), ...(queued.data?.runs ?? [])].find(settingUp)?.machine
  return <div className="status mak">{machine ? `Getting ${machine} ready` : "Queued"}</div>
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

/** Each take or version, side by side, which clicking looks at and dragging takes out, and + for a new one. */
function Takes({ view, name, width }: { view: ElementView; name: string; width: number }) {
  const store = useCanvasStore()
  const asset = view.asset!
  const shown = shownOf(view)
  const versions = asset.prefixes ?? []
  const adds = asset.tool && faceOf(view) !== "edit"
  if (!adds && versions.length < (asset.tool ? 1 : 2)) return null
  const takeOut = (event: DragEvent, version: (typeof versions)[number]) => {
    const description = store.valueOf(name, "description")
    const file = { sha256: version.main?.sha256, field: version.main?.field, name: `${asset.name}_${version.number}`, description: typeof description === "string" ? description : undefined }
    event.dataTransfer.setData(DRAGGED_FILE, JSON.stringify(file))
    event.dataTransfer.effectAllowed = "copy"
  }
  return (
    <div
      className="takes nodrag nopan nowheel"
      onWheel={(event) => {
        event.currentTarget.scrollLeft += event.deltaX || event.deltaY
      }}
    >
      {versions.map((version) => (
        <span key={version.number} className="take-at">
          <button
            type="button"
            className={`take ${shown?.number === version.number ? "on" : ""} ${view.slid === version.number ? "fresh" : ""}`}
            aria-label={`${asset.noun} ${version.number}`}
            title={[`${asset.noun} ${version.number}`, version.execution_ms ? took(version.execution_ms) : "", version.number === asset.approved ? "approved" : ""].filter(Boolean).join(", ")}
            draggable={Boolean(version.main)}
            onDragStart={(event) => takeOut(event, version)}
            onClick={() => store.view(name, version.number === asset.version ? undefined : version.number)}
          >
            {version.main && !version.main.absent ? <Thumb sha256={version.main.sha256} cardWidth={width} /> : <span>{version.number}</span>}
          </button>
          <button type="button" className="take-off" aria-label={`Discard ${asset.noun} ${version.number}`} title={`Discard ${asset.noun} ${version.number}`} onClick={() => store.discardTake(name, version.number)}>
            <Icon name="close" />
          </button>
        </span>
      ))}
      {(asset.making ?? []).map((run) => (
        <span key={run} className="take making" title={`Run ${run}`}>
          {view.run?.number === run && view.run.progress?.done_fraction !== undefined ? `${Math.round(view.run.progress.done_fraction * 100)}%` : "…"}
        </span>
      ))}
      {adds && (
        <button type="button" className="take add" aria-label={`New ${asset.noun}`} title={`New ${asset.noun}`} onClick={() => store.run(name)}>
          +
        </button>
      )}
    </div>
  )
}

/** The one thing to do with what's being looked at: approve it, or unapprove what's approved. */
function Approve({ view, name }: { view: ElementView; name: string }) {
  const store = useCanvasStore()
  const asset = view.asset
  if (!asset || !asset.tool) return null
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

/** The rows of a group that adds up, each as wide as it lasts, or a cut's pieces. Read only: a row opens the box's controls, a piece plays from there. */
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
          <button key={i} type="button" className="nodrag" style={{ flexGrow: lasts(row) }} onClick={() => store.patch(name, { face: "edit" })}>
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
    // An Edit's film shows on its own bar, and anything else's on its picture.
    return null
  }
  if (view.queued || view.asked !== undefined) return <Waiting />
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

/** What this asset is, in words of its own, whichever way it was made. It reaches what mentions it. It opens on a click, so a small picture isn't crowded; a caption opens it too, as an edit that's still only typed. */
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
      {!open && (
        <div className="row">
          <button type="button" className="act nodrag" onClick={() => void store.save(name)}>
            Save
          </button>
          <button type="button" className="act nodrag" onClick={() => store.discard(name, "description")}>
            Discard
          </button>
        </div>
      )}
    </div>
  )
}

/** The corner, pulled to make a box wider or narrower on the canvas. What it makes is another matter, and that's in the box. */
function Corner({ name, width }: { name: string; width: number }) {
  const store = useCanvasStore()
  const flow = useReactFlow()
  const from = useRef<{ x: number; w: number } | null>(null)
  const wide = useRef<number | null>(null)
  const moved = (event: PointerEvent) => {
    if (!from.current) return
    const w = Math.min(WIDEST, Math.max(NARROWEST, from.current.w + (event.clientX - from.current.x) / flow.getZoom()))
    wide.current = w
    store.setWidth(name, w, false)
  }
  const done = () => {
    if (!from.current) return
    from.current = null
    if (wide.current !== null) store.setWidth(name, wide.current)
    wide.current = null
  }
  return (
    <div
      className="corner nodrag nopan"
      role="slider"
      aria-label="Its width"
      aria-valuenow={Math.round(width)}
      tabIndex={-1}
      onClick={(event) => event.stopPropagation()}
      onPointerDown={(event) => {
        event.currentTarget.setPointerCapture(event.pointerId)
        from.current = { x: event.clientX, w: width }
      }}
      onPointerMove={moved}
      onPointerUp={done}
      onPointerCancel={done}
    />
  )
}

const CardBody = memo(function CardBody({ name, width, close }: { name: string; width: number; close?: () => void }) {
  const store = useCanvasStore()
  const view = useElement(name)
  const open = Boolean(close)
  const naming = useCallback(() => store.openName(name), [store, name])
  const cancel = useCallback(() => {
    if (store.get(name)?.placing) store.cancel(store.resolveName(name))
    else store.patch(name, { draft: undefined })
  }, [store, name])
  // Opened, its first text has taken the keys.
  useEffect(() => {
    if (store.focusing === name) store.focusing = null
  })
  if (!view) return null

  const asset = view.asset
  const versions = asset?.prefixes ?? []
  const shown = shownOf(view)
  const at = versions.findIndex((version) => version.number === shown?.number)
  const face = faceOf(view)
  const canFlip = makes(view)
  // On the canvas a box is its name, its picture or its text, its state and an X. All else is in it, a click away, so 300 boxes cost little.
  const editing = face === "edit" && open
  // An approved box says nothing but what's asked of it: what's wrong with its words is on its controls.
  const warned = view.warnings?.length && !(asset?.pinned && canFlip && face === "media")
  const actions = (
    <>
      {editing && <RenderButton name={name} view={view} />}
      <Approve view={view} name={name} />
      {Object.keys(view.drafts ?? {}).some((field) => field !== "description") && (
        <button type="button" className="act nodrag" onClick={() => void store.save(name)}>
          Save
        </button>
      )}
    </>
  )
  const more = asset && !view.placing && <Description name={name} view={view} />
  return (
    <div
      className={`card ${editing ? "editing" : ""} ${open ? "active" : ""} ${view.landing ? "landing" : ""} ${view.hot ? "hot" : ""} ${asset?.timeline && face === "edit" ? "tl-rests" : ""}`}
      data-asset={view.name}
      data-kind={kindOf(view)}
      onClick={buttonsOnly}
      onAnimationEnd={(event) => {
        if (event.target === event.currentTarget && view.landing) store.patch(name, { landing: undefined })
      }}
    >
      <div className="head">
        {view.draft !== undefined && (open || store.opened !== view.name) ? (
          <NameField name={name} draft={view.draft} onCancel={cancel} />
        ) : (
          <span className="nm" title={view.name} onDoubleClick={open ? naming : undefined}>
            {view.placing ? store.shownName(store.resolveName(name)) : view.name}
          </span>
        )}
        {view.draft === undefined && <span className="sp" />}
        {view.draft === undefined && open && versions.length > 1 && (
          <span className="count">
            {at + 1}/{versions.length}
          </span>
        )}
        {view.draft === undefined && open && warned ? (
          <button type="button" className="iconb warned nodrag" aria-label="Warnings" title={noHash(view.warnings!.join("\n"))} onClick={() => store.patch(name, { warned: !view.warned })}>
            !
          </button>
        ) : null}
        {open && (
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
        {view.draft === undefined && open && canFlip && (
          <button
            type="button"
            className="iconb nodrag"
            aria-label={face === "edit" ? `Show its ${asset?.noun ?? "file"}` : "Edit"}
            title={face === "edit" ? `Show its ${asset?.noun ?? "file"}` : "Edit"}
            onClick={() => store.patch(name, { face: face === "edit" ? "media" : "edit" })}
          >
            <Icon name={face === "edit" ? "picture" : "edit"} />
          </button>
        )}
        {close ? (
          <button type="button" className="iconb nodrag" aria-label="Close" title="Close" onClick={close}>
            <Icon name="close" />
          </button>
        ) : view.placing && view.placing.stage !== "adding" ? (
          <button type="button" className="iconb nodrag" aria-label="Cancel" onClick={cancel}>
            <Icon name="close" />
          </button>
        ) : (
          view.draft === undefined &&
          asset && (
            <button type="button" className="iconb nodrag on-hover" aria-label="Delete" title="Delete" onClick={() => store.remove([name])}>
              <Icon name="close" />
            </button>
          )
        )}
        <span className={`dot ${dotOf(view)}`} />
      </div>
      {open && asset?.kind === "text" ? (
        <TextAsset name={name} view={view} />
      ) : asset?.timeline && editing ? (
        <TimelineControl name={name} />
      ) : (
        <Media view={view} name={name} cardWidth={width} open={open}>
          {editing && <InFrame name={name} view={view} />}
          {!open && asset && !view.placing && <Corner name={name} width={width} />}
        </Media>
      )}
      {open && asset && !view.placing && !asset.timeline && <FromHere view={view} name={name} />}
      {open && asset?.kind !== "text" && (
        <div className="asset-more">
          {asset && !view.placing && <Takes view={view} name={name} width={width} />}
          {editing ? (
            <AssetEdit name={name} view={view} actions={actions}>
              {more}
            </AssetEdit>
          ) : (
            <>
              {asset && !view.placing && <div className="row actions">{actions}</div>}
              <Bar view={view} name={name} />
              {asset && !view.placing && <Description name={name} view={view} />}
              {view.conflict && <Conflict name={name} />}
            </>
          )}
          {view.warned && warned ? (
            <div className="warned-lines">
              {view.warnings!.map((warning) => (
                <p key={warning}>{noHash(warning)}</p>
              ))}
            </div>
          ) : null}
          <Status view={view} name={name} />
        </div>
      )}
    </div>
  )
})

/** A box opened: all of it, at the size of the room it's given. */
export function OpenCard({ name, close }: { name: string; close: () => void }) {
  return <CardBody name={name} width={960} close={close} />
}

/** Thin, as React Flow renders it: `data` is only the name. Element types are `nodeTypes`, looks are `CardBody`. */
export function Card({ data, width }: NodeProps<ElementNode>) {
  return <CardBody name={data.name} width={width ?? 240} />
}
