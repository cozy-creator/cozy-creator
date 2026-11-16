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

/** Its state in a word or two, with why where that's known: `Ready`, `Starting up (pulling the image)`, `Offline (its provider stopped
 *  it)`. */
export function stateWords(machine: Machine, now: number): string {
  if (machine.offline) return `Offline (${machine.offline})`
  if (machine.broken && machine.state !== "ended") return `Not working (${machine.broken.message.replace(/\.$/, "")})`
  const unverified = machine.unverified ? " (may be out of date)" : ""
  switch (machine.state) {
    case "booting":
      return `Starting up${machine.boot ? ` (${machine.boot})` : ""}${unverified}`
    case "ready": {
      const since = machine.rental?.unreachable_since_ms
      if (since) return `The Hub hasn't reached it for ${span((now - since) / 1000)}${unverified}`
      if (machine.unreachable_since_ms) return `Not responding for ${span((now - machine.unreachable_since_ms) / 1000)}${unverified}`
      return `Ready${unverified}`
    }
    case "ending":
      return "Ending"
    case "ended":
      return "Ended"
    case "stopped":
      return "Stopped, starts when needed"
    default:
      return "State unknown"
  }
}

/** Why nothing runs on it, and what becomes of it (cl-977). */
export function brokenWords(machine: Machine): string {
  const broken = machine.broken
  if (!broken || machine.state === "ended") return ""
  const then =
    machine.kind !== "rental"
      ? "Cozy won't send it work. It stops after 2 idle minutes, and checks itself again the next time it's needed."
      : machine.status?.phase === "releasing"
        ? "It's ending itself."
        : broken.code === "silent_after_update" && machine.ordered !== false
          ? "Cozy is ending it, since it costs money and can't do any work."
          : "Cozy won't send it work, and it ends itself once nothing is running on it."
  return `This machine can't run anything right now: ${broken.message.replace(/\.$/, "")}. ${then}`
}

/** What makes its runs slow now (cl-913): a GPU throttled for heat, a Runtime without its kernels. Red wherever it shows. */
export function slowWords(machine: Machine): string[] {
  const status = machine.status
  if (!status || machine.state === "ended") return []
  const heat = (status.heat ?? []).map(
    (hot) =>
      `GPU ${hot.gpu} slowed down from heat for ${Math.round(hot.share * 100)}% of its last run (${hot.temperature_max_c} °C, ${hot.sm_mhz_mean} of ${hot.sm_mhz_limit} MHz)${hot.replayed ? " (replayed readings)" : ""}`,
  )
  const kernels = status.runtime_kernels ? [`Runtime ${status.runtime} is missing its fast GPU kernels, so runs here are slower: ${status.runtime_kernels}`] : []
  return [...heat, ...kernels]
}

export const stateClass = (machine: Machine) =>
  machine.offline || machine.end_failed || machine.end_unconfirmed || (machine.broken && machine.state !== "ended")
    ? "s-bad"
    : machine.state === "ready"
      ? "s-current"
      : machine.state === "booting" || machine.state === "ending"
        ? "s-making"
        : machine.state === "stopped"
          ? "s-missing"
          : "s-queued"

export const kindWords = (machine: Machine) =>
  machine.kind === "local" ? "This computer" : machine.kind === "added" ? "Added computer" : machine.ordered === false ? "Rental (rented by another computer)" : "Rental"

/** When an idle machine ends itself; a rental with work names no deadline and ends after it. */
export function endsWords(machine: Machine, now: number): string {
  const deadline = machine.status?.idle_deadline_ms
  if (deadline != null) return deadline > now ? `Ends in ${span((deadline - now) / 1000)} if it stays idle` : "Ending now"
  return machine.status && machine.kind === "rental" ? "Ends 15 minutes after its last run" : ""
}

/** What it has cost so far: `$1.20 so far`, `About $1.20 so far` while that's an estimate. */
export function spentWords(machine: Machine): string {
  const rental = machine.rental
  if (rental?.spend_usd_micros == null) return ""
  const spent = usd(rental.spend_usd_micros)
  return rental.spend_basis === "charged" ? `${spent} so far` : rental.spend_basis === "unmetered" ? `${spent} so far (not metered)` : `About ${spent} so far`
}
