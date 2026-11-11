import { useState } from "react"
import type { Asset } from "../api/calcifer"
import { useSignal } from "../ui/signal"
import { useCanvasStore } from "./context"
import { isVideoEdit, modelLabel, operationOf, paletteEntries, toolFitsNode, type Operation } from "./palette"

/** A node chooses an operation and an executable model without changing its output type. */
export function ToolSettings({ name, asset }: { name: string; asset: Asset }) {
  const store = useCanvasStore()
  const tools = useSignal(store.tools)
  const current = tools.get(asset.tool ?? "")
  const [picked, setPicked] = useState<Operation | "">("")
  if (isVideoEdit(asset)) return null
  const compatible = [...tools.values()].filter((tool) => toolFitsNode(tool, asset))
  const entries = paletteEntries(compatible).filter((entry) => entry.makes === asset.kind)
  const operation = picked || (current && operationOf(current)) || entries[0]?.key || ""
  const entry = entries.find((entry) => entry.key === operation)
  const models = entry?.variants ?? compatible
  return (
    <div className="tool-settings nodrag nopan nowheel" data-tool-settings="">
      {entries.length > 0 && (
        <label className="slide">
          <span className="ctl-name">Tool</span>
          <select className="field nodrag" aria-label="Tool" value={operation} onChange={(event) => {
            const key = event.target.value as Operation
            setPicked(key)
            const next = entries.find((candidate) => candidate.key === key)?.tool
            if (next) store.putTool(name, next)
          }}>
            {entries.map((candidate) => <option key={candidate.key} value={candidate.key}>{candidate.label}</option>)}
          </select>
        </label>
      )}
      <label className="slide">
        <span className="ctl-name">Model</span>
        <select className="field nodrag" aria-label="Model" value={models.some((tool) => tool.id === asset.tool) ? asset.tool : ""} onChange={(event) => {
          const next = tools.get(event.target.value)
          if (next) store.putTool(name, next)
        }}>
          <option value="" disabled>{models.length ? "Choose a model" : "No model available"}</option>
          {models.map((tool) => <option key={tool.id} value={tool.id}>{modelLabel(tool)}</option>)}
        </select>
      </label>
    </div>
  )
}
