import type { NodeProps } from "@xyflow/react"
import { memo, useCallback } from "react"
import type { ElementNode, ElementView } from "./CanvasStore"
import { useCanvasStore, useElement } from "./context"
import { Media, stageLine } from "./Media"
import { NameField } from "./NameField"

const DOTS: Record<string, string> = { current: "s-current", out_of_date: "s-ood", making: "s-making", missing: "s-missing" }

function kindOf(view: ElementView): string {
  if (view.placing || view.landed) return "file"
  const asset = view.asset
  if (!asset) return ""
  return asset.field && !asset.with ? "file" : asset.kind
}

/** What it says under the media: an import's stage, how far a run is, why it's out of date. Never a hash. */
function statusLine(view: ElementView): { text: string; tone: string } | null {
  const stage = stageLine(view)
  if (stage) return { text: stage, tone: "mak" }
  const asset = view.asset
  if (!asset) return null
  const fraction = view.run?.progress?.done_fraction
  if (asset.standing === "making") {
    const how = fraction === undefined ? "" : `, ${Math.round(fraction * 100)}%`
    return { text: `making${how}`, tone: "mak" }
  }
  if (asset.standing === "out_of_date") return { text: asset.status, tone: "ood" }
  if (asset.standing === "missing") return { text: asset.status, tone: "" }
  return null
}

const CardBody = memo(function CardBody({ name, width }: { name: string; width: number }) {
  const store = useCanvasStore()
  const view = useElement(name)
  const open = useCallback(() => store.openName(name), [store, name])
  const cancel = useCallback(() => {
    if (store.get(name)?.placing) store.cancel(store.resolveName(name))
    else store.patch(name, { draft: undefined })
  }, [store, name])
  if (!view) return null

  const will = store.nameWillBe(name)
  const status = will ? { text: `Will be ${will}`, tone: "" } : statusLine(view)
  const dot = view.placing ? "s-making" : (DOTS[view.asset?.standing ?? "current"] ?? "s-missing")
  return (
    <div className="card">
      <div className="head">
        {view.draft !== undefined ? (
          <NameField name={name} draft={view.draft} onCancel={cancel} />
        ) : (
          <span className="nm" onDoubleClick={open}>
            {view.placing ? store.shownName(store.resolveName(name)) : view.name}
          </span>
        )}
        {view.draft === undefined && kindOf(view) && <span className="tag">{kindOf(view)}</span>}
        {view.draft === undefined && <span className="sp" />}
        {view.placing && view.placing.stage !== "adding" && (
          <button type="button" className="iconb nodrag" aria-label="Cancel" onClick={cancel}>
            ×
          </button>
        )}
        <span className={`dot ${dot}`} />
      </div>
      <Media view={view} name={name} cardWidth={width} />
      {status && <div className={`status ${status.tone}`}>{status.text}</div>}
    </div>
  )
})

/** Thin, as React Flow renders it: `data` is only the name. Element types are `nodeTypes`, looks are `CardBody`. */
export function Card({ data, width }: NodeProps<ElementNode>) {
  return <CardBody name={data.name} width={width ?? 240} />
}

export const nodeTypes = { element: Card }
