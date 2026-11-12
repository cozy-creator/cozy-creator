import type { NodeProps } from "@xyflow/react"
import { memo } from "react"
import type { Entity } from "../types"
import { buttonsOnly } from "./Card"
import type { ElementNode } from "./CanvasStore"
import { useElement } from "./context"
import { Thumb } from "./Media"
import { shownOf } from "./shown"
import { StateDot } from "./StateDot"


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

const EntityBody = memo(function EntityBody({ name }: { name: string }) {
  const view = useElement(name)
  const entity = view?.entity
  if (!view || !entity) return null
  return (
    <div className="card entity" data-kind={entity.kind} onClick={buttonsOnly}>
        <div className="head">
          <StateDot state={view.linked === "failed" ? "failed" : view.linked === "making" ? "running" : null} />
          <span className="nm" title={name}>
            {name}
          </span>
          <span className="tag">{entity.kind}</span>
          <span className="sp" />
          {view.warnings?.length ? (
            <span className="warned" role="img" aria-label="Warnings" title={view.warnings.join("\n")}>
              !
            </span>
          ) : null}
        </div>
        <p className="desc">{entity.description}</p>
        <Slots links={entity.links} />
        {(entity.subs ?? []).map((sub) => (
          <Sub key={sub.name} entity={sub} />
        ))}
    </div>
  )
})

/** What a file's entities say, read only, until the format holds only assets. */
export function EntityCard({ data }: NodeProps<ElementNode>) {
  return <EntityBody name={data.name} />
}
