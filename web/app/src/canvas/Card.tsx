import type { NodeProps } from "@xyflow/react"
import { memo, useCallback, useState } from "react"
import { useSignal } from "../ui/signal"
import type { ElementNode, ElementView } from "./CanvasStore"
import { useCanvasStore, useElement } from "./context"
import { Media, stageLine, Thumb } from "./Media"
import { NameField } from "./NameField"
import { type Anchor, anchorsFor, shownOf } from "./shown"
import { beatLength, beatsOf, noHash, piecesOf, took } from "./text"

const DOTS: Record<string, string> = { current: "s-current", out_of_date: "s-ood", making: "s-making", missing: "s-missing", failed: "s-failed" }

function kindOf(view: ElementView): string {
  if (view.placing || view.landed) return "file"
  const asset = view.asset
  if (!asset) return ""
  if (asset.written_by) return "written"
  if (asset.with === "cozy") return asset.kind === "video" ? "cut" : "frame"
  if (asset.field && !asset.with) return "file"
  return asset.kind === "video" ? "shot" : asset.kind
}

function dotOf(view: ElementView): string {
  if (view.placing || view.queued) return "s-making"
  const asset = view.asset
  if (asset?.standing === "making") return "s-making"
  if (asset?.failure) return "s-failed"
  if (asset && shownOf(view)?.absent) return "s-missing"
  return DOTS[asset?.standing ?? "current"] ?? "s-missing"
}

/** Where edges land: a measure of the card finds these as it would React Flow's own handles, at no cost per frame. */
export function Anchors({ anchors }: { anchors: Anchor[] }) {
  return (
    <>
      {anchors.map((anchor) => (
        <div
          key={`${anchor.type}:${anchor.id ?? ""}`}
          className={`anchor ${anchor.type}`}
          data-handleid={anchor.id}
          data-handlepos={anchor.position}
          style={{ left: anchor.x, top: anchor.y }}
        />
      ))}
    </>
  )
}

function PinIcon() {
  return (
    <svg viewBox="0 0 16 16" width="12" height="12" aria-hidden="true">
      <path fill="currentColor" d="M9.8 1.2 14.8 6.2l-1.1 1.1-.9-.2-2.9 2.9.2 3-1.2 1.2-3-3-3.4 3.4-.7-.7 3.4-3.4-3-3 1.2-1.2 3 .2 2.9-2.9-.2-.9z" />
    </svg>
  )
}

/** Stacked behind a card when there are others. Its pin writes or removes the one line in the file. */
function Pin({ view, name }: { view: ElementView; name: string }) {
  const store = useCanvasStore()
  const asset = view.asset
  if (!asset || asset.written_by || (asset.pinned && !asset.with && !asset.prefixes?.length)) return null
  const shown = shownOf(view)
  const lit = Boolean(asset.pinned) && (asset.approved === undefined ? view.viewed === undefined : shown?.number === asset.approved)
  if (!lit && shown?.number === undefined) return null
  return (
    <button
      type="button"
      className={`iconb nodrag ${lit ? "lit" : "hovered"}`}
      aria-label={lit ? "Unpin" : "Pin"}
      title={lit ? "Pinned: nothing makes this again until it's unpinned" : "Pin this one"}
      onClick={() => void (lit ? store.unpin(name) : store.pin(name, shown!.number!))}
    >
      <PinIcon />
    </button>
  )
}

/** On hover: each version, which clicking looks at, and + for a new one. */
function Takes({ view, name, width }: { view: ElementView; name: string; width: number }) {
  const store = useCanvasStore()
  const asset = view.asset!
  const shown = shownOf(view)
  const versions = asset.prefixes ?? []
  if (!asset.with && !versions.length) return null
  if (asset.written_by) return null
  return (
    <div className="takes nodrag nopan nowheel">
      {versions.map((version) => (
        <button
          key={version.number}
          type="button"
          className={`take ${shown?.number === version.number ? "on" : ""}`}
          aria-label={`${asset.noun} ${version.number}`}
          title={[`${asset.noun} ${version.number}`, version.execution_ms ? took(version.execution_ms) : "", version.number === asset.approved ? "pinned" : ""].filter(Boolean).join(", ")}
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
      {asset.with && (
        <button type="button" className="take add" aria-label={`New ${asset.noun}`} title={`New ${asset.noun}`} onClick={() => void store.newTake(name)}>
          +
        </button>
      )}
    </div>
  )
}

/** A shot's beats, each as wide as it lasts, or a cut's pieces. Read only: clicking a beat opens its words, a piece plays from there. */
function Bar({ view, name }: { view: ElementView; name: string }) {
  const store = useCanvasStore()
  const beats = beatsOf(view.asset?.text)
  if (beats.length) {
    return (
      <div className="beats">
        {beats.map((beat, i) => (
          <button key={i} type="button" className="nodrag" style={{ flexGrow: beatLength(beat) }} onClick={() => store.open.set({ name, at: `beats.${i}` })}>
            {beatLength(beat)}s
          </button>
        ))}
      </div>
    )
  }
  const pieces = piecesOf(view.asset?.text)
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

function About({ seconds }: { seconds: number }) {
  return <>about {seconds < 90 ? `${Math.max(1, Math.round(seconds))} s` : `${Math.round(seconds / 60)} min`} left</>
}

function Pull() {
  const store = useCanvasStore()
  const moving = useSignal(store.transferring)
  return (
    <div className="status">
      On another computer {moving ? "· pulling" : <Act label="Pull" onClick={() => void store.pull()} />}
    </div>
  )
}

/** What it says under its media: how far an import or a run is, and what's wrong and what to do about it. Never a hash. */
function Status({ view, name }: { view: ElementView; name: string }) {
  const store = useCanvasStore()
  const will = store.nameWillBe(name)
  if (will) return <div className="status">Will be {will}</div>
  const stage = stageLine(view)
  if (stage) return <div className="status mak">{stage}</div>
  const asset = view.asset
  if (!asset) return null
  if (asset.standing === "making") {
    const run = view.run
    const stalled = run?.stalled
    if (stalled) {
      const since = took(Math.max(0, Date.now() - stalled.since_ms))
      return (
        <div className="status bad">
          Stalled {since} on {stalled.machine}
          {stalled.usd_micros_per_hour !== undefined && ` · ${usdPerHour(stalled.usd_micros_per_hour)}`} <Act label="Cancel" onClick={() => void store.cancelRender(asset.name)} />
        </div>
      )
    }
    const progress = run?.progress
    if (run?.queue_position !== undefined) return <div className="status mak">Queued, number {run.queue_position} in line</div>
    return (
      <div className="status mak">
        Making
        {progress?.done_fraction !== undefined && `, ${Math.round(progress.done_fraction * 100)}%`}
        {progress?.eta_s !== undefined && (
          <>
            , <About seconds={progress.eta_s} />
          </>
        )}
      </div>
    )
  }
  if (view.queued) return <div className="status mak">Queued</div>
  if (asset.failure) {
    const reason = noHash(asset.failure.message).replace(`${asset.name}: `, "")
    return (
      <div className="status bad">
        {reason} {asset.with && <Act label="Try again" onClick={() => void store.newTake(name)} />}
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
    return (
      <div className="status ood">
        {noHash(asset.status)} {asset.with && <Act label={`New ${asset.noun}`} onClick={() => void store.newTake(name)} />}
      </div>
    )
  }
  if (asset.standing === "missing") {
    return (
      <div className="status">
        {noHash(asset.status)} {asset.with && <Act label={`New ${asset.noun}`} onClick={() => void store.newTake(name)} />}
      </div>
    )
  }
  if (asset.written_by || asset.status.includes("inputs changed")) return <div className="status">{noHash(asset.status)}</div>
  return null
}

const CardBody = memo(function CardBody({ name, width }: { name: string; width: number }) {
  const store = useCanvasStore()
  const view = useElement(name)
  const [hover, setHover] = useState(false)
  const open = useCallback(() => store.openName(name), [store, name])
  const cancel = useCallback(() => {
    if (store.get(name)?.placing) store.cancel(store.resolveName(name))
    else store.patch(name, { draft: undefined })
  }, [store, name])
  if (!view) return null

  const asset = view.asset
  const versions = asset?.prefixes ?? []
  const shown = shownOf(view)
  const at = versions.findIndex((version) => version.number === shown?.number)
  return (
    <>
      <Anchors anchors={anchorsFor(view, width)} />
      <div className={`card ${versions.length > 1 ? "stacked" : ""}`} onPointerEnter={() => setHover(true)} onPointerLeave={() => setHover(false)}>
        <div className="head">
          {view.draft !== undefined ? (
            <NameField name={name} draft={view.draft} onCancel={cancel} />
          ) : (
            <span className="nm" title={view.name} onDoubleClick={open}>
              {view.placing ? store.shownName(store.resolveName(name)) : view.name}
            </span>
          )}
          {view.draft === undefined && kindOf(view) && <span className="tag">{kindOf(view)}</span>}
          {view.draft === undefined && <span className="sp" />}
          {view.draft === undefined && versions.length > 1 && (
            <span className="count">
              {at + 1}/{versions.length}
            </span>
          )}
          {view.draft === undefined && view.warnings?.length ? (
            <button type="button" className="iconb warned nodrag" aria-label="Warnings" title={noHash(view.warnings.join("\n"))} onClick={() => store.open.set({ name })}>
              !
            </button>
          ) : null}
          {view.draft === undefined && <Pin view={view} name={name} />}
          {view.draft === undefined && asset && (
            <button type="button" className="iconb hovered nodrag" aria-label="Its words" title="Its words" onClick={() => store.open.set({ name })}>
              {"{ }"}
            </button>
          )}
          {view.placing && view.placing.stage !== "adding" && (
            <button type="button" className="iconb nodrag" aria-label="Cancel" onClick={cancel}>
              ×
            </button>
          )}
          <span className={`dot ${dotOf(view)}`} />
        </div>
        <Media view={view} name={name} cardWidth={width}>
          {hover && asset && !view.placing && <Takes view={view} name={name} width={width} />}
        </Media>
        <Bar view={view} name={name} />
        <Status view={view} name={name} />
      </div>
    </>
  )
})

/** Thin, as React Flow renders it: `data` is only the name. Element types are `nodeTypes`, looks are `CardBody`. */
export function Card({ data, width }: NodeProps<ElementNode>) {
  return <CardBody name={data.name} width={width ?? 240} />
}
