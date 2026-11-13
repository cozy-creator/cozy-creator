import { useState } from "react"
import type { ErrorBody, Reason, Run } from "../api/calcifer"

/** An error as plain text to paste: what it said, its code, its run and machine and the line that shows it, what to do, and what it
 *  said in full (a traceback). Each part only where it's known. */
export function errorText({ message, code, run, machine, remedy, next, detail }: { message: string; code?: string; run?: number | string; machine?: string | null; remedy?: string | null; next?: string[]; detail?: string | null }): string {
  const watch = run !== undefined ? `cozy run watch ${run}` : undefined
  return [
    message,
    code && `code: ${code}`,
    run !== undefined && `run: ${run}${machine ? ` on ${machine}` : ""}`,
    remedy,
    ...[watch, ...(next ?? [])].filter((line, at, all): line is string => Boolean(line) && all.indexOf(line) === at),
    detail,
  ]
    .filter(Boolean)
    .join("\n")
}

/** A refusal's or failure's text, from what the API answers. */
export const bodyText = (body: ErrorBody, run?: number | string, machine?: string | null) =>
  errorText({ message: body.message, code: body.code, run, machine, remedy: body.remedy, next: body.next })

/** A run's failure, from its record. */
export const runText = (run: Run, reason: Reason) =>
  errorText({ message: reason.message, code: reason.code, run: run.name || run.number || run.id, machine: run.machine, next: reason.next, detail: reason.detail })

/** Copy: the error, whole, on the clipboard as plain text (Paul, 11-12: "so I can post them here to you"). Its click selects, opens and
 *  drags nothing. */
export function CopyError({ text }: { text: string }) {
  const [copied, setCopied] = useState(false)
  return (
    <button
      type="button"
      className="copy-error nodrag nopan"
      title="Copy the whole error"
      onClick={(event) => {
        event.stopPropagation()
        void navigator.clipboard.writeText(text).then(() => setCopied(true))
      }}
    >
      {copied ? "Copied" : "Copy"}
    </button>
  )
}
