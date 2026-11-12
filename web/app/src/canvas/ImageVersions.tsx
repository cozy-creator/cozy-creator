import type { ElementView } from "./CanvasStore"
import { Icon } from "../ui/Icon"
import { useSignal } from "../ui/signal"
import { useCanvasStore } from "./context"
import { shownOf, toolName } from "./shown"
import { numbersOf } from "./versions"

/** One current image, its removable tool, and its retained numbered states. */
export function ImageVersions({ name, view }: { name: string; view: ElementView }) {
  const store = useCanvasStore()
  const tools = useSignal(store.tools)
  const asset = view.asset
  if (!asset || asset.kind !== "image") return null
  const numbers = numbersOf(asset).sort((a, b) => a - b)
  const current = shownOf(view)?.number ?? asset.version ?? numbers[0] ?? 0
  const at = numbers.indexOf(current)
  const tool = asset.tool ? tools.get(asset.tool) : undefined
  const word = tool ? toolName(tool) : asset.tool
  return (
    <div className="image-controls nodrag nopan nowheel">
      <div className="image-version-row">
        {word && <span className="image-tool">
          <button type="button" title={word} aria-label="Configure image tool" onClick={() => store.openHere(name)}>{word}</button>
          <button type="button" title="Remove tool" aria-label="Remove image tool" onClick={() => store.takeToolOff(name)}><Icon name="close" /></button>
        </span>}
        <span className="image-navigation">
          <button type="button" aria-label="Previous version" title="Previous version" disabled={at <= 0} onClick={() => store.selectVersion(name, numbers[at - 1])}><Icon name="back" /></button>
          <button type="button" className="image-history-toggle" aria-label="Image version history" aria-expanded={Boolean(view.imageHistory)} title="Show or hide versions" onClick={() => {
            if (!view.here) store.openHere(name)
            store.patch(name, { imageHistory: !view.imageHistory })
          }}><output aria-label="Current version">{current}/{numbers.at(-1) ?? 0}</output><span aria-hidden>▾</span></button>
          <button type="button" aria-label="Next version" title="Next version" disabled={at < 0 || at >= numbers.length - 1} onClick={() => store.selectVersion(name, numbers[at + 1])}><Icon name="forward" /></button>
        </span>
        {asset.tool && <button type="button" className="image-edit" aria-label={view.hereWords ? "View image" : "Edit image"} onClick={() => view.hereWords ? store.finishImageEdit(name) : store.beginImageEdit(name)}>{view.hereWords ? "View" : "Edit"}</button>}
      </div>
    </div>
  )
}
