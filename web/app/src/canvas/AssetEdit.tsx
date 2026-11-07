import { type ReactNode, useEffect, useRef, useState } from "react"
import type { Control, Tool } from "../types"
import { Icon } from "../ui/Icon"
import { useSignal } from "../ui/signal"
import { AddOnRows } from "./AddOns"
import { addOnsOf } from "./checkpoints"
import type { ElementView } from "./CanvasStore"
import { useCanvasStore } from "./context"
import { controlsOf, fieldWords, fullRows, heldBeyond, newRow, shownControls, VARIANT, variantOf } from "./controls"
import { Ghosts } from "./Media"
import { MentionText } from "./MentionText"
import { makes } from "./shown"
import { mentionsIn, type Row, rowsIn } from "./text"
import { ToolBox } from "./ToolBox"

const said = (value: unknown): string => (typeof value === "string" ? value : "")

function TextControl({ name, control, view, focus, named, framed }: { name: string; control: Control; view: ElementView; focus: boolean; named: boolean; framed?: boolean }) {
  const store = useCanvasStore()
  const text = (
    <MentionText
      asset={name}
      place={control.field}
      value={said(store.valueOf(name, control.field))}
      label={control.label}
      placeholder={control.placeholder}
      missing={view.asset?.missing ?? []}
      focus={focus}
      thumbs={!framed}
      onDraft={(words) => store.setDraft(name, control.field, words)}
      onSave={() => void store.save(name)}
    />
  )
  if (framed) return text
  if (!named)
    return (
      <div className="field-block">
        <span className="block-label">{control.label}</span>
        {text}
      </div>
    )
  return (
    <div className="slide text-row">
      <span className="ctl-name">{control.label}</span>
      <div className="text-box">{text}</div>
    </div>
  )
}

/** One member of a group's row, drawn by what it is: a text can hold mentions, and the rest change as they go. */
function RowMember({ name, member, place, label, row, set, save, view }: { name: string; member: Control; place: string; label: string; row: Row; set(row: Row): void; save(): void; view: ElementView }) {
  const value = row[member.field] ?? member.default
  const change = (next: unknown) => set({ ...row, [member.field]: next })
  switch (member.type) {
    case "range": {
      const most = Math.max(member.max ?? 0, Number(value) || 0)
      return (
        <label className="slide">
          <input className="nodrag nopan" type="range" aria-label={label} min={member.min} max={most} step={member.step ?? "any"} value={Number(value) || 0} onChange={(event) => change(Number(event.target.value))} onPointerUp={save} onKeyUp={save} />
          <span>{member.unit ? `${String(value)} ${member.unit}` : String(value)}</span>
        </label>
      )
    }
    case "toggle":
      return (
        <label className="slide">
          <input className="nodrag" type="checkbox" aria-label={label} checked={value === true} onChange={(event) => { change(event.target.checked); save() }} />
          <span className="ctl-name">{member.label}</span>
        </label>
      )
    case "choice":
      return (
        <select className="field nodrag" aria-label={label} value={(member.options ?? []).findIndex((option) => JSON.stringify(option) === JSON.stringify(value))} onChange={(event) => { change((member.options ?? [])[Number(event.target.value)]); save() }}>
          {(member.options ?? []).map((option, i) => (
            <option key={i} value={i}>
              {String(option)}
            </option>
          ))}
        </select>
      )
    default:
      return <MentionText asset={name} place={place} value={said(value)} label={label} placeholder={member.placeholder} missing={view.asset?.missing ?? []} onDraft={change} onSave={save} />
  }
}

/** A list of rows, each its members drawn by these same rules, with add and remove down to its least: a file holding fewer shows them, to write in, and holds them once one's touched. */
function GroupControl({ name, control, view }: { name: string; control: Control; view: ElementView }) {
  const store = useCanvasStore()
  const rows = fullRows(control, rowsIn(store.valueOf(name, control.field)))
  const members = control.members ?? []
  const called = control.label.replace(/s$/, "")
  const set = (next: Row[]) => store.setDraft(name, control.field, next)
  const save = () => void store.save(name)
  const total = members.find((member) => member.total)
  const sum = total && rows.reduce((all, row) => all + (Number(row[total.field] ?? total.default) || 0), 0)
  const least = control.min_items ?? 0
  const add = (rows.length < (control.max_items ?? Infinity)) && (
    <button type="button" className="add-beat nodrag" title={`Add a ${called.toLowerCase()}`} onClick={() => set([...rows, newRow(control)])}>
      + {called}
    </button>
  )
  return (
    <div className="beats-edit" role="group" aria-label={control.label}>
      {rows.map((row, i) => {
        const each = (member: Control) => (
          <RowMember
            key={member.field}
            name={name}
            member={member}
            place={`${control.field}.${i}.${member.field}`}
            label={member.type === "text" ? `${called} ${i + 1}` : `${called} ${i + 1}'s ${member.label.toLowerCase()}`}
            row={row}
            set={(one) => set(rows.map((other, at) => (at === i ? one : other)))}
            save={save}
            view={view}
          />
        )
        return (
          <div className={`beat ${rows.length > least ? "can-go" : ""}`} key={i}>
            {members.filter((member) => member.type === "text").map(each)}
            <div className="beat-time">
              {members.filter((member) => member.type !== "text").map(each)}
            </div>
            {rows.length > least && (
              <button type="button" className="iconb nodrag" aria-label={`Remove ${called.toLowerCase()} ${i + 1}`} onClick={() => {
                set(rows.filter((_, at) => at !== i))
                save()
              }}>
                <Icon name="close" />
              </button>
            )}
          </div>
        )
      })}
      <div className="beats-last">
        {add}
        {total && rows.length > 1 && <span>{sum} {total.unit ?? ""} in all</span>}
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

/** A toggle like any, flipping which of its package's two functions makes the box. Everything else it holds stays. */
function VariantControl({ name, control, view }: { name: string; control: Control; view: ElementView }) {
  const store = useCanvasStore()
  const tools = useSignal(store.tools)
  const variant = variantOf(tools.get(view.asset?.tool ?? ""), tools)
  if (!variant) return null
  return (
    <label className="slide">
      <input className="nodrag" type="checkbox" aria-label={control.label} checked={variant.on} onChange={() => store.setTool(name, variant.other)} />
      <span className="ctl-name">{control.label}</span>
    </label>
  )
}

export function ControlRow({ name, control, view, focus, named }: { name: string; control: Control; view: ElementView; focus: boolean; named: boolean }) {
  switch (control.type) {
    case "text":
      return <TextControl name={name} control={control} view={view} focus={focus} named={named} />
    case "group":
      return <GroupControl name={name} control={control} view={view} />
    case "range":
      return <RangeControl name={name} control={control} />
    case "choice":
      return <ChoiceControl name={name} control={control} />
    case "toggle":
      return control.field === VARIANT ? <VariantControl name={name} control={control} view={view} /> : <ToggleControl name={name} control={control} />
    case "file":
      return <FileControl name={name} control={control} view={view} />
    default:
      return null
  }
}

/** A text asset: only its words, which whatever mentions it reads. It has nothing to render, no takes and nothing to approve. */
export function TextAsset({ name, view }: { name: string; view: ElementView }) {
  const store = useCanvasStore()
  return (
    <div className="in-frame">
      <MentionText
        asset={name}
        place="text"
        value={said(store.valueOf(name, "text"))}
        label="Text"
        missing={view.asset?.missing ?? []}
        focus={store.focusing === name}
        onDraft={(words) => store.setDraft(name, "text", words)}
        onSave={() => void store.save(name)}
      />
      {view.conflict && <Conflict name={name} />}
    </div>
  )
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
  // Render while it's making queues one more: each take in the row has its own ×.
  return (
    <button
      type="button"
      className="run nodrag"
      aria-label="Render"
      title={view.asset?.pinned ? `Make another ${view.asset.noun} to look at: what's approved stays` : "Render"}
      onClick={() => store.run(name)}
    >
      <Icon name="play" />
      Render
    </button>
  )
}

/** What goes in the frame while nothing's made to fill it, so you write on what you're making: its words, else the slot for its file. A tool with its own box draws all of that itself. */
function framed(view: ElementView, controls: Control[], tool: Tool | undefined): Control | undefined {
  if (!view.hereWords || tool?.ui) return undefined
  return controls.find((control) => control.type === "text") ?? controls.find((control) => control.type === "file")
}

export function InFrame({ name, view }: { name: string; view: ElementView }) {
  const store = useCanvasStore()
  const tools = useSignal(store.tools)
  const control = view.asset && framed(view, controlsOf(view.asset, tools), view.asset.tool ? tools.get(view.asset.tool) : undefined)
  if (!control) return null
  const focus = store.focusing === name
  return (
    <div className="in-frame">
      <Ghosts name={name} view={view} />
      {control.type === "text" ? <TextControl name={name} control={control} view={view} focus={focus} named={false} framed /> : <ControlRow name={name} control={control} view={view} focus={focus} named={false} />}
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

/** What beside Render is only a thing to set: the rest of what a box shows unasked wants a line. */
const set = (control: Control) => control.type === "toggle" || control.type === "range" || control.type === "choice"

/** What a box asks and holds, under it. On its words: what a render can't go without and Render, each drawn from its tool's controls or by its package. On a take: what's done with that. Everything else is under More, if there's anything. */
export function AssetEdit({ name, view, editing, head, actions, children }: { name: string; view: ElementView; editing: boolean; head?: ReactNode; actions: ReactNode; children?: ReactNode }) {
  const store = useCanvasStore()
  const tools = useSignal(store.tools)
  if (!view.asset) return null
  const tool = view.asset.tool ? tools.get(view.asset.tool) : undefined
  const controls = makes(view) ? controlsOf(view.asset, tools) : []
  const inFrame = framed(view, controls, tool)
  const first = controls.find((control) => control.type === "text")
  // The box's own shape is its shape: its corner sets it.
  const rest = controls.filter((control) => control !== inFrame && control.type !== "timeline" && control.type !== "shape")
  const asked = editing ? shownControls(controls) : []
  const beyond = heldBeyond({ ...view.asset.fields, ...view.sent, ...view.drafts }, controls)
  const folded = rest.filter((control) => !asked.includes(control))
  const open = store.showsMore(name)
  const addOns = addOnsOf(store.valueOf(name, "lora"), store.valueOf(name, "models")).length > 0
  const shown = rest.filter((control) => asked.includes(control))
  return (
    <div className="edit-face">
      {head && <div className="tool-head">{head}</div>}
      <ToolBox
        tool={tool}
        name={name}
        view={view}
        shown={shown.filter((control) => !set(control))}
        folded={folded}
        row={(control) => <ControlRow key={control.field} name={name} control={control} view={view} focus={store.focusing === name && control === first} named={folded.includes(control)} />}
      >
        {(lines, more) => {
          const behind = (more !== null && folded.length > 0) || addOns || beyond.length > 0
          return (
            <>
              {editing && lines && <div className="edit-shown">{lines}</div>}
              <div className="row actions">
                {more !== null && shown.filter(set).map((control) => <ControlRow key={control.field} name={name} control={control} view={view} focus={false} named={false} />)}
                {actions}
                {behind && (
                  <button type="button" className="more-toggle nodrag" aria-expanded={open} onClick={() => store.setMore(name, !open)}>
                    <span className="more-word">{open ? "Less" : "More"}</span>
                  </button>
                )}
              </div>
              {behind ? (
                open && (
                  <div className="edit-more">
                    {open && more}
                    {open && <AddOnRows name={name} />}
                    {open && beyond.map(([field, value]) => <HeldLine key={field} field={field} value={value} />)}
                    {children}
                  </div>
                )
              ) : (
                children
              )}
            </>
          )
        }}
      </ToolBox>
      {view.conflict && <Conflict name={name} />}
    </div>
  )
}
