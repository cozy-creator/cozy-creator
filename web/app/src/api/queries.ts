import { keepPreviousData, queryOptions, type QueryClient } from "@tanstack/react-query"
import type { Asset, Run, RunList, CalciferStatus, ProjectState, ProjectWarning } from "./calcifer"
import type { Project } from "../types"
import type { Client, RunFilter } from "./client"
import type { EventStream } from "./events"
import { nextProjectOrder } from "./projectOrder"

/** Keyed by name, so an asset that didn't change keeps its identity whatever moved around it. */
function keepAssets(old: unknown, next: unknown): unknown {
  const before = old as Project | undefined
  const after = next as Project
  if (!before) return after
  return { ...after, assets: kept(before.assets, after.assets), entities: kept(before.entities ?? [], after.entities ?? []) }
}

function kept<T extends { name: string }>(before: T[], after: T[]): T[] {
  const known = new Map(before.map((row) => [row.name, row]))
  return after.map((row) => {
    const was = known.get(row.name)
    return was && (was === row || JSON.stringify(was) === JSON.stringify(row)) ? was : row
  })
}

/** A state with these assets as they now are, these gone, and with `head`, all that's now said of the project besides them: what it no longer says isn't in it. */
export function withAssets(state: Project, rows: Asset[], gone: Iterable<string>, head?: Project): Project {
  const fresh = new Map(rows.map((row) => [row.name, row]))
  const left = new Set(gone)
  const assets = state.assets.flatMap((row) => {
    if (left.has(row.name)) return []
    const now = fresh.get(row.name)
    fresh.delete(row.name)
    return [now ?? row]
  })
  assets.push(...fresh.values())
  // A head/asset delta cannot claim that every cached asset covers its newer position.
  return { ...(head ?? state), assets, covered: state.covered }
}

const openingWarningsKey = (id: string) => ["project-opening-warnings", id]

function mergeWarnings(...lists: ProjectWarning[][]): ProjectWarning[] {
  return [...new Map(lists.flat().map((warning) => [JSON.stringify([warning.name ?? null, warning.message]), warning])).values()]
}

/** Address lookup can consume one-time upgrade notices before the canvas reads the project. */
export function rememberProjectWarnings(queries: QueryClient, head: ProjectState) {
  const warnings = head.warnings?.filter(({ name }) => !name)
  if (warnings?.length) {
    queries.setQueryData<ProjectWarning[]>(openingWarningsKey(head.id), (held = []) => mergeWarnings(held, warnings))
  }
}

export const projectQuery = (client: Client, stream: EventStream, id: string) =>
  queryOptions({
    queryKey: ["project", id],
    queryFn: async ({ client: queries, signal }) => {
      await stream.whenShown(id, signal)
      signal.throwIfAborted()
      const began = nextProjectOrder()
      const state = await client.project(id, signal)
      signal.throwIfAborted()
      const key = openingWarningsKey(id)
      const opening = queries.getQueryData<ProjectWarning[]>(key)
      if (!opening) return { ...state, began }
      queries.removeQueries({ queryKey: key, exact: true })
      return { ...state, began, warnings: mergeWarnings(state.warnings ?? [], opening) }
    },
    structuralSharing: keepAssets,
  })

/** A list a late Hub left short is asked for again: what it read of the Hub so far is kept there, so it gets further each time. */
export const toolsQuery = (client: Client, q: string, on: string | null, project: string | null) =>
  queryOptions({
    queryKey: ["tools", q, on, project],
    queryFn: ({ signal }) => client.tools(q, on, project, signal),
    staleTime: 60_000,
    placeholderData: keepPreviousData,
    refetchInterval: (query) => (query.state.data?.note ? 10_000 : false),
  })

export const canvasQuery = (client: Client, stream: EventStream, id: string) =>
  queryOptions({
    queryKey: ["canvas", id],
    queryFn: async ({ signal }) => {
      await stream.whenShown(id, signal)
      signal.throwIfAborted()
      const began = performance.now()
      return { ...(await client.canvas(id, signal)), began }
    },
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
export const statusQuery = (client: Client, initial: CalciferStatus) =>
  queryOptions({
    queryKey: ["status"],
    queryFn: () => client.status(),
    initialData: initial,
  })
