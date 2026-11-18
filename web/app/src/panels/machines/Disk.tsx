import type { Machine, MachineStatus } from "../../api/calcifer"
import { Section } from "../common"
import { bytes } from "../format"

/** Nearly full (cl-1100, cl-1175): its room for downloads (free past what Cozy keeps free, with the models nothing uses, which a download
 *  clears) is less than its usual models need, as Calcifer judges it. Null when it isn't, or doesn't say. */
export function nearlyFull(status: MachineStatus): { room: number; needs: number } | null {
  if (!status.disk_total_bytes || !status.disk_needs_bytes) return null
  return { room: status.disk_room_bytes ?? 0, needs: status.disk_needs_bytes }
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
          Nearly full: {bytes(full.room)} of room for models, and its usual models need {bytes(full.needs)}. A render whose models don't fit
          waits: {machine.kind === "rental" ? "rent a machine with a bigger disk" : `free up space on ${machine.kind === "local" ? "this" : "that"} computer`}, or render on a new rental.
        </p>
      )}
    </Section>
  )
}
