import type { Box } from "./graph"
import { NARROWEST } from "./shown"

/** What a box that gave way is left of the rest, and a box nudged aside of all but it. */
const GAP = 16
const NUDGE_GAP = 8
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
/** A neighbour is nudged this far at most and never shrinks. A pixel of it costs more than one of the box placed, more the further it goes: many small nudges beat one long one. */
const NUDGE_MOST = 40
const NUDGE_RATE = 1.1
const NUDGE_RISE = 1.5
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
  /** Which boxes it lies on may be nudged aside. */
  nudges?: (name: string) => boolean
}

/** Where it goes, how wide, and the boxes nudged aside for it, by name. */
export interface Fitted extends Box {
  nudged: Map<string, Box>
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
  nudged?: Map<string, Box>
}

/**
 * Where `box` goes to lie on nothing, at the least cost: where it is if it can, else the cheapest mix of moving it (by its centre),
 * shrinking it and nudging what it lies on a little, a gap clear of the rest. Places are tried cheapest first, from where it is and
 * beside whatever each came near, the same every time, so two tabs settle a box the same way.
 */
export function fit(box: Box, field: Field, { tall, nudges }: Give = {}): Fitted {
  if (!field.under(box).size) return { ...box, nudged: new Map() }
  const scale = (box.w + box.h) / 2
  const [cx, cy] = [box.x + box.w / 2, box.y + box.h / 2]
  const sizes = sizesOf(box, tall)
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
    if (at.nudged) return { ...here, nudged: at.nudged }
    const hits = field.under(around(here, GAP))
    if (!hits.size) return { ...grown(here, box.w, field, tall), nudged: new Map() }
    const aside = nudges && nudged(here, hits, field, nudges, scale)
    if (aside) heap.push({ ...at, cost: at.cost + aside.cost, nudged: aside.moved })
    if (at.size && at.cost - cost > SHRUNK_NEAR) continue
    for (const hit of hits.values()) {
      look(hit.x - w - GAP, at.y, at.size)
      look(hit.x + hit.w + GAP, at.y, at.size)
      look(at.x, hit.y - h - GAP, at.size)
      look(at.x, hit.y + hit.h + GAP, at.size)
    }
  }
  return { ...field.beside(box), nudged: new Map() }
}

/** Its own size, then each step smaller it may take, with what that costs. */
function sizesOf(box: Box, tall?: (w: number) => number): Size[] {
  const sizes = [{ w: box.w, h: box.h, cost: 0 }]
  const least = Math.max(box.w * SMALLEST, NARROWEST)
  if (!tall || tall(least) >= box.h) return sizes
  for (let step = 1; box.w * (1 - step * STEP) >= least; step++) {
    const w = Math.round(box.w * (1 - step * STEP))
    sizes.push({ w, h: tall(w), cost: SHRINK_AT_ALL + 1 - w / box.w })
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

/** Each box `here` came near, slid the shortest way to a gap from it, if every one can go a little and lands clear of the rest: what that costs. */
function nudged(here: Box, hits: Map<string, Box>, field: Field, may: (name: string) => boolean, scale: number) {
  const moved = new Map<string, Box>()
  let cost = 0
  for (const [name, hit] of [...hits].sort(([a], [b]) => (a < b ? -1 : 1))) {
    if (!may(name)) return null
    const ways = [
      { x: here.x - GAP - hit.w, y: hit.y },
      { x: here.x + here.w + GAP, y: hit.y },
      { x: hit.x, y: here.y - GAP - hit.h },
      { x: hit.x, y: here.y + here.h + GAP },
    ]
      .map((to) => ({ ...hit, ...to, far: Math.abs(to.x - hit.x) + Math.abs(to.y - hit.y) }))
      .filter(({ far }) => far <= NUDGE_MOST)
      .sort((a, b) => a.far - b.far)
    const way = ways.find((to) => {
      const room = around(to, NUDGE_GAP)
      return ![...moved.values()].some((other) => overlap(other, room)) && [...field.under(room).keys()].every((other) => hits.has(other))
    })
    if (!way) return null
    const { far, ...to } = way
    moved.set(name, to)
    cost += (NUDGE_RATE * far) / scale + NUDGE_RISE * (far / scale) ** 2
  }
  return { cost, moved }
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
