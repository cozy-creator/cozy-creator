import type { Machine } from "../../types"
import { span, usd } from "../format"

/** A rental that doesn't answer yet: nothing else may be ordered until it comes to something. */
export const ordering = (machine: Machine) => machine.kind === "rental" && machine.ordered !== false && machine.state === "booting"

/** What `name` in a route means: a machine's name, former name or rental id. */
export const named = (machine: Machine, name: string) => [machine.name, machine.alias, machine.rental_id].some((known) => known?.toLowerCase() === name.toLowerCase())

/** What it's called in a route that must survive a rename: its rental id, else its name. */
export const stableName = (machine: Machine) => machine.rental_id ?? machine.name

export const gpusWords = (machine: Machine) => {
  const gpus = machine.status?.gpus
  if (gpus?.length) return `${gpus.length}× ${gpus[0].name}`
  const rental = machine.rental
  if (!rental || !rental.gpus || rental.accelerator.toLowerCase() === "cpu") return rental ? "CPU" : ""
  return `${rental.gpus}× ${rental.accelerator}`
}

export function stateWords(machine: Machine, now: number): string {
  if (machine.offline) return `offline: ${machine.offline}`
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

export const stateClass = (machine: Machine) =>
  machine.offline || machine.reason === "release_unconfirmed"
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
  if (deadline !== undefined) return deadline > now ? `ends in ${span((deadline - now) / 1000)} if it stays idle` : "ends now"
  return machine.status && machine.kind === "rental" ? "ends after its jobs" : ""
}

export function spentWords(machine: Machine): string {
  const rental = machine.rental
  if (rental?.spend_usd_micros === undefined) return ""
  const basis = rental.spend_basis === "charged" ? "" : rental.spend_basis === "unmetered" ? " unmetered" : " est."
  return `${usd(rental.spend_usd_micros)}${basis}`
}
