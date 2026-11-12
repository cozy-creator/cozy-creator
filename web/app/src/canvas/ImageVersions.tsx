import type { ElementView } from "./CanvasStore"
import { type CSSProperties, useState } from "react"
import { Icon } from "../ui/Icon"
import { useSignal } from "../ui/signal"
import { useCanvasStore } from "./context"
import { modelLabel } from "./palette"
import { ModelPicker } from "./ModelPicker"

/** One current image, its removable tool, and its retained numbered states. */
export function ImageVersions({ name, view, width }: { name: string; view: ElementView; width: number }) {
  const store = useCanvasStore()
  const tools = useSignal(store.tools)
  const [modelsOpen, setModelsOpen] = useState(false)
  const asset = view.asset
  if (!asset || asset.kind !== "image") return null
  const tool = asset.tool ? tools.get(asset.tool) : undefined
  const word = tool ? modelLabel(tool) : asset.tool ? "Image tool" : undefined
  return (
    <div className="image-controls nodrag nopan nowheel" style={{ "--image-control-width": `${width}px` } as CSSProperties}>
      <div className="image-version-row">
        {word && <span className="image-tool">
          <button type="button" title="Choose models and LoRAs" aria-label="Models and LoRAs" onClick={() => setModelsOpen(true)}>{word}</button>
          <button type="button" title="Remove tool" aria-label="Remove image tool" onClick={() => store.takeToolOff(name)}><Icon name="close" /></button>
        </span>}
        {asset.tool && <button type="button" className="image-edit" aria-label={view.hereWords ? "View image" : "Edit image"} onClick={() => view.hereWords ? store.finishImageEdit(name) : store.beginImageEdit(name)}>{view.hereWords ? "View" : "Edit"}</button>}
      </div>
      {modelsOpen && <ModelPicker name={name} onClose={() => setModelsOpen(false)} />}
    </div>
  )
}
