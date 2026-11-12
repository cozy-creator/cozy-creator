import type { ReactNode } from "react"
import type { ToolField, When } from "../api/calcifer"
import { optionWord } from "./controls"
import "./When.css"

/** Whether `when` holds while its field is `value`: truthy as Python reads it, or one of its values. */
function holds(when: When, value: unknown): boolean {
  if (when.values?.length) return when.values.some((one) => JSON.stringify(one) === JSON.stringify(value))
  if (Array.isArray(value)) return value.length > 0
  if (value !== null && typeof value === "object") return Object.keys(value).length > 0
  return Boolean(value)
}

/** Why `field` doesn't apply now (`When()`), as its hint says it: `Only with Prompt upsampling on`; null while it does. `peers` are the
 *  fields beside it, `valueOf` what each holds; unset is its default. Nothing here knows any tool. */
export function whenOff(field: ToolField, peers: ToolField[], valueOf: (field: string) => unknown): string | null {
  const when = field.when
  const peer = when ? peers.find((one) => one.field === when.field) : undefined
  if (!when || !peer || holds(when, valueOf(when.field) ?? peer.default ?? null)) return null
  const values = when.values ?? []
  if (!values.length) return `Only with ${peer.label} ${peer.type === "toggle" ? "on" : "set"}`
  if (values.length === 1 && typeof values[0] === "boolean") return `Only with ${peer.label} ${values[0] ? "on" : "off"}`
  return `Only when ${peer.label} is ${values.map((value) => optionWord(peer, value)).join(" or ")}`
}

/** A field that doesn't apply now: dulled and disabled, its value kept, and why on hover. */
export function WhenOff({ why, children }: { why: string | null; children: ReactNode }) {
  if (!why) return <>{children}</>
  return (
    <fieldset className="when-off" disabled title={why}>
      {children}
    </fieldset>
  )
}
