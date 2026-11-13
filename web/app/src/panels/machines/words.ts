import type { Machine } from "../../api/calcifer"
import { bytes, span, usd } from "../format"

/** What `name` in a route means: a machine's name, former name or rental id. */
export const named = (machine: Machine, name: string) => [machine.name, machine.alias, machine.rental_id].some((known) => known?.toLowerCase() === name.toLowerCase())

/** What people see it called: its name, or until its Hub names the host a rental is placed on, what's being rented (cl-888). */
export const called = (machine: Machine) => machine.placeholder ?? machine.name

/** What a command names it by: its name, or while it has none, its rental id. */
export const handle = (machine: Machine) => (machine.placeholder ? (machine.rental_id ?? machine.name) : machine.name)

/** What it's called in a route that must survive a rename: its rental id, else its name. */
export const stableName = (machine: Machine) => machine.rental_id ?? machine.name

export const gpusWords = (machine: Machine) => {
  const gpus = machine.status?.gpus
  if (gpus?.length) return `${gpus.length}× ${gpus[0].name}${gpus[0].memory_bytes ? ` · ${bytes(gpus[0].memory_bytes)}` : ""}`
  const rental = machine.rental
  if (!rental || !rental.gpus || rental.accelerator.toLowerCase() === "cpu") return rental ? "CPU" : ""
  return `${rental.gpus}× ${rental.accelerator}`
}

export function stateWords(machine: Machine, now: number): string {
  if (machine.offline) return `offline: ${machine.offline}`
  if (machine.broken && machine.state !== "ended") return `broken: ${machine.broken.code}`
  const unverified = machine.unverified ? " (unverified)" : ""
  switch (machine.state) {
    case "booting":
      return `booting${machine.boot ? ` · ${machine.boot}` : ""}${unverified}`
    case "ready": {
      const since = machine.rental?.unreachable_since_ms
      if (since) return `the Hub can't reach it, for ${span((now - since) / 1000)}${unverified}`
      if (machine.unreachable_since_ms) return `ready, not answering for ${span((now - machine.unreachable_since_ms) / 1000)}${unverified}`
      return `ready${unverified}`
    }
    case "ending":
      return "ending"
    case "ended":
      return "ended"
    case "stopped":
      return "stopped, starts when used"
    default:
      return "unknown"
  }
}

/** Why nothing runs on it, and what becomes of it (cl-977). */
export function brokenWords(machine: Machine): string {
  const broken = machine.broken
  if (!broken || machine.state === "ended") return ""
  const then =
    machine.kind !== "rental"
      ? "It takes no work until what's stuck exits; a reboot ends it."
      : machine.status?.phase === "releasing"
        ? "It's releasing itself."
        : broken.code === "silent_after_update" && machine.ordered !== false
          ? "This computer ends it, as it bills for nothing."
          : "It takes no work, and releases itself once nothing runs there."
  return `It's broken: ${broken.message}. ${then}`
}

/** What makes its runs slow now (cl-913): a GPU throttled for heat, a Runtime without its kernels. Red wherever it shows. */
export function slowWords(machine: Machine): string[] {
  const status = machine.status
  if (!status || machine.state === "ended") return []
  const heat = (status.heat ?? []).map(
    (hot) =>
      `GPU ${hot.gpu} ran throttled for heat ${Math.round(hot.share * 100)}% of its last run (${hot.temperature_max_c} °C, ${hot.sm_mhz_mean} of ${hot.sm_mhz_limit} MHz)${hot.replayed ? " (replayed readings)" : ""}`,
  )
  const kernels = status.runtime_kernels ? [`Its Runtime ${status.runtime} serves no native kernels: ${status.runtime_kernels}`] : []
  return [...heat, ...kernels]
}

export const stateClass = (machine: Machine) =>
  machine.offline || machine.end_failed || (machine.broken && machine.state !== "ended")
    ? "s-bad"
    : machine.state === "ready"
      ? "s-current"
      : machine.state === "booting" || machine.state === "ending"
        ? "s-making"
        : machine.state === "stopped"
          ? "s-missing"
          : "s-queued"

export const kindWords = (machine: Machine) =>
  machine.kind === "rental" && machine.ordered === false ? "rental, another computer's" : machine.kind

/** When an idle machine ends itself; a rental with work names no deadline and ends after it. */
export function endsWords(machine: Machine, now: number): string {
  const deadline = machine.status?.idle_deadline_ms
  if (deadline != null) return deadline > now ? `ends in ${span((deadline - now) / 1000)} if it stays idle` : "ends now"
  return machine.status && machine.kind === "rental" ? "ends 15 min after its last job" : ""
}

export function spentWords(machine: Machine): string {
  const rental = machine.rental
  if (rental?.spend_usd_micros == null) return ""
  const basis = rental.spend_basis === "charged" ? "" : rental.spend_basis === "unmetered" ? " unmetered" : " est."
  return `${usd(rental.spend_usd_micros)}${basis}`
}
