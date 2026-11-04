import { useEffect, useRef, useState } from "react"
import type { Control } from "../types"
import { useSignal } from "../ui/signal"
import type { ElementView } from "./CanvasStore"
import { useCanvasStore } from "./context"
import { controlsOf, shapesOf } from "./controls"
import { MentionText } from "./MentionText"
import { BEAT_S, mentionsIn } from "./text"

type Beat = Record<string, unknown>

const beatsIn = (value: unknown): Beat[] => (Array.isArray(value) ? value.filter((beat): beat is Beat => typeof beat === "object" && beat !== null) : [])
const said = (value: unknown): string => (typeof value === "string" ? value : "")

function TextControl({ name, control, view, focus }: { name: string; control: Control; view: ElementView; focus: boolean }) {
  const store = useCanvasStore()
  return (
    <MentionText
      asset={name}
      place={control.field}
      value={said(store.valueOf(name, control.field))}
      label={control.label}
      missing={view.asset?.missing ?? []}
      focus={focus}
      onDraft={(text) => store.setDraft(name, control.field, text)}
      onSave={() => void store.save(name)}
    />
  )
}

/** One text box and a duration slider per beat. The slider's most is the tool's word; a shot is as long as its beats. */
function BeatsControl({ name, control, view }: { name: string; control: Control; view: ElementView }) {
  const store = useCanvasStore()
  const beats = beatsIn(store.valueOf(name, control.field))
  const most = control.max_duration ?? Math.max(30, ...beats.map((beat) => Number(beat.duration) || 0))
  const set = (next: Beat[]) => store.setDraft(name, control.field, next)
  const change = (at: number, beat: Beat) => set(beats.map((held, i) => (i === at ? beat : held)))
  const save = () => void store.save(name)
  const lasts = beats.reduce((sum, beat) => sum + (Number(beat.duration) || BEAT_S), 0)
  return (
    <div className="beats-edit" role="group" aria-label={control.label}>
      {beats.map((beat, i) => {
        const duration = Number(beat.duration) || BEAT_S
        return (
          <div className="beat" key={i}>
            <MentionText
              asset={name}
              place={`${control.field}.${i}.description`}
              value={said(beat.description)}
              label={`Beat ${i + 1}`}
              missing={view.asset?.missing ?? []}
              onDraft={(text) => change(i, { ...beat, description: text })}
              onSave={save}
            />
            <label className="slide">
              <input
                className="nodrag nopan"
                type="range"
                aria-label={`Beat ${i + 1}'s seconds`}
                min={1}
                max={most}
                step={1}
                value={Math.min(duration, most)}
                onChange={(event) => change(i, { ...beat, duration: Number(event.target.value) })}
                onPointerUp={save}
                onKeyUp={save}
              />
              <span>{duration} s</span>
            </label>
            <button type="button" className="iconb nodrag" aria-label={`Remove beat ${i + 1}`} onClick={() => {
              set(beats.filter((_, at) => at !== i))
              save()
            }}>
              ×
            </button>
          </div>
        )
      })}
      <div className="row">
        <button type="button" className="act nodrag" onClick={() => {
          set([...beats, { duration: Math.min(BEAT_S, most), description: "" }])
        }}>
          Add a beat
        </button>
        <span className="sp" />
        {beats.length > 0 && <span className="muted">{lasts} s</span>}
      </div>
    </div>
  )
}

/** A number that goes on being moved until it's let go, and only then is written. */
function RangeControl({ name, control }: { name: string; control: Control }) {
  const store = useCanvasStore()
  const held = store.valueOf(name, control.field)
  const value = typeof held === "number" ? held : typeof control.default === "number" ? control.default : (control.min ?? 0)
  const [moving, setMoving] = useState<number | null>(null)
  const shown = moving ?? value
  const done = () => {
    if (moving !== null && moving !== value) void store.setField(name, control.field, moving)
    setMoving(null)
  }
  return (
    <label className="slide" title={control.label}>
      <span className="ctl-name">{control.label}</span>
      <input
        className="nodrag nopan"
        type="range"
        aria-label={control.label}
        min={control.min}
        max={control.max}
        step={control.step ?? "any"}
        value={shown}
        onChange={(event) => setMoving(Number(event.target.value))}
        onPointerUp={done}
        onKeyUp={done}
      />
      <span>{shown}</span>
    </label>
  )
}

function ChoiceControl({ name, control }: { name: string; control: Control }) {
  const store = useCanvasStore()
  const options = control.options ?? []
  const held = store.valueOf(name, control.field) ?? control.default
  return (
    <label className="slide">
      <span className="ctl-name">{control.label}</span>
      <select
        className="field nodrag"
        aria-label={control.label}
        value={options.findIndex((option) => JSON.stringify(option) === JSON.stringify(held))}
        onChange={(event) => void store.setField(name, control.field, options[Number(event.target.value)])}
      >
        <option value={-1} disabled hidden>
          Its default
        </option>
        {options.map((option, i) => (
          <option key={i} value={i}>
            {String(option)}
          </option>
        ))}
      </select>
    </label>
  )
}

function ToggleControl({ name, control }: { name: string; control: Control }) {
  const store = useCanvasStore()
  const held = store.valueOf(name, control.field) ?? control.default
  return (
    <label className="slide">
      <input className="nodrag" type="checkbox" checked={held === true} onChange={(event) => void store.setField(name, control.field, event.target.checked)} />
      <span className="ctl-name">{control.label}</span>
    </label>
  )
}

/** The shape it will be: the box's corner reshapes it, and this says which it is. */
function ShapeControl({ name, control, view }: { name: string; control: Control; view: ElementView }) {
  const store = useCanvasStore()
  const held = said(view.shape ?? store.valueOf(name, control.field)) || said(control.default)
  return (
    <label className="slide">
      <span className="ctl-name">{control.label}</span>
      <select className="field nodrag" aria-label={control.label} value={held} onChange={(event) => void store.setField(name, control.field, event.target.value)}>
        {!held && <option value="">Its default</option>}
        {[...new Set([...shapesOf(control), ...(held ? [held] : [])])].map((shape) => (
          <option key={shape} value={shape}>
            {shape}
          </option>
        ))}
      </select>
    </label>
  )
}

/** A slot you drag an element into: it holds `<name>`, or with `multiple` several, in order. */
function FileControl({ name, control, view }: { name: string; control: Control; view: ElementView }) {
  const store = useCanvasStore()
  const slot = useRef<HTMLDivElement>(null)
  const value = store.valueOf(name, control.field)
  const held = (Array.isArray(value) ? value : [value]).flatMap((item) => (typeof item === "string" ? mentionsIn(item) : []))
  const write = (names: string[]) => {
    const said = names.map((one) => `<${one}>`)
    void store.setField(name, control.field, control.multiple ? said : (said[0] ?? null))
  }
  useEffect(() => {
    const el = slot.current
    if (!el) return
    store.dropTargets.set(el, {
      over: () => {},
      drop: (names) => {
        const fresh = names.filter((one) => one !== name)
        write(control.multiple ? [...held, ...fresh] : fresh.slice(-1))
      },
    })
    return () => void store.dropTargets.delete(el)
  })
  const missing = view.asset?.missing ?? []
  return (
    <div className="slot-control" ref={slot} data-drop-target="" role="group" aria-label={control.label}>
      <span className="ctl-name">{control.label}</span>
      {held.map((one) => (
        <span key={one} className={`chip ${missing.includes(one) ? "missing" : ""}`} title={one}>
          {one}
          <button type="button" className="nodrag" aria-label={`Take out ${one}`} onClick={() => write(held.filter((other) => other !== one))}>
            ×
          </button>
        </span>
      ))}
      {!held.length && <span className="muted">Drag {control.accept ? `a ${control.accept}` : "something"} here</span>}
    </div>
  )
}

function ControlRow({ name, control, view, focus }: { name: string; control: Control; view: ElementView; focus: boolean }) {
  switch (control.type) {
    case "text":
      return <TextControl name={name} control={control} view={view} focus={focus} />
    case "beats":
      return <BeatsControl name={name} control={control} view={view} />
    case "range":
      return <RangeControl name={name} control={control} />
    case "choice":
      return <ChoiceControl name={name} control={control} />
    case "toggle":
      return <ToggleControl name={name} control={control} />
    case "shape":
      return <ShapeControl name={name} control={control} view={view} />
    case "file":
      return <FileControl name={name} control={control} view={view} />
    default:
      return null
  }
}

/** The file changed under what's typed. */
function Conflict({ name }: { name: string }) {
  const store = useCanvasStore()
  return (
    <div className="status ood">
      The file changed under this{" "}
      <button type="button" className="act nodrag" onClick={() => void store.keepMine(name)}>
        Keep mine
      </button>{" "}
      <button type="button" className="act nodrag" onClick={() => store.takeTheFiles(name)}>
        Take the file's
      </button>
    </div>
  )
}

/** Render. How far it's got is on the picture. On an approved box it makes one more to look at, and what's approved stays. */
function RenderButton({ name, view }: { name: string; view: ElementView }) {
  const store = useCanvasStore()
  // An Edit's film is only a moment's work here, and has no run to cancel.
  if (view.asset?.rendering !== undefined) return null
  if (view.asset?.standing === "making") {
    return (
      <button type="button" className="act nodrag" onClick={() => void store.cancelRender(view.asset?.name ?? name)}>
        Cancel
      </button>
    )
  }
  return (
    <button type="button" className="run nodrag" aria-label="Render" title={view.asset?.pinned ? `Make another ${view.asset.noun} to look at: what's approved stays` : "Render"} onClick={() => void store.run(name)}>
      ▶ Render
    </button>
  )
}

/** The face a box opens on: whatever its tool's controls ask for, and Render. It knows no function. */
export function AssetEdit({ name, view }: { name: string; view: ElementView }) {
  const store = useCanvasStore()
  const tools = useSignal(store.tools)
  const [focusing] = useState(() => {
    const here = store.focusing === name
    if (here) store.focusing = null
    return here
  })
  if (!view.asset) return null
  const controls = controlsOf(view.asset, tools)
  const first = controls.find((control) => control.type === "text")
  return (
    <div className="edit-face">
      {controls.map((control) => (
        <ControlRow key={control.field} name={name} control={control} view={view} focus={focusing && control === first} />
      ))}
      {view.conflict && <Conflict name={name} />}
      <div className="row foot">
        <RenderButton name={name} view={view} />
        {view.drafts && (
          <button type="button" className="act nodrag" onClick={() => void store.save(name)}>
            Save
          </button>
        )}
      </div>
    </div>
  )
}
