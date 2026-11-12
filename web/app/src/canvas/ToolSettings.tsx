import type { Asset } from "../api/calcifer"
import { useSignal } from "../ui/signal"
import { useCanvasStore } from "./context"
import { modelLabel, operationOf, paletteEntries, toolFitsNode, type Operation } from "./palette"

/** A node chooses an operation and an executable model without changing its output type. */
export function ToolSettings({ name, asset, selected }: { name: string; asset: Asset; selected?: Operation }) {
  const store = useCanvasStore()
  const tools = useSignal(store.tools)
  const current = tools.get(asset.tool ?? "")
  const compatible = [...tools.values()].filter((tool) => toolFitsNode(tool, asset))
  const entries = paletteEntries(compatible).filter((entry) => entry.makes === asset.kind)
  const picked = entries.find((entry) => entry.key === selected && (!current || entry.variants.some((tool) => tool.id === current.id)))
  const operation = picked?.key || (current && operationOf(current)) || entries[0]?.key || ""
  const entry = entries.find((entry) => entry.key === operation)
  const models = entry?.variants ?? compatible
  return (
    <div className="tool-settings nodrag nopan nowheel" data-tool-settings="">
      {entries.length > 0 && (
        <label className="slide">
          <span className="ctl-name">Tool</span>
          <select className="field nodrag" aria-label="Tool" value={operation} onChange={(event) => {
            const key = event.target.value as Operation
            const next = entries.find((candidate) => candidate.key === key)?.tool
            if (next) {
              store.putTool(name, next)
              store.patch(name, { operation: key })
            }
          }}>
            {entries.map((candidate) => <option key={candidate.key} value={candidate.key} disabled={!candidate.tool}>{candidate.label}</option>)}
          </select>
        </label>
      )}
      {operation !== "timeline" && <label className="slide">
        <span className="ctl-name">Model</span>
        <select className="field nodrag" aria-label="Model" value={models.some((tool) => tool.id === asset.tool) ? asset.tool ?? "" : ""} onChange={(event) => {
          const next = tools.get(event.target.value)
          if (next) store.putTool(name, next)
        }}>
          <option value="" disabled>{models.length ? "Choose a model" : "No model available"}</option>
          {models.map((tool) => <option key={tool.id} value={tool.id}>{modelLabel(tool)}</option>)}
        </select>
      </label>}
    </div>
  )
}
