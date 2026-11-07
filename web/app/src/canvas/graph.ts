import type { Entity, ProjectState, Row } from "../types"

// What reads what, from the state alone. It's never drawn: it says where a new name first sits, right of what it reads.

export interface ReadEdge {
  source: string
  target: string
}

function subsOf(entities: Entity[]): Map<string, string> {
  const owner = new Map<string, string>()
  for (const entity of entities) for (const sub of entity.subs ?? []) owner.set(sub.name, entity.name)
  return owner
}

/** Where a name sits: an element, or a sub-entity's row in one. A link of an entity, `<kyogre.image>`, sits where its entity does. */
function anchors(state: ProjectState) {
  const elements = new Set([...state.assets.map((asset) => asset.name), ...(state.entities ?? []).map((entity) => entity.name)])
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

export function readsOf(state: ProjectState): ReadEdge[] {
  const anchor = anchors(state)
  const reads = new Map<string, ReadEdge>()
  const add = (edge: ReadEdge) => {
    if (edge.source !== edge.target) reads.set(`${edge.source}>${edge.target}`, edge)
  }
  for (const asset of state.assets) {
    for (const read of asset.reads ?? []) {
      const from = anchor(read.name)
      if (from) add({ source: from.element, target: asset.name })
    }
  }
  for (const entity of state.entities ?? []) {
    for (const held of [entity, ...(entity.subs ?? [])]) {
      for (const target of Object.values(held.links ?? {})) if (anchor(target)) add({ source: target, target: entity.name })
    }
  }
  return [...reads.values()]
}

/** How many reads lie upstream of each name, along the longest way. */
function depths(names: string[], edges: ReadEdge[]): Map<string, number> {
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
/** Under the room each box keeps for what hangs off it (CanvasStore's `laid`). */
const GAP_Y = 16
const overlaps = (a: Box, b: Box) => a.x < b.x + b.w + GAP_X / 2 && b.x < a.x + a.w + GAP_X / 2 && a.y < b.y + b.h + GAP_Y && b.y < a.y + a.h + GAP_Y

/**
 * A home for each name with none, once: right of what it reads, else by graph depth, each below what's already there. `boxes` is
 * what's already placed, and grows. With `on`, what's on screen as more arrive, what reads nothing takes the first free place there:
 * down a column, then the next, then wherever's free.
 */
export function homesFor(names: string[], edges: ReadEdge[], boxes: Map<string, Box>, size: (name: string) => { w: number; h: number }, on?: Box): Map<string, Row> {
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
    const box: Box = { x: left + (depth.get(name) ?? 0) * (widest + GAP_X), y: top, w, h }
    if (reads.length) {
      box.x = Math.max(...reads.map((read) => read.x + read.w)) + GAP_X
      box.y = reads[0].y
    } else if (on) {
      box.x = on.x
      box.y = on.y
    }
    for (let bump = placed.find((other) => overlaps(box, other)); bump; bump = placed.find((other) => overlaps(box, other))) {
      box.y = bump.y + bump.h + GAP_Y
      if (on && !reads.length && box.y + h > on.y + on.h && box.x + widest + GAP_X + w <= on.x + on.w) {
        box.x += widest + GAP_X
        box.y = on.y
      }
    }
    placed.push(box)
    boxes.set(name, box)
    homes.set(name, { x: Math.round(box.x), y: Math.round(box.y), w })
  }
  return homes
}
