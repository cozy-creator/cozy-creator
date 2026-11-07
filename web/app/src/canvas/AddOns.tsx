import { useState } from "react"
import { Icon } from "../ui/Icon"
import { useSignal } from "../ui/signal"
import "./AddOns.css"
import { type AddOn, addOnsOf, atStrength, carried, takenOff, wordOf } from "./checkpoints"
import { useCanvasStore, useElement } from "./context"
import { madeHere, toolName } from "./shown"

const said = (addOn: AddOn) => `${addOn.lora ? "LoRA" : "Fine-tune"} ${addOn.ref}`

/** One thing on a box: its word, and an X when it can come off. */
export function Chip({ word, title, kind, off, mark }: { word: string; title: string; kind?: "tool" | "fine-tune"; off?: { label: string; take(): void }; mark?: boolean }) {
  if (mark) return <li className={`add-on ${kind ?? ""}`} title={title} />
  return (
    <li className={`add-on ${kind ?? ""}`} title={title} onClick={(event) => event.stopPropagation()}>
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

/** What's on a box, its tool first, each with its X: as many as it holds. Far out, each is only a mark. */
export function Stack({ name }: { name: string }) {
  const { held, lora, models, store } = useAddOns(name)
  const view = useElement(name)
  const tools = useSignal(store.tools)
  const mark = useSignal(store.far) && !view?.here
  const tool = view?.asset?.tool
  const known = tool ? tools.get(tool) : undefined
  const made = tool && !madeHere(tool) ? (known ? toolName(known) : (tool.split("/").at(-2) ?? tool)) : null
  if (!made && !held.length) return null
  return (
    <ul className="add-ons nodrag nopan" aria-label={`What's on ${name}`}>
      {made && <Chip word={made} title={tool ?? made} kind="tool" mark={mark} off={{ label: `Take ${made} off ${name}`, take: () => store.takeToolOff(name) }} />}
      {held.map((addOn) => (
        <Chip
          key={`${addOn.slot}:${addOn.ref}`}
          word={wordOf(addOn.ref)}
          title={said(addOn)}
          kind={addOn.lora ? undefined : "fine-tune"}
          mark={mark}
          off={{ label: `Take ${wordOf(addOn.ref)} off ${name}`, take: () => store.putOn(name, takenOff(addOn, lora, models)) }}
        />
      ))}
    </ul>
  )
}

/** Under a box on the canvas, at its left: its stack, and while something's carried whether it would take it. It follows the box, and is no part of its size. Written in where it stands, the stack goes with what's under it. */
export function AddOns({ name }: { name: string }) {
  const store = useCanvasStore()
  const view = useElement(name)
  const tools = useSignal(store.tools)
  const going = useSignal(carried)
  if (!view?.asset) return null
  return (
    <>
      {going && !going.whyNot(name, view, tools) && <span className="box-takes" />}
      {!view.here && <Stack name={name} />}
    </>
  )
}

/** Behind More, and in the left bar of an open box: each add-on, and how strong a LoRA is. */
export function AddOnRow({ name, addOn }: { name: string; addOn: AddOn }) {
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
