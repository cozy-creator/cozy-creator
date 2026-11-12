import { useState } from "react"
import { Icon } from "../ui/Icon"
import { useSignal } from "../ui/signal"
import "./AddOns.css"
import { type AddOn, addOnsOf, carried, takenOff, wordOf } from "./checkpoints"
import { useCanvasStore, useElement } from "./context"
import { madeHere } from "./shown"
import { modelLabel } from "./palette"
import { ModelPicker } from "./ModelPicker"
import { ModelConfig } from "./ModelConfig"

const said = (addOn: AddOn) => `${addOn.lora ? "LoRA" : "Fine-tune"} ${addOn.ref}`

/** One thing on a box: its word, and an X when it can come off. */
export function Chip({ word, title, kind, off, pick }: { word: string; title: string; kind?: "tool" | "fine-tune"; off?: { label: string; take(): void }; pick?: () => void }) {
  return (
    <li className={`add-on ${kind ?? ""}`} title={title} onClick={(event) => event.stopPropagation()}>
      <span className="add-on-word">{word}</span>
      {pick && <button type="button" aria-label="Models and LoRAs" onClick={pick}>Models &amp; LoRAs</button>}
      {off && (
        <button type="button" className="nodrag" aria-label={off.label} title="Take it off" onClick={off.take}>
          <Icon name="close" />
        </button>
      )}
    </li>
  )
}

function useAddOns(name: string) {
  const store = useCanvasStore()
  useElement(name)
  const lora = store.valueOf(name, "lora")
  const models = store.valueOf(name, "models")
  return { held: addOnsOf(lora, models), lora, models, store }
}

/** What makes an open box: its tool, with its X, and its models and LoRAs under it. */
export function Stack({ name }: { name: string }) {
  const [modelsOpen, setModelsOpen] = useState(false)
  const { held, store } = useAddOns(name)
  const view = useElement(name)
  const tools = useSignal(store.tools)
  const tool = view?.asset?.tool
  const known = tool ? tools.get(tool) : undefined
  const made = tool === "cozy/edit" ? "Video Timeline" : tool && !madeHere(tool) ? (known ? modelLabel(known) : (tool.split("/").at(-2) ?? tool)) : null
  if (!made && !held.length) return null
  return (
    <>
    <ul className="add-ons nodrag nopan" aria-label={`What's on ${name}`}>
      {made && <Chip word={made} title={made} kind="tool" pick={known?.models?.length || known?.model ? () => setModelsOpen(true) : undefined} off={{ label: `Take ${made} off ${name}`, take: () => store.takeToolOff(name) }} />}
    </ul>
    {view?.here && <ModelConfig name={name} />}
    {modelsOpen && <ModelPicker name={name} onClose={() => setModelsOpen(false)} />}
    </>
  )
}

/** On a box on the canvas, while something's carried: whether it would take it. Its stack is in Focus: at rest a box says only its tool and
 *  shape under it (cl-932). */
export function AddOns({ name }: { name: string }) {
  const store = useCanvasStore()
  const view = useElement(name)
  const tools = useSignal(store.tools)
  const going = useSignal(carried)
  if (!view?.asset) return null
  return (
    <>
      {going && !going.whyNot(name, view, tools) && <span className="box-takes" />}
    </>
  )
}
