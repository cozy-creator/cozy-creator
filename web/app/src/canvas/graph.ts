import type { Entity, ProjectState, Row } from "../types"

// What the canvas draws of what reads what, from the state alone. An edge goes from what's read to what reads it, so a project reads
// left to right. Solid is a mention, dotted an entity's link, dashed a shot's write.

export type EdgeKind = "mention" | "link" | "write"

export interface EdgeSpec {
  id: string
  kind: EdgeKind
  source: string
  sourceHandle?: string
  target: string
  targetHandle?: string
  /** Whose words made it. */
  reader: string
  /** The name as those words say it. */
  read: string
  /** Where: `summary`, `beats.0.description`, an entity's link. */
  at: string[]
}

export const subSource = (sub: string) => `from:${sub}`
export const subTarget = (sub: string) => `to:${sub}`
export const BELOW = "below"
export const ABOVE = "above"

function subsOf(entities: Entity[]): Map<string, string> {
  const owner = new Map<string, string>()
  for (const entity of entities) for (const sub of entity.subs ?? []) owner.set(sub.name, entity.name)
  return owner
}

/** Where a name sits: an element, or a sub-entity's row in one. A link of an entity, `<kyogre.image>`, sits where its entity does. */
function anchors(state: ProjectState) {
  const elements = new Set([...state.nodes.map((asset) => asset.name), ...(state.entities ?? []).map((entity) => entity.name)])
  const owners = subsOf(state.entities ?? [])
  return (name: string): { element: string; sub?: string } | null => {
    for (let at = name; ; at = at.slice(0, at.lastIndexOf("."))) {
      if (elements.has(at)) return { element: at }
      const owner = owners.get(at)
      if (owner) return { element: owner, sub: at }
      if (!at.includes(".")) return null
    }
  }
}

/** What reading the project said, by the card that shows it: a sub-entity's or a link's on its entity. What's of no card is the project's. */
export function warningsOf(state: ProjectState): { byElement: Map<string, string[]>; ofProject: string[] } {
  const anchor = anchors(state)
  const byElement = new Map<string, string[]>()
  const ofProject: string[] = []
  for (const { name, message } of state.warnings ?? []) {
    const element = name ? anchor(name)?.element : undefined
    if (element) byElement.set(element, [...(byElement.get(element) ?? []), message])
    else ofProject.push(message)
  }
  return { byElement, ofProject }
}

export function edgesOf(state: ProjectState): EdgeSpec[] {
  const anchor = anchors(state)
  const edges = new Map<string, EdgeSpec>()
  const add = (spec: Omit<EdgeSpec, "id">) => {
    if (spec.source === spec.target) return
    const id = `${spec.kind}:${spec.source}/${spec.sourceHandle ?? ""}>${spec.target}/${spec.targetHandle ?? ""}`
    const held = edges.get(id)
    if (held) held.at.push(...spec.at)
    else edges.set(id, { ...spec, id })
  }
  for (const asset of state.nodes) {
    for (const read of asset.reads ?? []) {
      const from = anchor(read.name)
      if (from) add({ kind: "mention", source: from.element, sourceHandle: from.sub && subSource(from.sub), target: asset.name, reader: asset.name, read: read.name, at: [...(read.at ?? [])] })
    }
    if (asset.written_by && anchor(asset.written_by)) {
      add({ kind: "write", source: asset.written_by, sourceHandle: BELOW, target: asset.name, targetHandle: ABOVE, reader: asset.written_by, read: asset.name, at: [] })
    }
  }
  const links = (entity: Entity, element: string, sub?: string) => {
    for (const [field, target] of Object.entries(entity.links ?? {})) {
      if (anchor(target)) add({ kind: "link", source: target, target: element, targetHandle: sub && subTarget(sub), reader: entity.name, read: target, at: [field] })
    }
  }
  for (const entity of state.entities ?? []) {
    links(entity, entity.name)
    for (const sub of entity.subs ?? []) links(sub, entity.name, sub.name)
  }
  return [...edges.values()]
}

/** How many reads lie upstream of each name, along the longest way. */
function depths(names: string[], edges: EdgeSpec[]): Map<string, number> {
  const reads = new Map<string, string[]>()
  for (const edge of edges) {
    const held = reads.get(edge.target)
    if (held) held.push(edge.source)
    else reads.set(edge.target, [edge.source])
  }
  const done = new Map<string, number>()
  const depth = (name: string, seen: Set<string>): number => {
    const known = done.get(name)
    if (known !== undefined) return known
    if (seen.has(name)) return 0
    seen.add(name)
    let deepest = 0
    for (const read of reads.get(name) ?? []) deepest = Math.max(deepest, depth(read, seen) + 1)
    seen.delete(name)
    done.set(name, deepest)
    return deepest
  }
  for (const name of names) depth(name, new Set())
  return done
}

export interface Box {
  x: number
  y: number
  w: number
  h: number
}

const GAP_X = 64
const GAP_Y = 32
const overlaps = (a: Box, b: Box) => a.x < b.x + b.w + GAP_X / 2 && b.x < a.x + a.w + GAP_X / 2 && a.y < b.y + b.h + GAP_Y && b.y < a.y + a.h + GAP_Y

/**
 * A home for each name with none, once: right of what it reads, else by graph depth, each below what's already there. What a shot
 * writes goes under the shot. `boxes` is what's already placed, and grows.
 */
export function homesFor(names: string[], edges: EdgeSpec[], boxes: Map<string, Box>, size: (name: string) => { w: number; h: number }): Map<string, Row> {
  const depth = depths(names, edges)
  const order = names.map((name, at) => ({ name, at })).sort((a, b) => (depth.get(a.name) ?? 0) - (depth.get(b.name) ?? 0) || a.at - b.at)
  const placed = [...boxes.values()]
  const left = placed.length ? Math.min(...placed.map((box) => box.x)) : 0
  const top = placed.length ? Math.min(...placed.map((box) => box.y)) : 0
  const widest = Math.max(240, ...names.map((name) => size(name).w))
  const homes = new Map<string, Row>()
  for (const { name } of order) {
    const { w, h } = size(name)
    const into = edges.filter((edge) => edge.target === name)
    const reads = into.flatMap((edge) => boxes.get(edge.source) ?? [])
    const writer = into.find((edge) => edge.kind === "write")
    const box: Box = { x: left + (depth.get(name) ?? 0) * (widest + GAP_X), y: top, w, h }
    if (writer && boxes.get(writer.source)) {
      const by = boxes.get(writer.source)!
      box.x = by.x
      box.y = by.y + by.h + GAP_Y * 2
    } else if (reads.length) {
      box.x = Math.max(...reads.map((read) => read.x + read.w)) + GAP_X
      box.y = reads[0].y
    }
    for (let bump = placed.find((other) => overlaps(box, other)); bump; bump = placed.find((other) => overlaps(box, other))) {
      if (writer) box.x = bump.x + bump.w + GAP_X / 2
      else box.y = bump.y + bump.h + GAP_Y
    }
    placed.push(box)
    boxes.set(name, box)
    homes.set(name, { x: Math.round(box.x), y: Math.round(box.y), w })
  }
  return homes
}
