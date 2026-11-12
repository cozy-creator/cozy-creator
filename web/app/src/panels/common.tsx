import { useQuery } from "@tanstack/react-query"
import { type ReactNode, useCallback, useLayoutEffect, useRef, useState } from "react"
import { ApiError } from "../api/client"
import { statusQuery } from "../api/queries"
import { useConnection } from "../app/Connection"
import { Copyable } from "../ui/Copyable"

/** A refusal as Calcifer said it: its message, its remedy, and `next` as text to copy. */
/** With `plain`, only what went wrong: no command, where someone is signing in here and may have no terminal to hand. */
export function Failure({ error, lead, plain }: { error: unknown; lead?: string; plain?: boolean }) {
  if (!error) return null
  const body = error instanceof ApiError ? error.body : undefined
  const message = error instanceof Error ? error.message : String(error)
  return (
    <div className="failure" role="alert">
      <span>{lead ? `${lead}: ${message}` : message}</span>
      {!plain && body?.remedy && <span className="remedy">{body.remedy}</span>}
      {!plain && body?.next?.map((command) => <Copyable key={command} text={command} />)}
    </div>
  )
}

export function KV({ rows }: { rows: [string, ReactNode][] }) {
  const shownRows = rows.filter(([, value]) => value !== undefined && value !== null && value !== "" && value !== false)
  return (
    <dl className="kv">
      {shownRows.map(([key, value]) => (
        <div key={key}>
          <dt>{key}</dt>
          <dd>{value}</dd>
        </div>
      ))}
    </dl>
  )
}

export function Section({ title, children, actions }: { title: string; children: ReactNode; actions?: ReactNode }) {
  return (
    <section className="sect">
      <h3>
        <span>{title}</span>
        {actions}
      </h3>
      {children}
    </section>
  )
}

export function Bar({ fraction }: { fraction: number }) {
  return (
    <div className="bar" role="progressbar" aria-valuenow={Math.round(fraction * 100)} aria-valuemin={0} aria-valuemax={100}>
      <i style={{ transform: `translateX(${Math.min(100, Math.max(0, fraction * 100)) - 100}%)` }} />
    </div>
  )
}

export function useSignedIn(): boolean {
  const { client, status } = useConnection()
  return useQuery(statusQuery(client, status)).data.signed_in
}

/** The time, every `every` ms while `live`: what a countdown or a running clock reads. Read again before it's drawn as it goes live. */
export function useNow(live = true, every = 1000): number {
  const [now, setNow] = useState(Date.now)
  useLayoutEffect(() => {
    if (!live) return
    setNow(Date.now())
    const timer = setInterval(() => setNow(Date.now()), every)
    return () => clearInterval(timer)
  }, [live, every])
  return now
}

/** One ask at a time from one control: a second click while the first flies does nothing, before anything renders. */
export function useAsk() {
  const flying = useRef(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<unknown>(null)
  const clear = useCallback(() => setError(null), [])
  const run = useCallback(async <T,>(work: () => Promise<T>): Promise<T | undefined> => {
    if (flying.current) return undefined
    flying.current = true
    setBusy(true)
    setError(null)
    try {
      return await work()
    } catch (failure) {
      setError(failure)
      return undefined
    } finally {
      flying.current = false
      setBusy(false)
    }
  }, [])
  return { run, busy, error, clear }
}

/** A button that first says what it'll do, there under it. Nothing happens but on its yes. */
export function Confirm({
  label,
  says,
  yes,
  danger = true,
  disabled = false,
  onYes,
}: {
  label: string
  says: ReactNode
  yes: string
  danger?: boolean
  disabled?: boolean
  onYes: () => void
}) {
  const [asking, setAsking] = useState(false)
  return (
    <>
      <button type="button" className={`tb ${danger ? "danger" : ""}`} disabled={disabled} aria-expanded={asking} onClick={() => setAsking(!asking)}>
        {label}
      </button>
      {asking && (
        <div className="confirm">
          <div className="says">{says}</div>
          <div className="row end">
            <button type="button" className="tb" onClick={() => setAsking(false)}>
              Not now
            </button>
            <button
              type="button"
              className={`tb ${danger ? "danger" : "primary"}`}
              onClick={() => {
                setAsking(false)
                onYes()
              }}
            >
              {yes}
            </button>
          </div>
        </div>
      )}
    </>
  )
}

/** Lines that follow their end, unless you scrolled up to read. Text only, whatever a package wrote. */
export function LogView({ text, empty }: { text: string; empty: string }) {
  const box = useRef<HTMLPreElement>(null)
  const pinned = useRef(true)
  useLayoutEffect(() => {
    const el = box.current
    if (el && pinned.current) el.scrollTop = el.scrollHeight
  }, [text])
  return (
    <pre
      ref={box}
      className="log"
      onScroll={(event) => {
        const el = event.currentTarget
        pinned.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24
      }}
    >
      {text || <span className="muted">{empty}</span>}
    </pre>
  )
}

/** Text as it would read on a terminal, without its colours. */
export const plain = (text: string) => text.replace(/\x1b\[[0-9;?]*[ -/]*[@-~]/g, "")
