import { type ReactNode, useEffect, useRef, useState } from "react"
import type { Control, Tool } from "../types"
import { Icon } from "../ui/Icon"
import { useSignal } from "../ui/signal"
import type { ElementView } from "./CanvasStore"
import { useCanvasStore } from "./context"
import { controlsOf, fieldWords, heldBeyond, markOf, newBeat, shownControls } from "./controls"
import { MentionText } from "./MentionText"
import { faceOf, shownOf } from "./shown"
import { BEAT_S, mentionsIn } from "./text"
import { ToolBox } from "./ToolBox"

type Beat = Record<string, unknown>

const beatsIn = (value: unknown): Beat[] => (Array.isArray(value) ? value.filter((beat): beat is Beat => typeof beat === "object" && beat !== null) : [])
const said = (value: unknown): string => (typeof value === "string" ? value : "")

function TextControl({ name, control, view, focus, named }: { name: string; control: Control; view: ElementView; focus: boolean; named: boolean }) {
  const store = useCanvasStore()
  const text = (
    <MentionText
      asset={name}
      place={control.field}
      value={said(store.valueOf(name, control.field))}
      label={control.label}
      missing={view.asset?.missing ?? []}
      focus={focus}
      onDraft={(words) => store.setDraft(name, control.field, words)}
      onSave={() => void store.save(name)}
    />
  )
  if (!named) return text
  return (
    <div className="slide text-row">
      <span className="ctl-name">{control.label}</span>
      <div className="text-box">{text}</div>
    </div>
  )
}

/** One text box and a duration slider per beat, and always one: a file with none shows one to write in, which it holds once it's touched, and until then it's as long as its tool makes it. The slider's most is the tool's word; a shot is as long as its beats. */
function BeatsControl({ name, control, view }: { name: string; control: Control; view: ElementView }) {
  const store = useCanvasStore()
  const held = beatsIn(store.valueOf(name, control.field))
  const beats: Beat[] = held.length ? held : [newBeat(control)]
  const most = control.max_duration ?? Math.max(30, ...beats.map((beat) => Number(beat.duration) || 0))
  const set = (next: Beat[]) => store.setDraft(name, control.field, next)
  const change = (at: number, beat: Beat) => set(beats.map((one, i) => (i === at ? beat : one)))
  const save = () => void store.save(name)
  const lasts = beats.reduce((sum, beat) => sum + (Number(beat.duration) || BEAT_S), 0)
  const add = (
    <button type="button" className="add-beat nodrag" title="Add a beat" onClick={() => set([...beats, newBeat(control)])}>
      + Beat
    </button>
  )
  return (
    <div className="beats-edit" role="group" aria-label={control.label}>
      {beats.map((beat, i) => {
        const duration = Number(beat.duration) || BEAT_S
        return (
          <div className={`beat ${beats.length > 1 ? "can-go" : ""}`} key={i}>
            <MentionText
              asset={name}
              place={`${control.field}.${i}.description`}
              value={said(beat.description)}
              label={`Beat ${i + 1}`}
              missing={view.asset?.missing ?? []}
              onDraft={(text) => change(i, { ...beat, description: text })}
              onSave={save}
            />
            <div className="beat-time">
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
                <span>{held.length ? `${duration} s` : "Auto"}</span>
              </label>
              {beats.length === 1 && add}
            </div>
            {beats.length > 1 && (
              <button type="button" className="iconb nodrag" aria-label={`Remove beat ${i + 1}`} onClick={() => {
                set(beats.filter((_, at) => at !== i))
                save()
              }}>
                <Icon name="close" />
              </button>
            )}
          </div>
        )
      })}
      {beats.length > 1 && (
        <div className="beats-last">
          <span>{lasts} s</span>
          {add}
        </div>
      )}
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
          Auto
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

const WANTED: Record<string, string> = { image: "a picture", video: "a video", audio: "a sound" }

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
      {!held.length && <span className="muted">Drag {WANTED[control.accept ?? ""] ?? "something"} here</span>}
    </div>
  )
}

/** A shape has no row: the box's corner is its control, and the box says which it is. */
function ControlRow({ name, control, view, focus, named }: { name: string; control: Control; view: ElementView; focus: boolean; named: boolean }) {
  switch (control.type) {
    case "text":
      return <TextControl name={name} control={control} view={view} focus={focus} named={named} />
    case "beats":
      return <BeatsControl name={name} control={control} view={view} />
    case "range":
      return <RangeControl name={name} control={control} />
    case "choice":
      return <ChoiceControl name={name} control={control} />
    case "toggle":
      return <ToggleControl name={name} control={control} />
    case "file":
      return <FileControl name={name} control={control} view={view} />
    default:
      return null
  }
}

/** The file changed under what's typed. */
export function Conflict({ name }: { name: string }) {
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
export function RenderButton({ name, view }: { name: string; view: ElementView }) {
  const store = useCanvasStore()
  // An Edit's film is only a moment's work here, and has no run to cancel.
  if (view.asset?.rendering !== undefined) return null
  if (view.asset?.standing === "making") {
    return (
      <button type="button" className="act nodrag" onClick={() => store.cancelRender(name)}>
        Cancel
      </button>
    )
  }
  return (
    <button
      type="button"
      className="run nodrag"
      aria-label="Render"
      title={view.asset?.pinned ? `Make another ${view.asset.noun} to look at: what's approved stays` : "Render"}
      onClick={() => {
        store.editing.set(null)
        store.run(name)
      }}
    >
      <Icon name="play" />
      Render
    </button>
  )
}

/** What goes in the frame while nothing's made to fill it, so you write on what you're making: its words, else the slot for its file. A tool with its own box draws all of that itself. */
function framed(view: ElementView, controls: Control[], tool: Tool | undefined): Control | undefined {
  if (shownOf(view) || faceOf(view) !== "edit" || tool?.ui) return undefined
  return controls.find((control) => control.type === "text") ?? controls.find((control) => control.type === "file")
}

export function InFrame({ name, view }: { name: string; view: ElementView }) {
  const store = useCanvasStore()
  const tools = useSignal(store.tools)
  const control = view.asset && framed(view, controlsOf(view.asset, tools), view.asset.tool ? tools.get(view.asset.tool) : undefined)
  if (!control) return null
  return (
    <div className="in-frame">
      <ControlRow name={name} control={control} view={view} focus={store.focusing === name} named={false} />
    </div>
  )
}

const plain = (value: unknown): string => (typeof value === "object" && value !== null ? Object.values(value).map(plain).join(" ") : String(value))

/** What it holds that no control draws: only to see. */
function HeldLine({ field, value }: { field: string; value: unknown }) {
  const text = plain(value)
  return (
    <div className="slide held-line" title={text}>
      <span className="ctl-name">{fieldWords(field)}</span>
      <span className="held-value">{text}</span>
    </div>
  )
}

/** The face a box opens on: its words and whatever a render can't go without, then Render, then More for all else, each drawn from its tool's controls or by its package. It knows no function. */
export function AssetEdit({ name, view, actions, children }: { name: string; view: ElementView; actions: ReactNode; children: ReactNode }) {
  const store = useCanvasStore()
  const tools = useSignal(store.tools)
  if (!view.asset) return null
  const tool = view.asset.tool ? tools.get(view.asset.tool) : undefined
  const controls = controlsOf(view.asset, tools)
  const inFrame = framed(view, controls, tool)
  const first = controls.find((control) => control.type === "text")
  const rest = controls.filter((control) => control !== inFrame && control.type !== "timeline")
  const asked = shownControls(controls)
  const beyond = heldBeyond({ ...view.asset.fields, ...view.sent, ...view.drafts }, controls)
  const folded = rest.filter((control) => !asked.includes(control))
  const described = store.valueOf(name, "description")
  const marksOf = (drawn: boolean) => [
    ...(drawn ? folded.flatMap((control) => markOf(control, store.valueOf(name, control.field)) ?? []) : []),
    ...beyond.map(([field]) => fieldWords(field)),
    ...(typeof described === "string" && described ? ["Description"] : []),
  ]
  const open = store.showsMore(name)
  // A caption is typed into its field, and shows until it's kept or dropped.
  const typing = view.drafts !== undefined && "description" in view.drafts
  return (
    <div className="edit-face">
      <ToolBox
        tool={tool}
        name={name}
        view={view}
        shown={rest.filter((control) => asked.includes(control))}
        folded={folded}
        row={(control) => <ControlRow key={control.field} name={name} control={control} view={view} focus={store.focusing === name && control === first} named={folded.includes(control)} />}
      >
        {(shown, more) => {
          // A package's whole face holds what would be behind More, so nothing's hidden there.
          const marks = marksOf(more !== null)
          return (
            <>
              {shown && <div className="edit-shown">{shown}</div>}
              <div className="row actions">
                {actions}
                <button type="button" className="more-toggle nodrag" aria-expanded={open} title={marks.join(", ") || undefined} onClick={() => store.setMore(name, !open)}>
                  {!open && marks.length > 0 && <span className="more-marks">{marks.join(", ")}</span>}
                  <span className="more-word">{open ? "Less" : "More"}</span>
                </button>
              </div>
              {(open || typing) && (
                <div className="edit-more">
                  {open && more}
                  {open && beyond.map(([field, value]) => <HeldLine key={field} field={field} value={value} />)}
                  {children}
                </div>
              )}
            </>
          )
        }}
      </ToolBox>
      {view.conflict && <Conflict name={name} />}
    </div>
  )
}
