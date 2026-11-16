import type { Machine, MachineStatus } from "../../api/calcifer"
import { Section } from "../common"
import { bytes } from "../format"
import { modelWords } from "./activity"

const GB = 1 << 30

/** What the machine keeps free and never fills, and below which it clears its caches: 1/50 of the disk, 1–10GB (TensorD's
 *  `reclaim.rs` `RESERVE_BYTES`/`RESERVE_DIVISOR`; TensorFS's `fetch/volume.rs` `room` keeps the same beside each download). */
const reserve = (total: number) => Math.min(10 * GB, Math.max(GB, total / 50))

/** Nearly full (cl-1100): under a tenth of it free, or too little past its reserve for another model the size of its biggest, which a
 *  download then makes room for by clearing models nothing is using. Null when it isn't, or doesn't say. */
export function nearlyFull(status: MachineStatus): { free: number; total: number; model?: { name: string; bytes: number } } | null {
  const total = status.disk_total_bytes
  if (!total) return null
  const free = status.disk_free_bytes ?? 0
  const biggest = [...(status.models ?? [])].sort((a, b) => b.total_bytes - a.total_bytes)[0]
  const model = biggest && free < reserve(total) + biggest.total_bytes ? { name: modelWords(biggest.repository), bytes: biggest.total_bytes } : undefined
  return free < total / 10 || model ? { free, total, model } : null
}

const KINDS = [
  ["models", "Models"],
  ["environments", "Packages"],
  ["runtime", "Runtime"],
  ["runs", "Run files"],
  ["other", "Other files"],
] as const

/** Used and free as a bar, by what fills it, and a warning when it's nearly full: Paul 11-16, "making sure disc space isn't too full". */
export function Disk({ machine, status }: { machine: Machine; status: MachineStatus }) {
  const total = status.disk_total_bytes
  if (!total) {
    return (
      <Section title="Disk">
        <p className="muted">It hasn't said how full its disk is yet.</p>
      </Section>
    )
  }
  const free = Math.min(total, status.disk_free_bytes ?? 0)
  const used = total - free
  const known = { models: status.disk_models_bytes ?? 0, environments: status.disk_environments_bytes ?? 0, runtime: status.disk_runtime_bytes ?? 0, runs: status.disk_runs_bytes ?? 0 }
  const measured = Object.values(known).some(Boolean)
  const parts: Record<(typeof KINDS)[number][0], number> = { ...known, other: Math.max(0, used - Object.values(known).reduce((a, b) => a + b, 0)) }
  const full = nearlyFull(status)
  const share = (n: number) => `${Math.min(100, (100 * n) / total)}%`
  return (
    <Section title="Disk">
      <div className={`diskbar ${full ? "full" : ""}`} role="img" aria-label={`${bytes(used)} used, ${bytes(free)} free of ${bytes(total)}`}>
        {measured ? KINDS.map(([kind, label]) => parts[kind] > 0 && <i key={kind} className={`dpart ${kind}`} style={{ width: share(parts[kind]) }} title={`${label}: ${bytes(parts[kind])}`} />) : <i className="dpart other" style={{ width: share(used) }} />}
      </div>
      <p className="disk-sum">
        <b>{bytes(free)} free</b> of {bytes(total)} · {Math.round((100 * used) / total)}% used
      </p>
      {measured ? (
        <ul className="disk-legend">
          {KINDS.map(([kind, label]) => (
            <li key={kind} data-kind={kind}>
              <i className={`dpart ${kind}`} />
              {label} <b>{bytes(parts[kind])}</b>
            </li>
          ))}
        </ul>
      ) : (
        <p className="muted">It hasn't measured what fills its disk yet.</p>
      )}
      {full && (
        <p className="disk-full">
          Nearly full: {bytes(full.free)} free of {bytes(full.total)}
          {full.model ? `, not enough for another model the size of ${full.model.name} (${bytes(full.model.bytes)})` : ""}. Before a download, Cozy clears out
          models nothing is using. If a run still doesn't fit, it won't start: {machine.kind === "rental" ? "rent a machine with a bigger disk" : `free up space on ${machine.kind === "local" ? "this" : "that"} computer`}.
        </p>
      )}
    </Section>
  )
}
