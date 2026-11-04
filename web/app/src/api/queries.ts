import { queryOptions } from "@tanstack/react-query"
import type { ProjectState, Run, RunList, Status } from "../types"
import type { Client, RunFilter } from "./client"

/** Keyed by name, so an asset that didn't change keeps its identity whatever moved around it. */
function keepAssets(old: unknown, next: unknown): unknown {
  const before = old as ProjectState | undefined
  const after = next as ProjectState
  if (!before) return after
  const known = new Map(before.assets.map((asset) => [asset.name, asset]))
  const assets = after.assets.map((asset) => {
    const was = known.get(asset.name)
    return was && JSON.stringify(was) === JSON.stringify(asset) ? was : asset
  })
  return { ...after, assets }
}

export const projectQuery = (client: Client, id: string) =>
  queryOptions({
    queryKey: ["project", id],
    queryFn: ({ signal }) => client.project(id, signal),
    structuralSharing: keepAssets,
  })

export const canvasQuery = (client: Client, id: string) =>
  queryOptions({
    queryKey: ["canvas", id],
    queryFn: ({ signal }) => client.canvas(id, signal),
    structuralSharing: false,
  })

export const yamlQuery = (client: Client, id: string) =>
  queryOptions({
    queryKey: ["yaml", id],
    queryFn: ({ signal }) => client.yaml(id, signal),
  })

export const projectsQuery = (client: Client) =>
  queryOptions({
    queryKey: ["projects"],
    queryFn: () => client.projects(),
  })

export const runsQuery = (client: Client, filter: RunFilter, limit = 50) =>
  queryOptions({
    queryKey: ["runs", filter, limit],
    queryFn: ({ signal }) => client.runs(filter, limit, signal),
  })

/** Kept live by `run.updated`, which carries a run's state, progress and outputs. */
export const runQuery = (client: Client, number: number) =>
  queryOptions({
    queryKey: ["run", number],
    queryFn: ({ signal }) => client.run(number, signal),
  })

/** What only `?detail=full` says, read again when `settled` moves: its report comes once it ended. */
export const runFullQuery = (client: Client, number: number, settled: number) =>
  queryOptions({
    queryKey: ["run", number, "full", settled],
    queryFn: ({ signal }) => client.runFull(number, signal),
  })

export const machinesQuery = (client: Client) =>
  queryOptions({
    queryKey: ["machines"],
    queryFn: () => client.machines(),
  })

/** One machine that isn't in the list: an ended one's record. Reading a live one's would start `local`. */
export const machineQuery = (client: Client, name: string) =>
  queryOptions({
    queryKey: ["machine", name],
    queryFn: ({ signal }) => client.machine(name, signal),
  })

export const rentalSkusQuery = (client: Client) =>
  queryOptions({
    queryKey: ["rental-skus"],
    queryFn: ({ signal }) => client.rentalSkus(signal),
  })

/** `list` as `changed` leaves it, and whether it must be read again: a run it doesn't hold may be new to it. */
export function runsAfter(list: RunList, filter: RunFilter, changed: Run[]): { list: RunList; again: boolean } {
  const byNumber = new Map(changed.map((run) => [run.number, run]))
  const held = new Set(list.runs.map((run) => run.number))
  const lowest = Math.min(...list.runs.map((run) => run.number))
  const belongs = (run: Run) => (!filter.status || run.status === filter.status) && (!filter.machine || run.machine === filter.machine) && (!filter.project || run.project_id === filter.project)
  const runs = list.runs.map((run) => byNumber.get(run.number) ?? run).filter(belongs)
  return { list: { ...list, runs }, again: changed.some((run) => !held.has(run.number) && belongs(run) && (!list.older || run.number > lowest)) }
}

/** Whether this computer is signed in moves when a person signs in or out, and with each `hello`. */
export const statusQuery = (client: Client, initial: Status) =>
  queryOptions({
    queryKey: ["status"],
    queryFn: () => client.status(),
    initialData: initial,
  })
