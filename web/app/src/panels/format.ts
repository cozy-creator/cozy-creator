// As the CLI says them (`output.rs`, `machine/mod.rs`), so a person reads one thing in both.

export const usd = (micros: number) => `$${(micros / 1e6).toFixed(2)}`

export function bytes(n: number): string {
  const units = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"]
  let value = n
  let unit = 0
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024
    unit += 1
  }
  if (unit === 0) return `${n} B`
  return `${value < 10 ? value.toFixed(1) : value.toFixed(0)} ${units[unit]}`
}

/** A span of seconds: `41s`, `3m12s`, `2h5m`, `1d4h`. */
export function span(s: number): string {
  const whole = Math.max(0, Math.floor(s))
  if (whole < 60) return `${whole}s`
  if (whole < 3600) return `${Math.floor(whole / 60)}m${whole % 60}s`
  if (whole < 86400) return `${Math.floor(whole / 3600)}h${Math.floor((whole % 3600) / 60)}m`
  return `${Math.floor(whole / 86400)}d${Math.floor((whole % 86400) / 3600)}h`
}

/** Time still to go, as a person says it: `40 s left`, `6 min left`, `2 h 5 min left`. */
export function leftWords(s: number): string {
  if (s < 90) return `${Math.max(1, Math.round(s))} s left`
  const minutes = Math.round(s / 60)
  return minutes < 90 ? `${minutes} min left` : `${Math.floor(minutes / 60)} h ${minutes % 60} min left`
}

/** A short time: milliseconds, tenths of a second under a minute, then a span. */
export const ms = (value: number) => (value < 1000 ? `${Math.round(value)} ms` : value < 60_000 ? `${(value / 1000).toFixed(1)}s` : span(value / 1000))

export const when = (at: number) => new Date(at).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" })

/** A Hub timestamp as a person reads it ("Nov 12, 2026, 10:04 AM"), never its raw ISO; what doesn't parse stays as it came. */
export const stamp = (iso: string) => {
  const at = Date.parse(iso)
  return Number.isNaN(at) ? iso : when(at)
}

/** A name as people read it: first-party names drop their `_/`. */
export const shown = (name: string) => name.replace(/^_\//, "")

/** A value as text, with no file's hash in it: the hash is never shown. */
export function masked(value: unknown): string {
  return JSON.stringify(value, (_, held) => (typeof held === "string" && /^sha256:[0-9a-f]{16,}/i.test(held) ? "(a file)" : held), 2) ?? ""
}
