import { useState } from "react"
import { Icon } from "../ui/Icon"
import { useSignal } from "../ui/signal"
import "./addons.css"
import { type AddOn, addOnsOf, atStrength, carried, takenOff, wordOf } from "./checkpoints"
import { useCanvasStore, useElement } from "./context"

const said = (addOn: AddOn) => `${addOn.lora ? "LoRA" : "Fine-tune"} ${addOn.ref}`

/** One thing on a box: its word, and an X when it can come off. */
export function Chip({ word, title, strong, off }: { word: string; title: string; strong?: boolean; off?: { label: string; take(): void } }) {
  return (
    <li className={`add-on ${strong ? "fine-tune" : ""}`} title={title} onClick={(event) => event.stopPropagation()}>
      <span className="add-on-word">{word}</span>
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

/** Beside a box, on the canvas: what's on it, each with its X, and while a checkpoint is carried whether it would take it. It follows the box, and is no part of its size. */
export function AddOns({ name }: { name: string }) {
  const { held, lora, models, store } = useAddOns(name)
  const view = useElement(name)
  const tools = useSignal(store.tools)
  const going = useSignal(carried)
  if (!view?.asset) return null
  return (
    <>
      {going && !going.whyNot(name, view, tools) && <span className="takes" />}
      {held.length > 0 && (
        <ul className="add-ons nodrag nopan" aria-label={`What's on ${name}`}>
          {held.map((addOn) => (
            <Chip
              key={`${addOn.slot}:${addOn.ref}`}
              word={wordOf(addOn.ref)}
              title={said(addOn)}
              strong={!addOn.lora}
              off={{ label: `Take ${wordOf(addOn.ref)} off ${name}`, take: () => store.putOn(name, takenOff(addOn, lora, models)) }}
            />
          ))}
        </ul>
      )}
    </>
  )
}

/** Behind More: each add-on, and how strong a LoRA is. */
function AddOnRow({ name, addOn }: { name: string; addOn: AddOn }) {
  const { lora, models, store } = useAddOns(name)
  const [moving, setMoving] = useState<number | null>(null)
  const shown = moving ?? addOn.strength
  const off = (
    <button type="button" className="iconb nodrag" aria-label={`Take ${wordOf(addOn.ref)} off ${name}`} title="Take it off" onClick={() => store.putOn(name, takenOff(addOn, lora, models))}>
      <Icon name="close" />
    </button>
  )
  if (!addOn.lora) {
    return (
      <div className="slide held-line" title={said(addOn)}>
        <span className="ctl-name">Fine-tune</span>
        <span className="held-value">{wordOf(addOn.ref)}</span>
        {off}
      </div>
    )
  }
  const done = () => {
    if (moving !== null && moving !== addOn.strength) store.putOn(name, atStrength(addOn, moving, lora, models))
    setMoving(null)
  }
  return (
    <div className="add-on-row">
      <label className="slide" title={said(addOn)}>
        <span className="ctl-name">{wordOf(addOn.ref)}</span>
        <input className="nodrag nopan" type="range" aria-label={`Strength of ${wordOf(addOn.ref)}`} min={0} max={2} step={0.05} value={shown} onChange={(event) => setMoving(Number(event.target.value))} onPointerUp={done} onKeyUp={done} />
        <span>{shown}</span>
      </label>
      {off}
    </div>
  )
}

export function AddOnRows({ name }: { name: string }) {
  const { held } = useAddOns(name)
  return held.map((addOn) => <AddOnRow key={`${addOn.slot}:${addOn.ref}`} name={name} addOn={addOn} />)
}
