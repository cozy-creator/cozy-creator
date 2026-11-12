import type { ElementView } from "./CanvasStore"
import { type CSSProperties, useState } from "react"
import { Icon } from "../ui/Icon"
import { useSignal } from "../ui/signal"
import { useCanvasStore } from "./context"
import { modelLabel } from "./palette"
import { shownOf } from "./shown"
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
  const version = shownOf(view)?.number
  return (
    <div className="image-controls nodrag nopan nowheel" style={{ "--image-control-width": `${width}px` } as CSSProperties}>
      <div className="image-version-row">
        <span className="image-version-label" aria-label="Image version">{version === undefined ? "New" : `v${version}`}</span>
        {word && <span className="image-tool">
          <span className="image-tool-name">{word}</span>
          <button type="button" title="Remove tool" aria-label="Remove image tool" onClick={() => store.takeToolOff(name)}><Icon name="close" /></button>
        </span>}
        {asset.tool && <button type="button" className="image-models" aria-label="Models and LoRAs" onClick={() => setModelsOpen(true)}>Models &amp; LoRAs</button>}
      </div>
      {modelsOpen && <ModelPicker name={name} onClose={() => setModelsOpen(false)} />}
    </div>
  )
}
