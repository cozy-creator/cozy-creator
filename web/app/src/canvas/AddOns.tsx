import { useSignal } from "../ui/signal"
import "./AddOns.css"
import { carried } from "./checkpoints"
import { useCanvasStore, useElement } from "./context"

/** On a box on the canvas, while something's carried: whether it would take it. What's on a box is in Models & LoRAs (cl-934); at rest a box
 *  says only its tool and shape under it (cl-932). */
export function AddOns({ name }: { name: string }) {
  const store = useCanvasStore()
  const view = useElement(name)
  const tools = useSignal(store.tools)
  const going = useSignal(carried)
  if (!view?.asset) return null
  return going && !going.whyNot(name, view, tools) ? <span className="box-takes" /> : null
}
