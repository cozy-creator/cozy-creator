import type { ElementView } from "./CanvasStore"
import { type CSSProperties, useState } from "react"
import { Icon } from "../ui/Icon"
import { useSignal } from "../ui/signal"
import { useCanvasStore, useElement } from "./context"
import { ModelPicker, slotsOf } from "./ModelPicker"
import { ModelConfig } from "./ModelConfig"
import { toolName } from "./shown"

/** What makes a box, named once as the canvas names it, "<Package> · <Tool>", with its × (cl-934, Paul 11-12). */
function ToolName({ name }: { name: string }) {
  const store = useCanvasStore()
  const view = useElement(name)
  const tools = useSignal(store.tools)
  const asset = view?.asset
  if (!asset?.tool) return null
  const tool = tools.get(asset.tool)
  return (
    <span className="image-tool" title={asset.tool}>
      <span className="image-tool-name">{tool ? toolName(tool) : asset.tool}</span>
      <button type="button" title="Remove tool" aria-label={`Remove ${asset.kind} tool`} onClick={() => store.takeToolOff(name)}><Icon name="close" /></button>
    </span>
  )
}

/** Models & LoRAs, the same for every tool with model slots: it opens the one picker. */
function ModelsButton({ name }: { name: string }) {
  const store = useCanvasStore()
  const view = useElement(name)
  const tools = useSignal(store.tools)
  const [open, setOpen] = useState(false)
  if (!slotsOf(tools.get(view?.asset?.tool ?? "")).length) return null
  return (
    <>
      <button type="button" className="image-models" aria-label="Models and LoRAs" onClick={() => setOpen(true)}>Models &amp; LoRAs</button>
      {open && <ModelPicker name={name} onClose={() => setOpen(false)} />}
    </>
  )
}

/** At the bottom of a box's fields: Models & LoRAs, and what's set listed under it. */
export function ModelsAndLoras({ name }: { name: string }) {
  return (
    <div className="models-loras">
      <ModelsButton name={name} />
      <ModelConfig name={name} />
    </div>
  )
}

/** Under a box's media: its tool, and Models & LoRAs beside it when no fields show under it (`models`). */
export function ImageVersions({ name, view, width, models }: { name: string; view: ElementView; width: number; models: boolean }) {
  if (!view.asset?.tool) return null
  return (
    <div className="image-controls nodrag nopan nowheel" style={{ "--image-control-width": `${width}px` } as CSSProperties}>
      <div className="image-version-row">
        <ToolName name={name} />
        {models && <ModelsButton name={name} />}
      </div>
      {models && <ModelConfig name={name} />}
    </div>
  )
}
