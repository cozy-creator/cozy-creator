import { useQuery } from "@tanstack/react-query"
import { createContext, type Dispatch, type DragEvent, type FocusEvent, type PointerEvent, type ReactNode, type SetStateAction, useContext, useEffect, useId, useRef, useState } from "react"
import type { ToolField, Tool } from "../api/calcifer"
import { runFullQuery } from "../api/queries"
import { useConnection } from "../app/Connection"
import type { Media } from "../types"
import { Icon } from "../ui/Icon"
import { say } from "../ui/notes"
import { useSignal } from "../ui/signal"
import { progressOfBeats } from "./beatProgress"
import type { CanvasStore, ElementView } from "./CanvasStore"
import { carriedAt, carriedFile, LET_GO } from "./carry"
import { ClipRange } from "./ClipRange"
import { useCanvasStore, useElement } from "./context"
import { atDefault, fieldsOf, fieldWords, fullRows, heldBeyond, newRow, onFace, optionWord, totalOf, WANTED } from "./controls"
import { Ghosts, Spread, Thumb } from "./Media"
import { MentionText } from "./MentionText"
import { angleOf, baseOf, degrees, FPS, framesIn, rangeOf, ranged, timecode } from "./ranges"
import { makes, watched } from "./shown"
import { mentionsIn, type Row, rowsIn, textsIn } from "./text"
import { ToolBox } from "./ToolBox"
import { WhenOff, whenOff } from "./When"
import "./FocusParameters.css"

/** Where a field's value is, and how it's changed: what's typed waits for a pause, what's set is written at once, and unset takes its key away. */
export interface Bound {
  value: unknown
  draft(next: unknown): void
  write(next: unknown): void
  unset(): void
  save(): void
}

/** A field of the box itself. */
export function topBound(store: CanvasStore, name: string, field: string): Bound {
  return {
    value: store.valueOf(name, field),
    draft: (next) => store.setDraft(name, field, next),
    write: (next) => void store.setField(name, field, next),
    unset: () => void store.setField(name, field, null),
    save: () => void store.save(name),
  }
}

const isRow = (value: unknown): value is Row => typeof value === "object" && value !== null && !Array.isArray(value)

/** A member of a row or a block: written as the whole of what holds it. */
function memberBound(parent: Bound, field: string): Bound {
  const row = isRow(parent.value) ? parent.value : {}
  const without = () => Object.fromEntries(Object.entries(row).filter(([key]) => key !== field))
  return {
    value: row[field],
    draft: (next) => parent.draft({ ...row, [field]: next }),
    write: (next) => parent.write({ ...row, [field]: next }),
    unset: () => parent.write(without()),
    save: parent.save,
  }
}

const said = (value: unknown): string => (typeof value === "string" ? value : "")

/** What words take of a mention: nothing but words. */
const WORDS_ONLY: Media[] = []

interface Drawn {
  name: string
  field: ToolField
  bound: Bound
  view: ElementView
  /** Where it is in the box, as `data-field` says: `beats.0.description`. */
  place: string
  /** What a reader hears it called. */
  label: string
  focus?: boolean
  /** Its name is beside it: under More, or in a block. */
  named?: boolean
  /** A field of the box's own face, under its label. */
  block?: boolean
  /** In the frame, over the pictures it mentions: none of them small under it. */
  framed?: boolean
}

/** Its reset, when it isn't at its default: unset takes its key out of the file. */
function Reset({ field, bound }: { field: ToolField; bound: Bound }) {
  if (atDefault(field, bound.value)) return null
  return (
    <button type="button" className="reset nodrag" aria-label={`Reset ${field.label}`} title="Back to its default" onClick={bound.unset}>
      ↺
    </button>
  )
}

/** Words, or a prompt: chips for what it mentions, and what a prompt can't send is marked, never sent. */
function TextField({ name, field, bound, view, place, label, focus, named, block, framed }: Drawn) {
  const store = useCanvasStore()
  const beat = /^beats\.\d+\./.test(place)
  // In the frame its pictures are the frame's (Backdrop); elsewhere a video's prompt carries its own behind it.
  const inFrame = Boolean(useContext(Sections)) || Boolean(framed)
  const references = !inFrame && view.asset?.kind === "video" && view.hereWords && field.type === "prompt" && (place === "summary" || beat)
  const showLabel = !(references && beat && field.field === "description")
  const text = (
    <MentionText
      asset={name}
      place={place}
      value={said(bound.value)}
      label={label}
      placeholder={field.placeholder ?? undefined}
      missing={view.asset?.missing ?? []}
      takes={field.type === "words" ? WORDS_ONLY : ((field.takes as Media[] | null) ?? undefined)}
      focus={focus}
      thumbs={!inFrame && !references}
      onDraft={bound.draft}
      onSave={bound.save}
      onSubmit={field.type === "prompt" ? () => store.run(name) : undefined}
    />
  )
  const surface = references ? <div className="video-reference-field" data-video-reference={place}>
    <Ghosts name={name} view={view} mentions={mentionsIn(said(bound.value))} />
    {text}
  </div> : text
  if (named)
    return (
      <div className="slide text-row">
        {showLabel && <span className="ctl-name">{field.label}</span>}
        <div className="text-box">{surface}</div>
      </div>
    )
  if (!block) return surface
  return (
    <div className="field-block">
      {showLabel && <span className="block-label">{field.label}</span>}
      {surface}
    </div>
  )
}

/** A switch, or with Auto three: Auto, On and Off. */
function ToggleField({ field, bound, label }: Drawn) {
  const value = bound.value ?? field.default
  if (field.auto) {
    const options: [string, boolean | null][] = [["Auto", null], ["On", true], ["Off", false]]
    return (
      <div className="slide" role="radiogroup" aria-label={label}>
        <span className="ctl-name">{field.label}</span>
        <span className="segments">
          {options.map(([word, is]) => (
            <button key={word} type="button" role="radio" aria-checked={(value ?? null) === is} className={`seg nodrag ${(value ?? null) === is ? "on" : ""}`} onClick={() => (is === null ? bound.unset() : bound.write(is))}>
              {word}
            </button>
          ))}
        </span>
      </div>
    )
  }
  return (
    <label className="slide">
      <input className="nodrag" type="checkbox" role="switch" aria-label={label} checked={value === true} onChange={(event) => bound.write(event.target.checked)} />
      <span className="ctl-name">{field.label}</span>
      <Reset field={field} bound={bound} />
    </label>
  )
}

/** A slider and its value, written as it's let go. With Auto, unset is Auto. */
function NumberField({ field, bound, label }: Drawn) {
  const held = typeof bound.value === "number" ? bound.value : undefined
  const fallback = typeof field.default === "number" ? field.default : (field.min ?? 0)
  const [moving, setMoving] = useState<number | null>(null)
  const shown = moving ?? held ?? fallback
  const auto = field.auto && held === undefined && moving === null
  const done = () => {
    if (moving !== null && moving !== held) bound.write(moving)
    setMoving(null)
  }
  return (
    <label className={`slide ${auto ? "auto" : ""}`} title={field.label}>
      <span className="ctl-name">{field.label}</span>
      <input
        className="nodrag nopan"
        type="range"
        aria-label={label}
        min={field.min ?? undefined}
        max={Math.max(field.max ?? 0, shown)}
        step={field.step ?? (field.integer ? 1 : "any")}
        value={shown}
        onChange={(event) => setMoving(Number(event.target.value))}
        onPointerUp={done}
        onKeyUp={done}
      />
      <span className="num">{auto ? "Auto" : `${shown}${field.unit ? ` ${field.unit}` : ""}`}</span>
      {field.auto && held !== undefined ? (
        <button type="button" className="reset nodrag" aria-label={`${field.label}: Auto`} title="Auto" onClick={bound.unset}>
          Auto
        </button>
      ) : (
        <Reset field={field} bound={bound} />
      )}
    </label>
  )
}

/** Options that each say what they are: a dropdown of their words and a line on each, showing the chosen one's words shut. */
function Described({ field, bound, label }: Drawn) {
  const options = field.options ?? []
  const automatic = options.find((option) => typeof option === "string" && option.toLowerCase() === "auto")
  const held = bound.value ?? field.default ?? (field.auto ? automatic : undefined)
  const at = options.findIndex((option) => JSON.stringify(option) === JSON.stringify(held))
  const [open, setOpen] = useState(false)
  const choose = (option: unknown) => {
    setOpen(false)
    bound.write(option)
  }
  return (
    <div className="slide described" onBlur={(event) => !event.currentTarget.contains(event.relatedTarget as Node | null) && setOpen(false)}>
      <span className="ctl-name">{field.label}</span>
      <button type="button" className="field dropdown nodrag" aria-label={label} aria-haspopup="listbox" aria-expanded={open} onClick={() => setOpen(!open)} onKeyDown={(event) => event.key === "Escape" && setOpen(false)}>
        {at < 0 ? (field.placeholder ?? "Choose") : optionWord(field, options[at])}
      </button>
      {open && (
        <ul className="dropdown-list nodrag nowheel" role="listbox" aria-label={label}>
          {field.auto && automatic === undefined && <li><button type="button" role="option" aria-selected={at < 0} onClick={() => { bound.unset(); setOpen(false) }}>Auto</button></li>}
          {options.map((option, i) => (
            <li key={i}>
              <button type="button" role="option" aria-selected={i === at} className={i === at ? "on" : ""} onClick={() => choose(option)}>
                <b>{optionWord(field, option)}</b>
                {field.descriptions?.[String(option)] && <span className="muted">{field.descriptions[String(option)]}</span>}
              </button>
            </li>
          ))}
        </ul>
      )}
      <Reset field={field} bound={bound} />
    </div>
  )
}

/** Up to four options side by side, more in a menu; options that say what they are, a dropdown. */
function ChoiceField(props: Drawn) {
  const { field, bound, label } = props
  if (field.descriptions && Object.keys(field.descriptions).length) return <Described {...props} />
  const options = field.options ?? []
  const automatic = options.find((option) => typeof option === "string" && option.toLowerCase() === "auto")
  const held = bound.value ?? field.default ?? (field.auto ? automatic : undefined)
  const at = options.findIndex((option) => JSON.stringify(option) === JSON.stringify(held))
  if (options.length <= 4) {
    return (
      <div className="slide" role="radiogroup" aria-label={label}>
        <span className="ctl-name">{field.label}</span>
        <span className="segments">
          {field.auto && automatic === undefined && <button type="button" role="radio" aria-checked={at < 0} className={`seg nodrag ${at < 0 ? "on" : ""}`} onClick={bound.unset}>Auto</button>}
          {options.map((option, i) => (
            <button key={i} type="button" role="radio" aria-checked={i === at} className={`seg nodrag ${i === at ? "on" : ""}`} onClick={() => bound.write(option)}>
              {optionWord(field, option)}
            </button>
          ))}
        </span>
        <Reset field={field} bound={bound} />
      </div>
    )
  }
  return (
    <label className="slide">
      <span className="ctl-name">{field.label}</span>
      <select className="field nodrag" aria-label={label} value={at} onChange={(event) => Number(event.target.value) < 0 ? bound.unset() : bound.write(options[Number(event.target.value)])}>
        <option value={-1} disabled={!field.auto || automatic !== undefined} hidden={!field.auto || automatic !== undefined}>
          {field.auto ? "Auto" : ""}
        </option>
        {options.map((option, i) => (
          <option key={i} value={i}>
            {optionWord(field, option)}
          </option>
        ))}
      </select>
      <Reset field={field} bound={bound} />
    </label>
  )
}

/** Random, or a number kept. */
function SeedField({ field, bound, label }: Drawn) {
  const held = typeof bound.value === "number" ? bound.value : null
  return (
    <div className="slide" role="group" aria-label={label}>
      <span className="ctl-name">{field.label}</span>
      <span className="segments">
        <button type="button" className={`seg nodrag ${held === null ? "on" : ""}`} aria-pressed={held === null} onClick={bound.unset}>
          Random
        </button>
        <button type="button" className={`seg nodrag ${held !== null ? "on" : ""}`} aria-pressed={held !== null} onClick={() => held === null && bound.write(Math.floor(Math.random() * 2 ** 31))}>
          Fixed
        </button>
      </span>
      {held !== null && (
        <input
          className="field seed nodrag"
          type="number"
          aria-label={`${label}: its number`}
          min={0}
          step={1}
          defaultValue={held}
          key={held}
          onBlur={(event) => {
            const typed = Number(event.target.value)
            if (Number.isInteger(typed) && typed >= 0 && typed !== held) bound.write(typed)
          }}
          onKeyDown={(event) => event.key === "Enter" && event.currentTarget.blur()}
        />
      )}
    </div>
  )
}

/** What a mention would put in a slot that takes `accept`, or why it can't: by what the box it names is. */
export function whyNotIn(store: CanvasStore, accept: Media | undefined, mention: string, besides: string, label: string): string | null {
  const said = store.mentionOf(mention, besides)
  if (!said) return `${baseOf(mention)} is nothing in this project`
  const kind = store.kindOf(mention, besides)
  if (!accept || kind === accept) return null
  return `${label} takes ${WANTED[accept]}, and ${baseOf(mention)} is ${kind === "text" ? "words" : (WANTED[kind as Media] ?? "a file")}`
}

/** A slot's file: its thumbnail, and on a video its in and out. One of another kind, written by hand, is marked: a render ignores it (decision 8). */
function Held({ name, mention, label, wrong, onRange, onOff }: { name: string; mention: string; label: string; wrong: string | null; onRange(next: string): void; onOff(): void }) {
  const store = useCanvasStore()
  const said = store.mentionOf(mention, name)
  useElement(said?.name ?? "")
  const file = store.fileOf(mention, name)
  const range = rangeOf(mention)
  const angled = angleOf(mention)
  const missing = !said || said.gone
  const fps = file?.fps ?? FPS
  const frames = framesIn(file?.duration_ms, fps)
  return (
    <div className={`slot-held ${missing ? "missing" : ""}`}>
      <span className={`chip ${wrong && !missing ? "refused" : ""}`} title={wrong && !missing ? `${wrong}: a render leaves it out` : mention} data-mention={mention}>
        {baseOf(mention)}
        {range && ` ${timecode(range.from, fps)}–${timecode(range.to ?? frames, fps)}`}
        {angled && ` ${degrees(angled)}`}
        <button type="button" className="nodrag" aria-label={`Take ${baseOf(mention)} out of ${label}`} onClick={onOff}>
          ×
        </button>
      </span>
      {file && !file.absent && file.field === "image" && (
        <span className="slot-thumb">
          <Thumb sha256={file.sha256} cardWidth={120} />
        </span>
      )}
      {file && !file.absent && file.field === "video" && !angled && (
        <ClipRange
          sha256={file.sha256}
          frames={frames}
          fps={fps}
          from={range?.from ?? 0}
          to={range?.to ?? frames}
          width={240}
          label={`${label}: ${baseOf(mention)}`}
          onSet={(from, to) => onRange(ranged(baseOf(mention), from, to, frames))}
        />
      )}
    </div>
  )
}

/** A drop target: a box or one of its versions, or `<name>` typed in it. What it can't take is refused at the drop, with why, and said as it's carried over. */
function SlotField({ name, field, bound, view, label, place, siblings }: Drawn & { siblings?: Row }) {
  const store = useCanvasStore()
  const slot = useRef<HTMLDivElement>(null)
  const [typed, setTyped] = useState("")
  const value = bound.value
  const held = (Array.isArray(value) ? value : [value]).flatMap((item) => (typeof item === "string" ? mentionsIn(item) : []))
  const write = (mentions: string[]) => {
    const said = mentions.map((one) => `<${one}>`)
    if (!said.length) return bound.unset()
    bound.write(field.multiple ? said : said[0])
  }
  const refusal = (mention: string) => whyNotIn(store, (field.accept as Media | null) ?? undefined, mention, name, field.label)
  const take = (mentions: string[]) => {
    const fresh = mentions.filter((one) => store.resolveName(baseOf(one)) !== store.resolveName(name))
    const why = fresh.map(refusal).find(Boolean)
    if (why) return void say(why)
    if (fresh.length) write(field.multiple ? [...held, ...fresh] : fresh.slice(-1))
  }
  useEffect(() => {
    const el = slot.current
    if (!el) return
    store.dropTargets.set(el, { over: () => {}, whyNot: (names) => names.map(refusal).find(Boolean) ?? null, drop: take })
    return () => void store.dropTargets.delete(el)
  })
  const spans = field.spans ? spanOf(store, name, field, held[0], siblings) : null
  const refusing = useSignal(store.refusing)
  const [carried, setCarried] = useState(false)
  // A take carried over it says whether it takes it, and let go on it, it's that take.
  useEffect(
    () =>
      carriedAt.subscribe(() => {
        const at = carriedAt.get()
        const rect = carriedFile.get() && at ? slot.current?.getBoundingClientRect() : undefined
        setCarried(Boolean(at && rect && at.x >= rect.left && at.x <= rect.right && at.y >= rect.top && at.y <= rect.bottom))
      }),
    [],
  )
  useEffect(() => {
    const el = slot.current
    if (!el) return
    const heard = (event: Event) => {
      const mention = store.carriedVersion(name)
      if (!mention) return
      event.stopPropagation()
      setCarried(false)
      take([mention])
    }
    el.addEventListener(LET_GO, heard)
    return () => el.removeEventListener(LET_GO, heard)
  })
  const version = carried ? store.carriedVersion(name) : null
  const why = (refusing && refusing.el === slot.current ? refusing.why : null) ?? (version ? refusal(version) : null)
  return (
    <div
      className={`slot-control ${held.length ? "filled" : ""} ${why ? "refuses" : ""}`}
      ref={slot}
      data-drop-target=""
      data-field={place}
      data-accept={field.accept}
      role="group"
      aria-label={label}
    >
      <span className="ctl-name">{field.label}</span>
      {why && <span className="refusal">{why}</span>}
      {held.map((one, i) => (
        <Held key={`${i}:${baseOf(one)}`} name={name} mention={one} label={field.label} wrong={refusal(one)} onOff={() => write(held.filter((other) => other !== one))} onRange={(next) => write(held.map((other) => (other === one ? next : other)))} />
      ))}
      {(!held.length || field.multiple) && (
        <input
          className="slot-type nodrag"
          aria-label={`${label}: a name`}
          placeholder={`Drag ${WANTED[(field.accept as Media | null) ?? "image"]} here`}
          value={typed}
          onChange={(event) => setTyped(event.target.value)}
          onKeyDown={(event) => {
            if (event.key !== "Enter") return
            const mention = typed.trim().replace(/^<|>$/g, "")
            if (!mention) return
            setTyped("")
            take([mention])
          }}
        />
      )}
      {spans && <span className={`spans ${spans.differs ? "warn" : ""}`}>{spans.says}</span>}
      {view.asset?.missing?.some((gone) => held.includes(gone)) && <span className="muted">Not in this project any more</span>}
    </div>
  )
}

/** A slot whose video is the timeline of a list group's rows: both lengths, and a warning when they differ. */
function spanOf(store: CanvasStore, name: string, field: ToolField, mention: string | undefined, siblings: Row | undefined) {
  if (!mention || !field.spans) return null
  const tool = store.tools.get().get(store.get(name)?.asset?.tool ?? "")
  const group = (tool?.fields ?? []).find((held) => held.field === field.spans)
  const rows = rowsIn(siblings ? siblings[field.spans] : store.valueOf(name, field.spans))
  const total = group && totalOf(group, fullRows(group, rows))
  const file = store.fileOf(mention, name)
  if (!total || !file?.duration_ms || angleOf(mention)) return null
  const fps = file.fps ?? FPS
  const range = rangeOf(mention)
  const frames = (range?.to ?? framesIn(file.duration_ms, fps)) - (range?.from ?? 0)
  const seconds = Math.round((frames / fps) * 10) / 10
  const differs = Math.abs(seconds - total.sum) >= 0.5
  const unit = total.member.unit ?? "s"
  return { differs, says: differs ? `${field.label} is ${seconds} ${unit}, and the ${group.label.toLowerCase()} ${total.sum} ${unit}: the ${field.label.toLowerCase()} wins` : `${seconds} ${unit}` }
}

/** One row of a list, or a block: each member drawn by these same rules. */
function Members({ name, group, bound, view, place, label }: { name: string; group: ToolField; bound: Bound; view: ElementView; place: string; label: string }) {
  const members = group.members ?? []
  const row = isRow(bound.value) ? bound.value : {}
  const each = (member: ToolField) => (
    <WhenOff key={member.field} why={whenOff(member, members, (peer) => row[peer])}>
      <FieldControl
        name={name}
        field={member}
        bound={memberBound(bound, member.field)}
        view={view}
        place={`${place}.${member.field}`}
        label={member.type === "prompt" || member.type === "words" ? label : `${label}'s ${member.label.toLowerCase()}`}
        named={member.type === "group" || !(member.type === "prompt" || member.type === "words")}
        siblings={row}
      />
    </WhenOff>
  )
  return (
    <>
      {members.filter((member) => member.type === "prompt" || member.type === "words").map(each)}
      <div className="beat-time">{members.filter((member) => member.type !== "prompt" && member.type !== "words" && member.type !== "group").map(each)}</div>
      {members.filter((member) => member.type === "group").map(each)}
    </>
  )
}

/** Rows, each its members, with + and × down to its least and up to its most, dragged by the grip to reorder. A file holding fewer shows them, to write in, and holds them once one's touched. */
function ListGroup({ name, field, bound, view, place }: Drawn) {
  const store = useCanvasStore()
  const { client } = useConnection()
  const focused = useSignal(store.focusedName)
  const rows = fullRows(field, rowsIn(bound.value))
  const sections = useContext(Sections)
  const accordion = Boolean(sections) || (view.asset?.kind === "video" && view.hereWords && field.field === "beats")
  const run = accordion && focused === name ? watched(view)?.run : undefined
  const live = view.run?.number === run ? view.run : undefined
  const settled = (status: string | undefined) => status === "completed" || status === "failed" || status === "canceled"
  const enabled = Boolean(store.projectId) && run !== undefined && !settled(live?.status)
  const { data: full } = useQuery({
    ...runFullQuery(client, store.projectId ?? "", run ?? 0, live?.settled_at_ms ?? 0),
    enabled,
    refetchInterval: (query) => enabled && !settled(query.state.data?.status) ? 1000 : false,
    refetchIntervalInBackground: false,
  })
  const progress = run === undefined ? null : progressOfBeats(field, rows, full)
  const timing = field.members?.find((member) => member.total)
  const cut = accordion && view.continuing ? view.continuing.beats_from ?? view.continuing.out_frame / (view.continuingFps ?? 24) : 0
  let elapsed = 0
  const kept = rows.map((value) => {
    const duration = timing ? Number(value[timing.field] ?? timing.default) : NaN
    elapsed += Number.isFinite(duration) && duration > 0 ? duration : NaN
    return cut > 0 && elapsed <= cut + 1e-6
  })
  const keptCount = kept.filter(Boolean).length
  const [own, setOwn] = useState<number | null>(() => keptCount < rows.length ? keptCount : null)
  // In the frame, one section is open at a time, its first prompt's or a row's.
  const [expanded, setExpanded] = sections ? [sections.row, sections.setRow] as const : [own, setOwn] as const
  const panelId = useId()
  // A new continuation opens its first unfinished Beat; editing rows keeps the chosen accordion open.
  // biome-ignore lint/correctness/useExhaustiveDependencies: only a changed continuation resets which Beat is open
  useEffect(() => { if (!sections || view.continuing) setExpanded(keptCount < rows.length ? keptCount : null) }, [view.continuing])
  const called = field.label.replace(/s$/, "")
  const least = field.min_items ?? 0
  const list = useRef<HTMLDivElement>(null)
  const [moving, setMoving] = useState<{ from: number; to: number } | null>(null)
  const movingNow = useRef<{ from: number; to: number } | null>(null)
  const move = (next: { from: number; to: number } | null) => { movingNow.current = next; setMoving(next) }
  const drop = () => {
    const held = movingNow.current
    if (held && held.to !== held.from) {
      const next = [...rows]
      const [one] = next.splice(held.from, 1)
      next.splice(held.to, 0, one)
      bound.write(next)
      if (accordion) setExpanded((open) => open === null ? null : open === held.from ? held.to : held.from < open && held.to >= open ? open - 1 : held.from > open && held.to <= open ? open + 1 : open)
    }
    move(null)
  }
  const rowBound = (i: number): Bound => ({
    value: rows[i],
    draft: (next) => { if (!kept[i]) bound.draft(rows.map((other, at) => (at === i ? next : other))) },
    write: (next) => { if (!kept[i]) bound.write(rows.map((other, at) => (at === i ? next : other))) },
    unset: () => { if (!kept[i]) bound.write(rows.filter((_, at) => at !== i)) },
    save: () => { if (!kept[i]) bound.save() },
  })
  const slotAt = (y: number) => {
    const held = [...(list.current?.querySelectorAll<HTMLElement>(":scope > .beat") ?? [])]
    const at = held.findIndex((el) => y < el.getBoundingClientRect().top + el.getBoundingClientRect().height / 2)
    return Math.max(keptCount, at < 0 ? held.length - 1 : at)
  }
  const grip = (i: number) => ({
    onPointerDown(event: PointerEvent<HTMLElement>) {
      event.stopPropagation()
      if (event.button !== 0 || event.pointerType === "mouse") return
      event.currentTarget.setPointerCapture(event.pointerId)
      move({ from: i, to: i })
    },
    onPointerMove(event: PointerEvent<HTMLElement>) {
      if (movingNow.current) move({ ...movingNow.current, to: slotAt(event.clientY) })
    },
    onPointerUp() { if (movingNow.current) drop() },
    onPointerCancel(event: PointerEvent<HTMLElement>) { if (event.pointerType !== "mouse") move(null) },
    draggable: true,
    onDragStart(event: DragEvent<HTMLElement>) {
      event.stopPropagation()
      event.dataTransfer.effectAllowed = "move"
      event.dataTransfer.setData("application/x-cozy-beat", String(i))
      const card = event.currentTarget.closest<HTMLElement>(".beat")
      if (card) {
        const rect = card.getBoundingClientRect()
        event.dataTransfer.setDragImage(card, event.clientX - rect.left, event.clientY - rect.top)
      }
      move({ from: i, to: i })
    },
    onDragEnd() { move(null) },
  })
  const add = rows.length < (field.max_items ?? Infinity) && (
    <button type="button" className="add-beat nodrag" title={`Add a ${called.toLowerCase()}`} onClick={() => { bound.write([...rows, newRow(field)]); if (accordion) setExpanded(rows.length) }}>
      + {called}
    </button>
  )
  return (
    <div className="beats-edit" role="group" aria-label={field.label} data-field={place} ref={list}
      onDragOver={(event) => {
        if (!movingNow.current) return
        event.preventDefault()
        event.stopPropagation()
        event.dataTransfer.dropEffect = "move"
        move({ ...movingNow.current, to: slotAt(event.clientY) })
      }}
      onDrop={(event) => {
        if (!movingNow.current) return
        event.preventDefault()
        event.stopPropagation()
        drop()
      }}>
      {rows.map((value, i) => {
        const status = progress?.[i]
        const statusWord = status?.state === "skipped" ? "Skipped" : kept[i] || status?.state === "kept" ? "Kept" : status?.state === "completed" ? "Complete" : status?.state === "in_progress" ? "Generating" : run !== undefined && progress ? "Queued" : null
        const handle = !kept[i] && rows.length - keptCount > 1 && <span className="grip nodrag nopan" role="button" aria-label={`Move ${called.toLowerCase()} ${i + 1}`} title="Drag to reorder" {...grip(i)}>⋮⋮</span>
        const remove = !kept[i] && rows.length > least && <button type="button" className="iconb nodrag" aria-label={`Remove ${called.toLowerCase()} ${i + 1}`} title={`Remove ${called.toLowerCase()} ${i + 1}`} onClick={() => {
          bound.write(rows.filter((_, at) => at !== i))
          if (accordion) setExpanded((open) => open === null ? null : open === i ? Math.max(0, Math.min(i, rows.length - 2)) : open > i ? open - 1 : open)
        }}><Icon name="trash" /></button>
        const body = <div className="beat-body" inert={kept[i]}><Members name={name} group={field} bound={rowBound(i)} view={view} place={`${place}.${i}`} label={`${called} ${i + 1}`} /></div>
        const duration = timing ? value[timing.field] ?? timing.default : undefined
        const open = expanded === i
        return <div className={`beat ${accordion ? "beat-accordion" : ""} ${kept[i] ? "beat-kept" : rows.length > least ? "can-go" : ""} ${moving?.from === i ? "moving" : ""} ${moving && moving.to === i && moving.from !== i ? "landing" : ""}`} data-beat-index={i} data-beat-kept={kept[i] || undefined} data-beat-status={status?.state} key={i}>
          {accordion ? <>
            <div className="beat-heading">{handle}<button type="button" className="beat-toggle" aria-label={`${called} ${i + 1}${statusWord ? `, ${statusWord.toLowerCase()}` : ""}`} aria-expanded={open} aria-controls={`${panelId}-${i}`} onClick={() => setExpanded(open ? null : i)}><b>{called} {i + 1}</b>{statusWord && <span className="beat-status">{statusWord}</span>}{duration != null && <span>{String(duration)} {timing?.unit ?? ""}</span>}<Icon name="forward" /></button>{remove}</div>
            {status && status.state !== "kept" && status.state !== "skipped" && <div className="beat-progress" role="progressbar" aria-label={`${called} ${i + 1} published frames`} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(status.completed * 100)}><span style={{ width: `${status.completed * 100}%` }} /></div>}
            <div className="beat-panel" id={`${panelId}-${i}`} data-open={open || undefined} aria-hidden={!open} inert={!open}>{body}</div>
          </> : <>{handle}{body}{remove}</>}
        </div>
      })}
      <div className="beats-last">{add}</div>
    </div>
  )
}

/** A block of fields inside the box. One that may be absent is added with + and taken out with ×. */
function BlockGroup({ name, field, bound, view, place, label }: Drawn) {
  const present = isRow(bound.value)
  if (!present && !field.required) {
    return (
      <div className="group-block absent" data-field={place}>
        <button type="button" className="add-beat nodrag" onClick={() => bound.write(newRow(field))}>
          + {field.label}
        </button>
      </div>
    )
  }
  const held: Bound = present ? bound : { ...bound, value: newRow(field) }
  return (
    <div className="group-block" role="group" aria-label={label} data-field={place}>
      <div className="group-head">
        <span className="ctl-name">{field.label}</span>
        {!field.required && (
          <button type="button" className="iconb nodrag" aria-label={`Remove ${field.label.toLowerCase()}`} title={`Remove ${field.label.toLowerCase()}`} onClick={bound.unset}>
            <Icon name="trash" />
          </button>
        )}
      </div>
      <Members name={name} group={field} bound={held} view={view} place={place} label={label} />
    </div>
  )
}

/** One field, drawn by its type. */
export function FieldControl(props: Drawn & { siblings?: Row }) {
  switch (props.field.type) {
    case "prompt":
    case "words":
      return <TextField {...props} />
    case "group":
      return props.field.list ? <ListGroup {...props} /> : <BlockGroup {...props} />
    case "number":
      return <NumberField {...props} />
    case "choice":
      return <ChoiceField {...props} />
    case "toggle":
      return <ToggleField {...props} />
    case "slot":
      return <SlotField {...props} />
    case "seed":
      return <SeedField {...props} />
    default:
      return null
  }
}

/** A field of the box itself: on its face under its label, beside its name under More, or in the frame. */
export function ControlRow({ name, field, view, focus, named, framed }: { name: string; field: ToolField; view: ElementView; focus: boolean; named: boolean; framed?: boolean }) {
  const store = useCanvasStore()
  const tools = useSignal(store.tools)
  const peers = (view.asset?.tool && tools.get(view.asset.tool)?.fields) || []
  return (
    <WhenOff why={whenOff(field, peers, (peer) => store.valueOf(name, peer))}>
      <FieldControl name={name} field={field} bound={topBound(store, name, field.field)} view={view} place={field.field} label={field.label} focus={focus} named={named} block={!framed} framed={framed} />
    </WhenOff>
  )
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
        onSubmit={() => void store.save(name)}
      />
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

/** The rows the frame opens one at a time, with its first prompt's section (cl-938): absent outside the frame. */
const Sections = createContext<{ row: number | null; setRow: Dispatch<SetStateAction<number | null>> } | null>(null)

/** What the frame holds while a take is written, as an image's prompt is held over it (cl-938, Paul 11-12): its first prompt (else its first
 *  slot), and the list of prompts that adds up to it (H3's beats). A tool with its own box draws all of that itself. */
export function framedOf(view: ElementView, fields: ToolField[], tool: Tool | undefined): ToolField[] {
  // A picture or a video with no tool asks nothing: its words are a tool's to ask for (Paul, 11-12, cl-933).
  if (!view.hereWords || tool?.ui || (!view.asset?.tool && view.asset?.kind !== "text")) return []
  const first = fields.find((field) => field.type === "prompt" || field.type === "words") ?? fields.find((field) => field.type === "slot")
  const rows = fields.find((field) => field.type === "group" && field.list && field.members?.some((member) => member.total) && field.members.some((member) => member.type === "prompt"))
  return [...(first ? [first] : []), ...(rows ? [rows] : [])]
}

/** Behind what's written, faded, every picture it uses, mentioned or an image input, filling the frame as large as they can (`Spread`, cl-949);
 *  a new set fades in as the writing moves. */
function Backdrop({ name, view, mentions }: { name: string; view: ElementView; mentions: string[] }) {
  const store = useCanvasStore()
  useSignal(store.names)
  const pictures = view.asset ? store.mentionedPictures(name, mentions) : []
  if (!pictures.length) return null
  return (
    <div key={pictures.map((ghost) => ghost.mention).join(" ")} className="backdrop" aria-hidden>
      <Spread pictures={pictures.map((ghost) => ({ key: ghost.mention, sha256: ghost.sha256, shape: ghost.shape, making: ghost.making }))} cardWidth={960} />
    </div>
  )
}

/** The frame being written in, the same for a picture and a video: one prompt fills it; two or more are sections, each opened by its head, one
 *  at a time, with + for another row. Behind them the pictures the section being written mentions; with none being written, all it mentions;
 *  with none mentioned, its take, faded (the media under it). */
export function InFrame({ name, view }: { name: string; view: ElementView }) {
  const store = useCanvasStore()
  const tools = useSignal(store.tools)
  const [open, setOpen] = useState<"first" | number | null>("first")
  const [writing, setWriting] = useState<"first" | number | null>(null)
  const panel = useId()
  const fields = view.asset ? framedOf(view, fieldsOf(view.asset, tools), view.asset.tool ? tools.get(view.asset.tool) : undefined) : []
  if (!fields.length) return null
  const [first, list] = fields
  const values = (field: ToolField) => store.valueOf(name, field.field)
  const rows = list ? rowsIn(values(list)) : []
  const mentionsOf = (texts: string[]) => [...new Set(texts.flatMap(mentionsIn))]
  // What it's made from: what the words being written mention, and its pictures given as inputs (cl-949: every one, never one picked).
  const inputs = view.asset ? fieldsOf(view.asset, tools).filter((field) => field.type === "slot").flatMap((field) => textsIn(values(field))) : []
  const written = writing === "first" ? textsIn(values(first)) : typeof writing === "number" ? textsIn(rows[writing]) : [...textsIn(values(first)), ...rows.flatMap(textsIn)]
  const mentions = mentionsOf([...written, ...inputs])
  if (!list) {
    return (
      <div className="in-frame">
        <Backdrop name={name} view={view} mentions={mentions} />
        <ControlRow name={name} field={first} view={view} focus={store.focusing === name} named={false} framed />
      </div>
    )
  }
  const sectionOf = (target: EventTarget | null): "first" | number | null => {
    const place = (target as Element | null)?.closest?.("[data-field]")?.getAttribute("data-field") ?? ""
    const row = place.match(new RegExp(`^${list.field}\\.(\\d+)`))
    return row ? Number(row[1]) : place === first.field ? "first" : null
  }
  const shut = open !== "first"
  return (
    <Sections.Provider value={{ row: typeof open === "number" ? open : null, setRow: (action) => setOpen((now) => {
      const row = typeof action === "function" ? action(typeof now === "number" ? now : null) : action
      return row ?? (now === "first" ? "first" : null)
    }) }}>
      <div className="in-frame sections nowheel"
        onFocus={(event: FocusEvent<HTMLDivElement>) => setWriting(sectionOf(event.target))}
        onBlur={(event: FocusEvent<HTMLDivElement>) => { if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setWriting(null) }}>
        <Backdrop name={name} view={view} mentions={mentions} />
        <div className="frame-sections">
          <div className="beat beat-accordion frame-first" data-section={first.field}>
            <div className="beat-heading">
              <button type="button" className="beat-toggle" aria-expanded={!shut} aria-controls={panel} onClick={() => setOpen(shut ? "first" : null)}><b>{first.label}</b><Icon name="forward" /></button>
            </div>
            <div className="beat-panel" id={panel} data-open={!shut || undefined} aria-hidden={shut} inert={shut}>
              <div className="beat-body"><ControlRow name={name} field={first} view={view} focus={store.focusing === name} named={false} framed /></div>
            </div>
          </div>
          <ControlRow name={name} field={list} view={view} focus={false} named={false} framed />
        </div>
      </div>
    </Sections.Provider>
  )
}

const plain = (value: unknown): string => (typeof value === "object" && value !== null ? Object.values(value).map(plain).join(" ") : String(value))

/** What it holds that no field declares: only to see. */
function HeldLine({ field, value }: { field: string; value: unknown }) {
  const text = plain(value)
  return (
    <div className="slide held-line" title={text}>
      <span className="ctl-name">{fieldWords(field)}</span>
      <span className="held-value">{text}</span>
    </div>
  )
}

/** What beside Render is only a thing to set: the rest of the face wants a line. */
const small = (field: ToolField) => field.type === "toggle" || field.type === "number" || (field.type === "choice" && !field.descriptions)

/** What a box asks and holds, under its frame (what it's written in is in the frame, InFrame); its tool and Models & LoRAs are beneath the
 *  node (ToolLine). In Focus (`focused`), the same for a picture and a video (cl-938): Render, then every field at once, nothing under
 *  More or Advanced (cl-934). Elsewhere what isn't on the face is under More. What's only a thing to set is grouped, named. */
export function AssetEdit({ name, view, editing, focused = false, actions, children }: { name: string; view: ElementView; editing: boolean; focused?: boolean; actions: ReactNode; children?: ReactNode }) {
  const store = useCanvasStore()
  const tools = useSignal(store.tools)
  if (!view.asset) return null
  const tool = view.asset.tool ? tools.get(view.asset.tool) : undefined
  const fields = makes(view) ? fieldsOf(view.asset, tools) : []
  const inFrame = framedOf(view, fields, tool)
  const first = fields.find((field) => field.type === "prompt" || field.type === "words")
  // The box's own shape is its shape: its corner sets it.
  const rest = fields.filter((field) => !inFrame.includes(field) && field.type !== "timeline" && field.type !== "shape")
  const faced = !editing ? [] : focused ? rest : rest.filter((field) => field.field === "megapixels" || onFace(field, store.valueOf(name, field.field)))
  const beyond = heldBeyond({ ...view.asset.fields, ...view.sent, ...view.drafts }, fields)
  const folded = rest.filter((field) => !faced.includes(field))
  const plainFolded = folded.filter((field) => !field.advanced)
  const expert = folded.filter((field) => field.advanced)
  const parameters = [...faced.filter(small), ...expert.filter(small)]
  const open = store.showsMore(name)
  const row = (field: ToolField) => {
    const control = <ControlRow key={field.field} name={name} field={field} view={view} focus={store.focusing === name && field === first} named={folded.includes(field)} />
    return focused && field.type === "group" && field.list ? <section key={field.field} className="focus-parameter-group"><h3>{field.label}</h3>{control}</section> : control
  }
  const held = beyond.map(([field, value]) => <HeldLine key={field} field={field} value={value} />)
  return (
    <div className={`edit-face ${focused ? "focused" : ""}`}>
      <ToolBox tool={tool} name={name} view={view} shown={faced.filter((field) => !small(field))} folded={plainFolded} row={row}>
        {(lines, more) => {
          const behind = !focused && ((more !== null && plainFolded.length > 0) || beyond.length > 0)
          const render = (
            <div className="row actions">
              {behind && <button type="button" className="more-toggle nodrag" aria-expanded={open} onClick={() => store.setMore(name, !open)}><span className="more-word">{open ? "Less" : "More"}</span></button>}
              {actions}
            </div>
          )
          const grouped = more !== null && parameters.length > 0 && <div className="edit-parameters">{parameters.map((field) => <ControlRow key={field.field} name={name} field={field} view={view} focus={false} named />)}</div>
          if (focused) {
            return (
              <>
                {render}
                {editing && lines && <div className="edit-shown">{lines}</div>}
                {grouped}
                {held}
                {children}
              </>
            )
          }
          return (
            <>
              {editing && lines && <div className="edit-shown">{lines}</div>}
              {more !== null && expert.some((field) => !small(field)) && <div className="advanced">{expert.filter((field) => !small(field)).map(row)}</div>}
              {grouped}
              {behind ? (
                open && (
                  <div className="edit-more">
                    {more}
                    {held}
                    {children}
                  </div>
                )
              ) : (
                children
              )}
              {render}
            </>
          )
        }}
      </ToolBox>
    </div>
  )
}
