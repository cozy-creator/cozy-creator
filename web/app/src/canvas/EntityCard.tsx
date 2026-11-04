import type { NodeProps } from "@xyflow/react"
import { memo } from "react"
import type { Entity } from "../types"
import { Anchors } from "./Card"
import type { ElementNode } from "./CanvasStore"
import { useCanvasStore, useElement } from "./context"
import { Thumb } from "./Media"
import { anchorsFor, shownOf } from "./shown"

const DOTS: Record<string, string> = { current: "s-current", out_of_date: "s-ood", making: "s-making", missing: "s-missing", failed: "s-failed" }

/** One link: that asset's picture and how it stands, live, whatever else the entity does. */
function Slot({ field, target }: { field: string; target: string }) {
  const view = useElement(target)
  const shown = view && shownOf(view)
  const picture = shown && !shown.absent && shown.field !== "audio" ? shown : null
  return (
    <div className="slot" title={`${field}: ${target}`}>
      <div className={`slot-media ${picture?.alpha ? "alpha" : ""}`}>
        {picture ? <Thumb sha256={picture.sha256} cardWidth={120} /> : <span>{view?.asset?.standing === "making" ? "making" : field}</span>}
      </div>
      <span className="slot-name">{field}</span>
    </div>
  )
}

function Slots({ links }: { links?: Record<string, string> }) {
  const held = Object.entries(links ?? {})
  if (!held.length) return null
  return (
    <div className="slots">
      {held.map(([field, target]) => (
        <Slot key={field} field={field} target={target} />
      ))}
    </div>
  )
}

function Sub({ entity }: { entity: Entity }) {
  return (
    <div className="sub">
      <div className="sub-name">{entity.name}</div>
      <p className="desc two">{entity.description}</p>
      <Slots links={entity.links} />
    </div>
  )
}

const EntityBody = memo(function EntityBody({ name, width }: { name: string; width: number }) {
  const store = useCanvasStore()
  const view = useElement(name)
  const entity = view?.entity
  if (!view || !entity) return null
  return (
    <>
      <Anchors anchors={anchorsFor(view, width)} />
      <div className="card entity">
        <div className="head">
          <span className="nm" title={name}>
            {name}
          </span>
          <span className="tag">{entity.kind}</span>
          <span className="sp" />
          {view.warnings?.length ? (
            <button type="button" className="iconb warned nodrag" aria-label="Warnings" onClick={() => store.open.set({ name })}>
              !
            </button>
          ) : null}
          <button type="button" className="iconb hovered nodrag" aria-label="Its words" title="Its words" onClick={() => store.open.set({ name })}>
            {"{ }"}
          </button>
          <span className={`dot ${DOTS[view.linked ?? "current"] ?? "s-missing"}`} />
        </div>
        <p className="desc">{entity.description}</p>
        <Slots links={entity.links} />
        {(entity.subs ?? []).map((sub) => (
          <Sub key={sub.name} entity={sub} />
        ))}
      </div>
    </>
  )
})

/** An entity is a card with slots, not a frame: moving anything never means anything. */
export function EntityCard({ data, width }: NodeProps<ElementNode>) {
  return <EntityBody name={data.name} width={width ?? 240} />
}
