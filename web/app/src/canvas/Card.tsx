import type { NodeProps } from "@xyflow/react"
import { useReactFlow, useStore } from "@xyflow/react"
import { type DragEvent, type MouseEvent, memo, type PointerEvent, useCallback, useEffect, useRef, useState, useSyncExternalStore } from "react"
import { Icon } from "../ui/Icon"
import { useSignal } from "../ui/signal"
import { AssetEdit, Conflict, InFrame, RenderButton, TextAsset } from "./AssetEdit"
import type { ElementNode, ElementView } from "./CanvasStore"
import { useCanvasStore, useElement } from "./context"
import { controlsOf, shapesOf, snapped } from "./controls"
import { DRAGGED_FILE } from "./drag"
import { Media, Thumb } from "./Media"
import { MentionText } from "./MentionText"
import { NameField } from "./NameField"
import { aspectOf, faceOf, madeHere, makes, ratioOf, shownOf } from "./shown"
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
  if (versions.length < 2 && !adds && !(versions.length && asset.making?.length)) return null
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
        <button
          key={version.number}
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

/** On a video: scrub to a point, and a new take keeps it up to there. Its words stay as they are. */
function FromHere({ view, name }: { view: ElementView; name: string }) {
  const store = useCanvasStore()
  const tools = useSignal(store.tools)
  const shown = shownOf(view)
  if (!makes(view) || !tools.get(view.asset?.tool ?? "")?.takes || shown?.field !== "video" || shown.number === undefined || !shown.duration_ms) return null
  const at = view.scrub ?? 0
  return (
    <div className="from-here">
      <input
        className="nodrag nopan"
        type="range"
        aria-label="Where to keep it up to"
        min={0}
        max={shown.duration_ms / 1000}
        step={0.1}
        value={at}
        onChange={(event) => store.patch(name, { scrub: Number(event.target.value) })}
      />
      <span className="tc-inline">{clock(at * 1000)}</span>
      <button type="button" className="act nodrag" disabled={at <= 0} onClick={() => store.run(name, { take: shown.number, in_s: at })}>
        New take from here
      </button>
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
  if (view.queued || view.asked !== undefined) return <div className="status mak">Queued</div>
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

/** The corner. On a box's controls it reshapes what it will make, snapping to what's allowed and saying which; anywhere else it's only the width. */
function Corner({ name, view, width }: { name: string; view: ElementView; width: number }) {
  const store = useCanvasStore()
  const flow = useReactFlow()
  const tools = useSignal(store.tools)
  const from = useRef<{ x: number; y: number; w: number; h: number } | null>(null)
  const wide = useRef<number | null>(null)
  const shape = faceOf(view) === "edit" && view.asset ? controlsOf(view.asset, tools).find((control) => control.type === "shape") : undefined

  const moved = (event: PointerEvent) => {
    if (!from.current) return
    const zoom = flow.getZoom()
    const w = Math.min(WIDEST, Math.max(NARROWEST, from.current.w + (event.clientX - from.current.x) / zoom))
    wide.current = w
    if (shape) store.patch(name, { shape: snapped(w / Math.max(40, from.current.h + (event.clientY - from.current.y) / zoom), shapesOf(shape)) })
    store.setWidth(name, w, false)
  }
  const done = () => {
    if (!from.current) return
    from.current = null
    const now = store.get(name)
    if (wide.current !== null) store.setWidth(name, wide.current)
    wide.current = null
    if (shape && now?.shape) void store.setField(name, shape.field, now.shape).then(() => store.patch(name, { shape: undefined }))
  }
  return (
    <>
      {shape && <span className="ratio-shown">{String(view.shape ?? store.valueOf(name, shape.field) ?? shape.default ?? "")}</span>}
      <div
        className="corner nodrag nopan"
        role="slider"
        aria-label={shape ? "Its shape" : "Its width"}
        aria-valuenow={Math.round(width)}
        tabIndex={-1}
        onPointerDown={(event) => {
          event.currentTarget.setPointerCapture(event.pointerId)
          from.current = { x: event.clientX, y: event.clientY, w: width, h: width / (ratioOf(view.shape) ?? aspectOf(view)) }
        }}
        onPointerMove={moved}
        onPointerUp={done}
        onPointerCancel={done}
      />
    </>
  )
}

const CardBody = memo(function CardBody({ name, width, selected }: { name: string; width: number; selected: boolean }) {
  const store = useCanvasStore()
  const view = useElement(name)
  const far = useStore((state) => state.transform[2] < 0.2)
  const [hover, setHover] = useState(false)
  const [focused, setFocused] = useState(false)
  const typedIn = useSyncExternalStore(store.editing.subscribe, () => store.editing.get() === name)
  const open = useCallback(() => store.openName(name), [store, name])
  const cancel = useCallback(() => {
    if (store.get(name)?.placing) store.cancel(store.resolveName(name))
    else store.patch(name, { draft: undefined })
  }, [store, name])
  // A dropped tool's text has taken the keys, or isn't going to.
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
  // At rest a box is its picture or its text and a name. All else comes when it's asked for, and isn't there until then, so 300 boxes cost little.
  const active = !far && (hover || selected || focused || typedIn || Boolean(view.hot) || Boolean(view.drafts) || view.draft !== undefined || store.focusing === name)
  const editing = face === "edit" && active
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
  const more = asset && !view.placing && (
    <>
      {!asset.timeline && <FromHere view={view} name={name} />}
      <Description name={name} view={view} />
    </>
  )
  return (
    <div
      className={`card ${editing ? "editing" : ""} ${active ? "active" : ""} ${view.landing ? "landing" : ""} ${view.hot ? "hot" : ""} ${asset?.timeline && face === "edit" ? "tl-rests" : ""}`}
      data-asset={view.name}
      data-kind={kindOf(view)}
      onClick={buttonsOnly}
      onPointerEnter={() => setHover(true)}
      onPointerLeave={() => setHover(false)}
      onFocus={(event) => setFocused(event.target.matches(":focus-visible"))}
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget)) setFocused(false)
      }}
      onAnimationEnd={(event) => {
        if (event.target === event.currentTarget && view.landing) store.patch(name, { landing: undefined })
      }}
    >
      <div className="head">
        {view.draft !== undefined ? (
          <NameField name={name} draft={view.draft} onCancel={cancel} />
        ) : (
          <span className="nm" title={view.name} onDoubleClick={open}>
            {view.placing ? store.shownName(store.resolveName(name)) : view.name}
          </span>
        )}
        {view.draft === undefined && <span className="sp" />}
        {view.draft === undefined && active && versions.length > 1 && (
          <span className="count">
            {at + 1}/{versions.length}
          </span>
        )}
        {view.draft === undefined && active && warned ? (
          <button type="button" className="iconb warned nodrag" aria-label="Warnings" title={noHash(view.warnings!.join("\n"))} onClick={() => store.patch(name, { warned: !view.warned })}>
            !
          </button>
        ) : null}
        {view.draft === undefined && active && canFlip && (
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
        {view.placing && view.placing.stage !== "adding" ? (
          <button type="button" className="iconb nodrag" aria-label="Cancel" onClick={cancel}>
            <Icon name="close" />
          </button>
        ) : (
          view.draft === undefined &&
          active &&
          asset && (
            <button type="button" className="iconb nodrag" aria-label="Delete" title="Delete" onClick={() => store.remove([name])}>
              <Icon name="close" />
            </button>
          )
        )}
        <span className={`dot ${dotOf(view)}`} />
      </div>
      {asset?.kind === "text" ? (
        <TextAsset name={name} view={view} />
      ) : asset?.timeline && editing ? (
        <TimelineControl name={name} />
      ) : (
        <Media view={view} name={name} cardWidth={width}>
          {editing && <InFrame name={name} view={view} />}
          {active && asset && !view.placing && <Corner name={name} view={view} width={width} />}
        </Media>
      )}
      {active && asset?.kind !== "text" && (
        <div className="asset-more">
          {asset && !view.placing && <Takes view={view} name={name} width={width} />}
          {editing ? (
            <AssetEdit name={name} view={view} actions={actions}>
              {more}
            </AssetEdit>
          ) : (
            <>
              {asset && !view.placing && <div className="row actions">{actions}</div>}
              {asset && !view.placing && !asset.timeline && <FromHere view={view} name={name} />}
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

/** Thin, as React Flow renders it: `data` is only the name. Element types are `nodeTypes`, looks are `CardBody`. */
export function Card({ data, width, selected }: NodeProps<ElementNode>) {
  return <CardBody name={data.name} width={width ?? 240} selected={Boolean(selected)} />
}
