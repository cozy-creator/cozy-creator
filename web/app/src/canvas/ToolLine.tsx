import type { ElementView } from "./CanvasStore"
import { type CSSProperties, useState } from "react"
import { Icon } from "../ui/Icon"
import { useSignal } from "../ui/signal"
import { useCanvasStore, useElement } from "./context"
import { ModelPicker, slotsOf } from "./ModelPicker"
import { ModelConfig } from "./ModelConfig"
import { toolName } from "./shown"

/** What makes a box, named once as the canvas names it, "<Package> · <Tool>", with its trash can (cl-934, cl-939). */
function ToolName({ name }: { name: string }) {
  const store = useCanvasStore()
  const view = useElement(name)
  const tools = useSignal(store.tools)
  const asset = view?.asset
  if (!asset?.tool) return null
  const tool = tools.get(asset.tool)
  return (
    <span className="image-tool" title={tool?.source ? `${asset.tool}, from source: ${tool.source}` : asset.tool}>
      <span className="image-tool-name">{tool ? toolName(tool) : asset.tool}</span>
      {tool?.source && <small className="image-tool-source">from source · {tool.source}</small>}
      <button type="button" title={`Take ${tool ? toolName(tool) : asset.tool} off`} aria-label={`Remove ${asset.kind} tool`} onClick={() => store.takeToolOff(name)}><Icon name="trash" /></button>
    </span>
  )
}

/** Models & LoRAs, the same for every tool with model slots: it opens the one picker, in place under the tool, and closes it. */
function ModelsButton({ name, open, onToggle }: { name: string; open: boolean; onToggle(): void }) {
  const store = useCanvasStore()
  const view = useElement(name)
  const tools = useSignal(store.tools)
  if (!slotsOf(tools.get(view?.asset?.tool ?? "")).length) return null
  return <button type="button" className="image-models" aria-label="Models and LoRAs" aria-expanded={open} onClick={onToggle}>Models &amp; LoRAs</button>
}

/** Beneath every node with a tool, the same for every tool and kind (Paul, 11-12: "We should make this a standard type of interface"): its
 *  tool, Models & LoRAs beside it, and the models and LoRAs set listed under them. Nothing of models or LoRAs is inside the card. */
export function ToolLine({ name, view, width }: { name: string; view: ElementView; width: number }) {
  const [picking, setPicking] = useState(false)
  if (!view.asset?.tool) return null
  return (
    <div className="image-controls tool-line nodrag nopan nowheel" style={{ "--image-control-width": `${width}px` } as CSSProperties}>
      <div className="image-version-row">
        <ToolName name={name} />
        <ModelsButton name={name} open={picking} onToggle={() => setPicking((now) => !now)} />
      </div>
      {picking && <ModelPicker name={name} onClose={() => setPicking(false)} />}
      <ModelConfig name={name} />
    </div>
  )
}
