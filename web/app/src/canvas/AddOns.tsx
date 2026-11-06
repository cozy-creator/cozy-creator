import { useLayoutEffect, useRef, useState } from "react"
import { Icon } from "../ui/Icon"
import { useSignal } from "../ui/signal"
import "./AddOns.css"
import { type AddOn, addOnsOf, atStrength, carried, takenOff, wordOf } from "./checkpoints"
import { useCanvasStore, useElement } from "./context"
import { fieldsOf } from "./controls"
import { madeHere } from "./shown"

const said = (addOn: AddOn) => `${addOn.lora ? "LoRA" : "Fine-tune"} ${addOn.ref}`

/** One thing on a box: its word, and an X when it can come off. */
export function Chip({ word, title, kind, off, mark, away }: { word: string; title: string; kind?: "tool" | "fine-tune"; off?: { label: string; take(): void }; mark?: boolean; away?: boolean }) {
  if (mark) return <li className={`add-on ${kind ?? ""} ${away ? "away" : ""}`} title={title} />
  return (
    <li className={`add-on ${kind ?? ""} ${away ? "away" : ""}`} title={title} onClick={(event) => event.stopPropagation()}>
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

/** How many of a row's chips fit in its width beside a "+N", measured as its words grow and shrink with the zoom. */
function useFits(count: number) {
  const row = useRef<HTMLUListElement>(null)
  const [fits, setFits] = useState(count)
  useLayoutEffect(() => {
    const el = row.current
    if (!el) return
    const measure = () => {
      const chips = [...el.querySelectorAll<HTMLElement>(":scope > .add-on:not(.more)")]
      const more = el.querySelector<HTMLElement>(":scope > .more")
      const gap = parseFloat(getComputedStyle(el).columnGap) || 0
      const room = el.parentElement?.clientWidth ?? el.clientWidth
      let used = 0
      let shown = chips.length
      for (const [at, chip] of chips.entries()) {
        const left = chips.length - at - 1
        const next = used + chip.offsetWidth + (at ? gap : 0)
        if (next + (left ? gap + (more?.offsetWidth ?? 0) : 0) > room) {
          shown = at
          break
        }
        used = next
      }
      setFits(shown)
    }
    measure()
    const watch = new ResizeObserver(measure)
    watch.observe(el.parentElement ?? el)
    for (const chip of el.children) watch.observe(chip)
    return () => watch.disconnect()
  }, [count])
  return { row, fits }
}

/** What's on a box, its tool first, each with its X: in one row in its width, as many as fit, then "+N" saying the rest. Far out, each is only a mark. */
export function Stack({ name }: { name: string }) {
  const { held, lora, models, store } = useAddOns(name)
  const view = useElement(name)
  const tools = useSignal(store.tools)
  const mark = useSignal(store.far) && !view?.here
  const tool = view?.asset?.tool
  const made = tool && !madeHere(tool) ? (tools.get(tool)?.label ?? tool.split("/").at(-2) ?? tool) : null
  const chips = [
    ...(made ? [{ key: "tool", word: made, title: tool ?? made, kind: "tool" as const, off: { label: `Take ${made} off ${name}`, take: () => store.takeToolOff(name) } }] : []),
    ...held.map((addOn) => ({
      key: `${addOn.slot}:${addOn.ref}`,
      word: wordOf(addOn.ref),
      title: said(addOn),
      kind: addOn.lora ? undefined : ("fine-tune" as const),
      off: { label: `Take ${wordOf(addOn.ref)} off ${name}`, take: () => store.putOn(name, takenOff(addOn, lora, models)) },
    })),
  ]
  const { row, fits } = useFits(chips.length)
  if (!chips.length) return null
  const rest = chips.slice(fits)
  return (
    <ul className="add-ons nodrag nopan" aria-label={`What's on ${name}`} ref={row}>
      {chips.map((chip, at) => (
        <Chip key={chip.key} word={chip.word} title={chip.title} kind={chip.kind} mark={mark} off={chip.off} away={at >= fits} />
      ))}
      <li className={`add-on more ${rest.length ? "" : "away"}`} title={rest.map((chip) => chip.title).join("\n")}>
        +{rest.length}
      </li>
    </ul>
  )
}

/** The shape it makes, in the words of its name, where there's room. */
function Ratio({ name }: { name: string }) {
  const store = useCanvasStore()
  const view = useElement(name)
  const tools = useSignal(store.tools)
  const field = view?.asset && fieldsOf(view.asset, tools).find((held) => held.type === "shape")
  if (!field || view?.shape || view?.placing) return null
  const held = store.valueOf(name, field.field)
  return <span className="ratio-rest">{typeof held === "string" ? held : "Auto"}</span>
}

/** Inside a box along its foot: its stack at the left and its shape at the right, in one row in its width, so nothing is drawn on another box at any zoom. While something's carried, whether it would take it. Written in where it stands, the stack goes with what's under it. */
export function AddOns({ name }: { name: string }) {
  const store = useCanvasStore()
  const view = useElement(name)
  const tools = useSignal(store.tools)
  const going = useSignal(carried)
  const lite = useSignal(store.far)
  if (!view?.asset) return null
  return (
    <>
      {going && !going.whyNot(name, view, tools) && <span className="box-takes" />}
      {!view.here && (
        <div className="foot">
          <Stack name={name} />
          {!lite && <Ratio name={name} />}
        </div>
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
