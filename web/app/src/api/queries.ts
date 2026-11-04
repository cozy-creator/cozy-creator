import { queryOptions } from "@tanstack/react-query"
import type { ProjectState } from "../types"
import type { Client } from "./client"

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
