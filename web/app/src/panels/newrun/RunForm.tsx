import { useState } from "react"
import type { RequestField } from "../../types"
import { Copyable } from "../../ui/Copyable"
import { type Dropped, DropZone, FileRow } from "../FileDrop"
import { shown } from "../format"
import { acceptOf, hintOf, isFile, widgetOf } from "./fields"

/** A form's value that doesn't read: it stops the button, and says why beside the field. */
export const UNREADABLE = Symbol("unreadable")

export interface Files {
  files: Dropped[]
  add: (files: File[], onSent?: (file: Dropped) => void) => number[]
  remove: (id: number) => void
}

const rangeOf = (field: RequestField) => {
  const { ge, gt, le, lt } = field.constraints ?? {}
  const low = ge ?? gt
  const high = le ?? lt
  if (low !== undefined && high !== undefined) return `${low} to ${high}`
  return low !== undefined ? `from ${low}` : high !== undefined ? `to ${high}` : ""
}

function JsonField({ value, hint, onChange }: { value: unknown; hint: string; onChange: (value: unknown) => void }) {
  const [text, setText] = useState(value === undefined ? "" : JSON.stringify(value, null, 2))
  const [bad, setBad] = useState("")
  return (
    <>
      <textarea
        className="field area"
        spellCheck={false}
        placeholder={hint ? `${hint}, as JSON` : "As JSON"}
        rows={Math.min(8, Math.max(2, text.split("\n").length))}
        value={text}
        onChange={(event) => {
          const next = event.target.value
          setText(next)
          if (!next.trim()) {
            setBad("")
            return onChange(undefined)
          }
          try {
            onChange(JSON.parse(next))
            setBad("")
          } catch (error) {
            setBad(error instanceof Error ? error.message : "It isn't JSON")
            onChange(UNREADABLE)
          }
        }}
      />
      {bad && <small className="warn">{bad}</small>}
    </>
  )
}

function FileField({ field, asset, many, value, files, onChange }: { field: RequestField; asset: string; many: boolean; value: unknown; files: Files; onChange: (value: unknown) => void }) {
  const named = (many ? ((value as unknown[]) ?? []) : value ? [value] : []).filter(isFile) as string[]
  const [rows, setRows] = useState<number[]>([])
  const shown = files.files.filter((file) => rows.includes(file.id) || (file.digest && named.includes(file.digest)))

  const set = (digests: string[]) => onChange(many ? (digests.length ? digests : undefined) : digests[0])

  return (
    <div className="filefield">
      {shown.map((file) => (
        <FileRow
          key={file.id}
          file={file}
          onRemove={() => {
            files.remove(file.id)
            setRows((now) => now.filter((id) => id !== file.id))
            set(named.filter((digest) => digest !== file.digest))
          }}
        />
      ))}
      {(many || !shown.length) && (
        <DropZone
          accept={acceptOf(field, asset)}
          multiple={many}
          onFiles={(chosen) => {
            let held = named
            const ids = files.add(chosen, (sent) => {
              held = [...held, sent.digest!]
              set(many ? held : [sent.digest!])
            })
            setRows((now) => [...now, ...ids])
          }}
        >
          Drop {many ? `${asset}s` : `a ${asset}`} here, or choose
        </DropZone>
      )}
    </div>
  )
}

function Control({ field, value, files, target, onChange }: { field: RequestField; value: unknown; files: Files; target: string; onChange: (value: unknown) => void }) {
  const widget = widgetOf(field.type)
  const hint = hintOf(field)
  switch (widget.kind) {
    case "text":
      return (
        <textarea
          className="field area"
          spellCheck={false}
          rows={2}
          placeholder={hint}
          value={typeof value === "string" ? value : ""}
          onChange={(event) => onChange(event.target.value === "" ? undefined : event.target.value)}
        />
      )
    case "number": {
      const { ge, gt, le, lt, multiple_of: step } = field.constraints ?? {}
      return (
        <input
          className="field"
          type="number"
          min={ge ?? gt}
          max={le ?? lt}
          step={step ?? (widget.integer ? 1 : "any")}
          placeholder={[hint, rangeOf(field)].filter(Boolean).join(", ")}
          value={typeof value === "number" ? value : ""}
          onChange={(event) => {
            const read = widget.integer ? Number.parseInt(event.target.value, 10) : Number.parseFloat(event.target.value)
            onChange(Number.isFinite(read) ? read : undefined)
          }}
        />
      )
    }
    case "yesno":
      return (
        <select className="field" value={value === undefined ? "" : String(value)} onChange={(event) => onChange(event.target.value === "" ? undefined : event.target.value === "true")}>
          <option value="">{hint ? `Default: ${hint}` : "Default"}</option>
          <option value="true">Yes</option>
          <option value="false">No</option>
        </select>
      )
    case "choice":
      return (
        <select
          className="field"
          value={value === undefined ? "" : JSON.stringify(value)}
          onChange={(event) => onChange(event.target.value === "" ? undefined : JSON.parse(event.target.value))}
        >
          <option value="">{hint ? `Default: ${hint}` : "Default"}</option>
          {widget.options.map((option) => (
            <option key={JSON.stringify(option)} value={JSON.stringify(option)}>
              {String(option)}
            </option>
          ))}
        </select>
      )
    case "file":
      return <FileField field={field} asset={widget.asset} many={widget.many} value={value} files={files} onChange={onChange} />
    case "tree":
      return (
        <span className="filefield">
          <small className="muted">A folder is named from a terminal:</small>
          <Copyable text={`cozy run ${shown(target)} ${field.name}=<folder>`} />
        </span>
      )
    case "json":
      return <JsonField value={value} hint={hint} onChange={onChange} />
  }
}

/** A form generated from a function's interface: every field it declares, and only those. Nothing is refused here: Calcifer clamps and warns. */
export function RunForm({
  fields,
  values,
  files,
  target,
  onChange,
}: {
  fields: RequestField[]
  values: Record<string, unknown>
  files: Files
  target: string
  onChange: (name: string, value: unknown) => void
}) {
  return (
    <div className="runform">
      {fields.map((field) => (
        <label key={field.name} className={widgetOf(field.type).kind === "file" ? "asblock" : undefined}>
          <span className="fname">
            {field.name}
            {(field.wire === undefined || field.wire === "required") && <i>*</i>}
          </span>
          <Control field={field} value={values[field.name]} files={files} target={target} onChange={(value) => onChange(field.name, value)} />
        </label>
      ))}
    </div>
  )
}
