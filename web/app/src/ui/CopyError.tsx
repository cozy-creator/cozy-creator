import type { ErrorBody, Reason, Run } from "../api/calcifer"
import { CopyButton } from "./Copyable"

/** An error as plain text to paste: what it said, its code, its run and machine, what to do, and what it said in full (a traceback).
 *  Each part only where it's known; no command (Paul, 11-16). */
export function errorText({ message, code, run, machine, remedy, detail }: { message: string; code?: string; run?: number | string; machine?: string | null; remedy?: string | null; detail?: string | null }): string {
  return [
    message,
    code && `code: ${code}`,
    run !== undefined && `run: ${run}${machine ? ` on ${machine}` : ""}`,
    remedy,
    detail,
  ]
    .filter(Boolean)
    .join("\n")
}

/** A refusal's or failure's text, from what the API answers. */
export const bodyText = (body: ErrorBody, run?: number | string, machine?: string | null) =>
  errorText({ message: body.message, code: body.code, run, machine, remedy: body.remedy })

/** A run's failure, from its record. */
export const runText = (run: Run, reason: Reason) =>
  errorText({ message: reason.message, code: reason.code, run: run.name || run.number || run.id, machine: run.machine, detail: reason.detail })

/** Copy: the error, whole, on the clipboard as plain text (Paul, 11-12: "so I can post them here to you"), by the usual copy icon. */
export function CopyError({ text }: { text: string }) {
  return <CopyButton text={text} title="Copy the whole error" />
}
