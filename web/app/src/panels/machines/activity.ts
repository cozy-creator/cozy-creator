import { useQuery } from "@tanstack/react-query"
import { machinesQuery } from "../../api/queries"
import { useConnection } from "../../app/Connection"
import type { Machine, Operation, Run } from "../../types"
import { bytes, leftWords } from "../format"
import { target } from "../runs/words"

/** One thing a machine is doing: a package installing, weights arriving, a run, or the machine coming up. */
export type Doing =
  | { kind: "install" | "download"; key: string; operation: Operation }
  | { kind: "run"; key: string; run: Run; on?: Operation }
  | { kind: "boot"; key: string; words: string }

/** `qwen/qwen-image-2.1-turbo`, or a Hugging Face source of it, as people say it: `Qwen Image 2.1 Turbo`. Anything else is said as it is. */
export function modelWords(subject: string): string {
  const repository = subject.replace(/^hf:\/\//, "").match(/^[\w.-]+\/([\w.-]+)(@|\/|$)/)?.[1]
  if (!repository) return subject
  return repository
    .split(/[-_]/)
    .filter(Boolean)
    .map((word) => word[0].toUpperCase() + word.slice(1))
    .join(" ")
}

/** A package as a box says it: this computer's own without its `local/`. */
export const packageWords = (pkg: string) => pkg.replace(/^local\//, "")

export const operationName = (operation: Operation) => (operation.kind === "download" ? modelWords(operation.subject) : packageWords(operation.subject))

export const waits = (run: Run) => run.status === "queued" || run.machine_state === "queued"

/** What run `number` waits on there: its install before its weights. */
export const waitsOnOf = (activity: Operation[], number: number) =>
  activity.find((operation) => operation.kind === "install" && operation.runs.includes(number)) ?? activity.find((operation) => operation.runs.includes(number))

/** Its own work under way: begun, waiting on nothing. */
export const working = (run: Run, on?: Operation) => run.started_at_ms != null && !waits(run) && !on

/** What a machine is doing now: each install and download once, however many runs wait on it, then its runs, those going first. */
export function doingOf(machine: Machine, runs: Run[]): Doing[] {
  const activity = machine.status?.activity ?? []
  const booting: Doing[] = machine.state === "booting" ? [{ kind: "boot", key: "boot", words: machine.boot ?? "booting" }] : []
  const operations: Doing[] = [...activity]
    .sort((a, b) => Number(a.kind === "download") - Number(b.kind === "download"))
    .map((operation) => ({ kind: operation.kind, key: operation.id, operation }))
  const mine: Doing[] = runs
    .filter((run) => run.machine === machine.name)
    .map((run) => ({ kind: "run", key: `run ${run.number}`, run, on: waitsOnOf(activity, run.number) }))
  const going = (doing: Doing) => Number(doing.kind === "run" && !working(doing.run, doing.on))
  return [...booting, ...operations, ...mine.sort((a, b) => going(a) - going(b))]
}

/** What a run's row is called: its box, where the open project has it, else what it runs. */
export const runName = (run: Run, boxes: Map<number, string>) => boxes.get(run.number) ?? target(run)

/** How far a download is: `6.6 GiB / 9.4 GiB · 29 MiB/s · 3 min left · 8.8 GiB already here`. */
export function fetchedWords(operation: Operation): string {
  const parts = [operation.bytes_total ? `${bytes(operation.bytes_done)} / ${bytes(operation.bytes_total)}` : bytes(operation.bytes_done)]
  if (operation.bytes_per_s) parts.push(`${bytes(operation.bytes_per_s)}/s`)
  if (operation.eta_s !== undefined) parts.push(leftWords(operation.eta_s))
  if (operation.bytes_here) parts.push(`${bytes(operation.bytes_here)} already here`)
  return parts.join(" · ")
}

export const fractionOf = (operation: Operation) => (operation.bytes_total ? operation.bytes_done / operation.bytes_total : undefined)

/** What `run` waits on, kept by the machines' stream: a box says it while it waits. */
export function useWaitsOn(run: Run | undefined): Operation | undefined {
  const { client } = useConnection()
  const { data } = useQuery({
    ...machinesQuery(client),
    select: (list) => (run ? waitsOnOf(list.machines.find((machine) => machine.name === run.machine)?.status?.activity ?? [], run.number) : undefined),
  })
  return data
}

/** Package `pkg` installing on any machine: a box whose render waits for it before its run is sent says so. */
export function useInstalling(pkg: string | undefined): Operation | undefined {
  const { client } = useConnection()
  const { data } = useQuery({
    ...machinesQuery(client),
    select: (list) => (pkg ? list.machines.flatMap((machine) => machine.status?.activity ?? []).find((operation) => operation.kind === "install" && operation.subject === pkg) : undefined),
  })
  return data
}
