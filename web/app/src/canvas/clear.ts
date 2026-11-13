import type { Box } from "./graph"
import { GAP, NARROWEST } from "./shown"

const CELL = 512
export const GLIDE_MS = 180

// What giving way costs, in the size of the box placed (its width and height averaged): moving it that far costs 1.
/** Shrinking at all costs this, and then the fraction it shrinks by: a short move beats any shrink, a long one doesn't. */
const SHRINK_AT_ALL = 0.15
/** It shrinks in these steps, to half its width at most, and never narrower than its corner pulls: then it grows back as far as it fits. */
const STEP = 0.05
const SMALLEST = 0.5
/** A smaller size is looked for no further off than this: past it, only its own size. */
const SHRUNK_NEAR = 1.5
/** Placed, it shrinks this far at most to push less, and shrinking at all costs this, in its size, then the fraction it shrinks by: a pixel pushed costs about 1. */
const PUSHED_SMALLEST = 0.8
const SHRINK_TO_PUSH_LESS = 0.25
/** A box pushed costs how far it goes, more the further (twice as much a pixel this far), and this many pixels besides: many short pushes
 *  down a line beat one long one, and fewer boxes moved beats as many moved less. */
const PUSHED_FAR = 100
const PUSHED_EACH = 4
/** A box goes the way it's least in the way of, out of where it lies, unless another way moves everything this many times less: a row
 *  stays a row, and only a long line pushed sends the first of it off the line instead. */
const ASIDE = 3
/** Past this many boxes, or this many looks, the push goes too far: it can't stay there. */
const PUSH_MOST = 60
const PUSH_LOOKS = 2000
/** Past this many places looked at, nowhere near will do: it goes beside everything. */
const LOOKS = 5000

const overlap = (a: Box, b: Box) => a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
const around = (box: Box, by: number) => ({ x: box.x - by, y: box.y - by, w: box.w + 2 * by, h: box.h + 2 * by })

/** Boxes by where they are, so what's under one costs what's near it, however many there are. */
export class Field {
  private cells = new Map<string, Map<string, Box>>()
  private boxes = new Map<string, Box>()
  /** Everything lies inside this. */
  private edges = { left: Infinity, top: Infinity, right: -Infinity, bottom: -Infinity }

  private *cellsOf(box: Box) {
    for (let x = Math.floor(box.x / CELL); x <= Math.floor((box.x + box.w) / CELL); x++) {
      for (let y = Math.floor(box.y / CELL); y <= Math.floor((box.y + box.h) / CELL); y++) yield `${x}:${y}`
    }
  }

  /** It's here now, wherever it was. */
  add(name: string, box: Box) {
    this.remove(name)
    this.boxes.set(name, box)
    for (const key of this.cellsOf(box)) {
      let cell = this.cells.get(key)
      if (!cell) this.cells.set(key, (cell = new Map()))
      cell.set(name, box)
    }
    const { edges } = this
    edges.left = Math.min(edges.left, box.x)
    edges.top = Math.min(edges.top, box.y)
    edges.right = Math.max(edges.right, box.x + box.w)
    edges.bottom = Math.max(edges.bottom, box.y + box.h)
  }

  /** Where it is. */
  get(name: string): Box | undefined {
    return this.boxes.get(name)
  }

  remove(name: string) {
    const box = this.boxes.get(name)
    if (!box) return
    this.boxes.delete(name)
    for (const key of this.cellsOf(box)) this.cells.get(key)?.delete(name)
  }

  /** What `box` lies on, each once, by name. */
  under(box: Box): Map<string, Box> {
    const found = new Map<string, Box>()
    for (const key of this.cellsOf(box)) {
      for (const [name, other] of this.cells.get(key) ?? []) if (overlap(box, other)) found.set(name, other)
    }
    return found
  }

  /** The nearest place outside everything, which is always clear. */
  beside(box: Box): Box {
    const { left, top, right, bottom } = this.edges
    const ways = [
      { ...box, x: right + GAP },
      { ...box, x: left - box.w - GAP },
      { ...box, y: bottom + GAP },
      { ...box, y: top - box.h - GAP },
    ]
    return ways.reduce((best, way) => (Math.hypot(way.x - box.x, way.y - box.y) < Math.hypot(best.x - box.x, best.y - box.y) ? way : best))
  }
}

/** How a box may give way, besides moving. */
export interface Give {
  /** Its height at a width: given, it may shrink to fit, keeping its picture's shape. */
  tall?: (w: number) => number
  /** Given, it stays where it is and pushes aside what it lies on, these boxes, unless one that can't be is in the way. */
  pushes?: (name: string) => boolean
  /** Where it was, as it grows: what was closer to it than a gap stays that close. */
  was?: Box
}

/** Where it goes, how wide, and the boxes pushed aside for it, by name. */
export interface Fitted extends Box {
  pushed: Map<string, Box>
  /** Nowhere near had room: it's beside everything. */
  beside?: true
}

interface Size {
  w: number
  h: number
  cost: number
}

interface Look {
  cost: number
  x: number
  y: number
  /** Which size, largest first. */
  size: number
}

/**
 * Where `box` goes to lie on nothing. Pushing, it stays (a little smaller if that pushes much less) and what it lies on goes aside, the least
 * that clears it: see `shove`. Otherwise, or with something in the way that can't be pushed, the cheapest mix of moving it (by its centre)
 * and shrinking it, a gap clear of the rest. Places are tried cheapest first, from where it is and beside whatever each came near, the
 * same every time, so two tabs settle a box the same way.
 */
export function fit(box: Box, field: Field, { tall, pushes, was }: Give = {}): Fitted {
  // Growing, what it comes within a gap of goes as it comes, not once it's on it.
  if (!was && !field.under(box).size) return { ...box, pushed: new Map() }
  const stays = pushes && stay(box, field, pushes, tall, was)
  if (stays) return stays
  const scale = (box.w + box.h) / 2
  const [cx, cy] = [box.x + box.w / 2, box.y + box.h / 2]
  const sizes = sizesOf(box, tall, SMALLEST, SHRINK_AT_ALL)
  const heap = new Heap()
  const seen = new Set<string>()
  const look = (x: number, y: number, size: number) => {
    const key = `${size}:${x}:${y}`
    if (seen.has(key)) return
    seen.add(key)
    const { w, h, cost } = sizes[size]
    heap.push({ cost: Math.hypot(x + w / 2 - cx, y + h / 2 - cy) / scale + cost, x, y, size })
  }
  sizes.forEach(({ w, h }, size) => look(Math.round(cx - w / 2), Math.round(cy - h / 2), size))
  for (let looks = 0; looks < LOOKS; looks++) {
    const at = heap.pop()
    if (!at) break
    const { w, h, cost } = sizes[at.size]
    const here = { x: at.x, y: at.y, w, h }
    const hits = field.under(around(here, GAP))
    if (!hits.size) return { ...grown(here, box.w, field, tall), pushed: new Map() }
    if (at.size && at.cost - cost > SHRUNK_NEAR) continue
    for (const hit of hits.values()) {
      look(hit.x - w - GAP, at.y, at.size)
      look(hit.x + hit.w + GAP, at.y, at.size)
      look(at.x, hit.y - h - GAP, at.size)
      look(at.x, hit.y + hit.h + GAP, at.size)
    }
  }
  return { ...field.beside(box), pushed: new Map(), beside: true }
}

/** What `box`, growing from `was` where it is, pushes aside, as `fit` would: none if something that can't be pushed is in the way. */
export function pushAside(box: Box, field: Field, may: (name: string) => boolean, was?: Box): Map<string, Box> {
  return shove(box, field, may, was)?.moved ?? new Map()
}

/** Where it was put, at its own size or a little smaller about its centre, with what that pushes aside: whichever moves least, a shrink costed as pixels. */
function stay(box: Box, field: Field, may: (name: string) => boolean, tall?: (w: number) => number, was?: Box): Fitted | null {
  const scale = (box.w + box.h) / 2
  const [cx, cy] = [box.x + box.w / 2, box.y + box.h / 2]
  let best: { fitted: Fitted; cost: number } | null = null
  for (const [size, { w, h, cost }] of sizesOf(box, tall, PUSHED_SMALLEST, SHRINK_TO_PUSH_LESS).entries()) {
    const here = size ? { x: Math.round(cx - w / 2), y: Math.round(cy - h / 2), w, h } : box
    const pushed = shove(here, field, may, was)
    if (!pushed) continue
    const total = pushed.cost + cost * scale
    if (!best || total < best.cost) best = { fitted: { ...(pushed.moved.size ? here : grown(here, box.w, field, tall)), pushed: pushed.moved }, cost: total }
  }
  return best?.fitted ?? null
}

/** Its own size, then each step smaller it may take down to `smallest` of it, with what that costs. */
function sizesOf(box: Box, tall: ((w: number) => number) | undefined, smallest: number, atAll: number): Size[] {
  const sizes = [{ w: box.w, h: box.h, cost: 0 }]
  const least = Math.max(box.w * smallest, NARROWEST)
  if (!tall || tall(least) >= box.h) return sizes
  for (let step = 1; box.w * (1 - step * STEP) >= least - 0.5; step++) {
    const w = Math.round(box.w * (1 - step * STEP))
    sizes.push({ w, h: tall(w), cost: atAll + 1 - w / box.w })
  }
  return sizes
}

/** Shrunk to fit, it grows back about its centre as far as it stays clear by a gap, up to its own width. */
function grown(box: Box, most: number, field: Field, tall?: (w: number) => number): Box {
  if (!tall || box.w >= most) return box
  const [cx, cy] = [box.x + box.w / 2, box.y + box.h / 2]
  const at = (w: number) => {
    const h = tall(w)
    return { x: Math.round(cx - w / 2), y: Math.round(cy - h / 2), w, h }
  }
  const clear = (w: number) => !field.under(around(at(w), GAP)).size
  let [fits, not] = [box.w, most + 1]
  while (not - fits > 1) {
    const mid = Math.floor((fits + not) / 2)
    if (clear(mid)) fits = mid
    else not = mid
  }
  return at(fits)
}

// Pushing aside: right, left, down, up.
const WAYS = [0, 1, 2, 3] as const
type Way = (typeof WAYS)[number]

/** How far `b` must go `way` to be `sep` clear of `a` that way; none or less, it's clear. */
function short(a: Box, b: Box, way: Way, sep: number): number {
  if (way === 0) return a.x + a.w + sep - b.x
  if (way === 1) return b.x + b.w + sep - a.x
  if (way === 2) return a.y + a.h + sep - b.y
  return b.y + b.h + sep - a.y
}

/** What's left between `a` and `b` going `way`, kept as it is if it's under a gap. */
const sepOf = (a: Box, b: Box, way: Way) => Math.min(GAP, Math.max(0, -short(a, b, way, 0)))
const shifted = (box: Box, way: Way, by: number): Box =>
  way < 2 ? { ...box, x: box.x + (way === 0 ? by : -by) } : { ...box, y: box.y + (way === 2 ? by : -by) }
/** Further along `way` than `b`, by their middles. */
const ahead = (a: Box, b: Box, way: Way) =>
  way < 2 ? (a.x + a.w / 2 - b.x - b.w / 2) * (way === 0 ? 1 : -1) > 0 : (a.y + a.h / 2 - b.y - b.h / 2) * (way === 2 ? 1 : -1) > 0
/** The room a box going `way` comes into, a gap ahead of it. */
const reach = (box: Box, way: Way): Box =>
  way === 0 ? { ...box, w: box.w + GAP } : way === 1 ? { ...box, x: box.x - GAP, w: box.w + GAP } : way === 2 ? { ...box, h: box.h + GAP } : { ...box, y: box.y - GAP, h: box.h + GAP }

/** What a push moves, by name, and what that costs. */
interface Shoved {
  moved: Map<string, Box>
  cost: number
}

/**
 * Push and shove, as a router shoves a track: `box` stays, and each box it lies on, or comes within a gap of, goes the way it's least in
 * the way of, or another if that moves everything much less (`ASIDE`). A box that goes one way pushes what's then in its way that way too, as little as clears it, and so on down the line. A
 * box only goes further the way it went, so nothing swings back and forth, and only what the push reaches is looked at. What was closer
 * than a gap, to `was` or to a box pushed, stays that close. Null if it reaches a box that can't be pushed and lies on it, or goes too far.
 */
function shove(box: Box, field: Field, may: (name: string) => boolean, was?: Box): Shoved | null {
  let plan: Shoved = { moved: new Map(), cost: 0 }
  for (const [name, hit] of [...field.under(around(box, GAP))].sort(([a], [b]) => (a < b ? -1 : 1))) {
    // Lying on it before it grew, it lies on it as it did.
    if (was && overlap(was, hit)) continue
    const now = plan.moved.get(name) ?? hit
    const needs = WAYS.map((way) => Math.ceil(short(box, now, way, was ? sepOf(was, hit, way) : GAP)))
    if (Math.min(...needs) <= 0) continue
    if (!may(name)) {
      if (overlap(box, now)) return null
      continue
    }
    const least = needs.indexOf(Math.min(...needs))
    let best: { plan: Shoved; cost: number } | null = null
    for (const way of WAYS) {
      const tried = push(plan, name, way, needs[way], field, may)
      if (!tried) continue
      const cost = (tried.cost - plan.cost) * (way === least ? 1 : ASIDE)
      if (!best || cost < best.cost) best = { plan: tried, cost }
    }
    if (!best) return null
    plan = best.plan
  }
  return clearOfAll(plan.moved, box, field, was) ? plan : null
}

/** `plan`, and `first` pushed `by` going `way`, with all that pushes in turn. */
function push(plan: Shoved, first: string, way: Way, by: number, field: Field, may: (name: string) => boolean): Shoved | null {
  const moved = new Map(plan.moved)
  const queue: string[] = []
  const go = (name: string, by: number) => {
    const from = field.get(name)!
    const now = moved.get(name) ?? from
    // Only ever further the way it went: never back.
    if (way === 0 ? now.x < from.x : way === 1 ? now.x > from.x : way === 2 ? now.y < from.y : now.y > from.y) return false
    moved.set(name, shifted(now, way, by))
    queue.push(name)
    return true
  }
  if (!go(first, by)) return null
  for (let looks = 0; queue.length; looks++) {
    if (looks > PUSH_LOOKS || moved.size > PUSH_MOST) return null
    const name = queue.shift()!
    const now = moved.get(name)!
    const from = field.get(name)!
    const room = reach(now, way)
    const near = new Map([...field.under(room)].filter(([other]) => !moved.has(other)))
    for (const [other, at] of moved) if (overlap(room, at)) near.set(other, at)
    for (const [other, at] of near) {
      const start = field.get(other)!
      if (other === name || !ahead(start, from, way)) continue
      // Two that lay on each other still do, as much.
      const need = Math.ceil(short(now, at, way, sepOf(from, start, way)) - (overlap(from, start) ? Math.max(0, short(from, start, way, 0)) : 0))
      if (need <= 0) continue
      if (!may(other)) {
        if (overlap(now, at)) return null
        continue
      }
      if (!go(other, need)) return null
    }
  }
  let cost = 0
  for (const [name, at] of moved) {
    const from = field.get(name)!
    const far = Math.abs(at.x - from.x) + Math.abs(at.y - from.y)
    cost += far + far ** 2 / PUSHED_FAR + PUSHED_EACH
  }
  return { moved, cost }
}

/** Nothing moved lies on `box`, on another moved, or on anything left where it was, unless it lay on that before. */
function clearOfAll(moved: Map<string, Box>, box: Box, field: Field, was?: Box): boolean {
  const all = [...moved]
  const before = (name: string, other: string) => overlap(field.get(name)!, field.get(other)!)
  return all.every(
    ([name, at], i) =>
      (!overlap(at, box) || Boolean(was && overlap(was, field.get(name)!))) &&
      all.slice(i + 1).every(([other, there]) => !overlap(at, there) || before(name, other)) &&
      [...field.under(at).keys()].every((other) => other === name || moved.has(other) || before(name, other)),
  )
}

/** The cheapest look first; ties go to the larger size, then the same place every time. */
class Heap {
  private looks: Look[] = []

  private static before(a: Look, b: Look) {
    return a.cost - b.cost || a.size - b.size || a.y - b.y || a.x - b.x
  }

  push(look: Look) {
    const { looks } = this
    looks.push(look)
    for (let at = looks.length - 1; at > 0; ) {
      const up = (at - 1) >> 1
      if (Heap.before(looks[up], looks[at]) <= 0) break
      ;[looks[up], looks[at]] = [looks[at], looks[up]]
      at = up
    }
  }

  pop(): Look | undefined {
    const { looks } = this
    const top = looks[0]
    const last = looks.pop()
    if (!looks.length || !last) return top
    looks[0] = last
    for (let at = 0; ; ) {
      const [left, right] = [2 * at + 1, 2 * at + 2]
      let least = at
      if (left < looks.length && Heap.before(looks[left], looks[least]) < 0) least = left
      if (right < looks.length && Heap.before(looks[right], looks[least]) < 0) least = right
      if (least === at) break
      ;[looks[least], looks[at]] = [looks[at], looks[least]]
      at = least
    }
    return top
  }
}
