import type { Box } from "./graph"

/** What a box moved out of another's way is left of it. */
const GAP = 16
const CELL = 512
const BEYOND = 2000
export const GLIDE_MS = 180

export const overlap = (a: Box, b: Box) => a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h

/** Boxes by where they are, so what's under one costs what's near it, however many there are. */
export class Field {
  private cells = new Map<string, Map<string, Box>>()

  private *cellsOf(box: Box) {
    for (let x = Math.floor(box.x / CELL); x <= Math.floor((box.x + box.w) / CELL); x++) {
      for (let y = Math.floor(box.y / CELL); y <= Math.floor((box.y + box.h) / CELL); y++) yield `${x}:${y}`
    }
  }

  add(name: string, box: Box) {
    for (const key of this.cellsOf(box)) {
      let cell = this.cells.get(key)
      if (!cell) this.cells.set(key, (cell = new Map()))
      cell.set(name, box)
    }
  }

  remove(name: string, box: Box) {
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
}

/**
 * Where `box` goes to lie on nothing: where it is if it can, else the nearest place, by how far it moves, beside what it lay on. Nearest first and over a finite set
 * of edges, the same every time, so two tabs settle a box the same way. `inside` keeps it there if it can.
 */
export function clearSpot(box: Box, field: Field, inside?: Box): { x: number; y: number } {
  if (!field.under(box).size) return { x: box.x, y: box.y }
  const seen = new Set<string>()
  const near = (x: number, y: number) => Math.hypot(x - box.x, y - box.y)
  const tries = [{ x: box.x, y: box.y, off: 0 }]
  const fits = (x: number, y: number) => !inside || (x >= inside.x && y >= inside.y && x + box.w <= inside.x + inside.w && y + box.h <= inside.y + inside.h)
  let outside: { x: number; y: number; off: number } | null = null
  while (tries.length) {
    const at = tries.reduce((best, one, i) => (one.off < tries[best].off ? i : best), 0)
    const { x, y, off } = tries.splice(at, 1)[0]
    // Everything inside is taken: the nearest place past it will do.
    if (outside && off > outside.off + BEYOND) break
    const hits = field.under({ ...box, x, y })
    if (!hits.size) {
      if (fits(x, y)) return { x, y }
      outside ??= { x, y, off }
      continue
    }
    for (const hit of hits.values()) {
      const steps = [[hit.x - box.w - GAP, y], [hit.x + hit.w + GAP, y], [x, hit.y - box.h - GAP], [x, hit.y + hit.h + GAP]]
      for (const [nx, ny] of steps) {
        const key = `${nx}:${ny}`
        if (seen.has(key)) continue
        seen.add(key)
        tries.push({ x: nx, y: ny, off: near(nx, ny) })
      }
    }
  }
  return outside ?? { x: box.x, y: box.y }
}
