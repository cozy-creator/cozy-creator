import type { Card } from "../api/calcifer"
import type { Box } from "./graph"

// Where boxes go that are given no place (cl-1319; Paul, 11-19: "automatically find a home for this box"): what `cozy apply` and another
// writer add, laid out as one block, and the boxes of a drop laid out about its point. One rule for all of them: in their order, in rows
// that wrap, "tiled out both right and left, along a 2D grid", as near a screen's shape as they make ("most people's screens are 16:9").
// The one exception: a single box that reads one already there goes right of it, as the page puts a box made from another (cl-759).

/** Between the columns of boxes laid out together. */
export const GAP_X = 64
/** Under the room each box keeps for what hangs off it (CanvasStore's `laid`). */
export const GAP_Y = 16
/** The shape a block of boxes is laid out nearest to. */
const SCREEN = 16 / 9

export interface Size {
  w: number
  h: number
}
type Point = { x: number; y: number }

/** How far a shape, wide over tall, is from a screen's, wider and taller alike. */
const offScreen = (shape: number) => Math.abs(Math.log(shape / SCREEN))

/** `sizes` in `columns`, row by row from 0,0: each column as wide as its widest, each row as tall as its tallest. */
function inColumns(sizes: Size[], columns: number): { at: Point[]; w: number; h: number } {
  const widths: number[] = []
  const heights: number[] = []
  sizes.forEach(({ w, h }, i) => {
    widths[i % columns] = Math.max(widths[i % columns] ?? 0, w)
    heights[Math.floor(i / columns)] = Math.max(heights[Math.floor(i / columns)] ?? 0, h)
  })
  const lefts = [0]
  for (const w of widths) lefts.push(lefts.at(-1)! + w + GAP_X)
  const tops = [0]
  for (const h of heights) tops.push(tops.at(-1)! + h + GAP_Y)
  return { at: sizes.map((_, i) => ({ x: lefts[i % columns], y: tops[Math.floor(i / columns)] })), w: lefts.at(-1)! - GAP_X, h: tops.at(-1)! - GAP_Y }
}

/** `sizes` in their order, left to right in rows that wrap, with as many columns as bring the whole nearest 16:9: where each goes from the
 *  whole's top left, and the whole's size. */
export function tile(sizes: Size[]): { at: Point[]; w: number; h: number } {
  if (!sizes.length) return { at: [], w: 0, h: 0 }
  const narrowest = Math.min(...sizes.map(({ w }) => w))
  const tallest = Math.max(...sizes.map(({ h }) => h))
  let best = inColumns(sizes, 1)
  for (let columns = 2; columns <= sizes.length; columns++) {
    // Never narrower than this with this many columns or more: once it's past a screen's shape and further from it than the best, no more
    // columns can do better. A count that mixes kinds into every column can be wider than one more, so each count up to there is tried.
    const least = (columns * narrowest + (columns - 1) * GAP_X) / (Math.ceil(sizes.length / columns) * (tallest + GAP_Y))
    if (least > SCREEN && offScreen(least) >= offScreen(best.w / best.h)) break
    const next = inColumns(sizes, columns)
    if (offScreen(next.w / next.h) < offScreen(best.w / best.h)) best = next
  }
  return best
}

/**
 * Where in `on` a `w` by `h` block first lies clear of `placed`: down its left edge, else down the next place a box there ends, left to
 * right. None when nothing in it has room.
 */
function roomIn(on: Box, w: number, h: number, placed: Box[]): Point | undefined {
  const columns = [...new Set([on.x, ...placed.map((box) => box.x + box.w + GAP_X)])].filter((x) => x >= on.x && x + w <= on.x + on.w).sort((a, b) => a - b)
  for (const x of columns) {
    const below = placed.filter((other) => other.x < x + w + GAP_X / 2 && x < other.x + other.w + GAP_X / 2).sort((a, b) => a.y - b.y)
    let y = on.y
    for (const other of below) {
      if (other.y >= y + h + GAP_Y) break
      y = Math.max(y, other.y + other.h + GAP_Y)
    }
    if (y + h <= on.y + on.h) return { x, y }
  }
  return undefined
}

/** Beside everything `placed`, right of it or under it, whichever leaves the whole nearer 16:9; with nothing placed, 0,0. */
function beside(placed: Box[], block: Size): Point {
  if (!placed.length) return { x: 0, y: 0 }
  const left = Math.min(...placed.map((box) => box.x))
  const top = Math.min(...placed.map((box) => box.y))
  const w = Math.max(...placed.map((box) => box.x + box.w)) - left
  const h = Math.max(...placed.map((box) => box.y + box.h)) - top
  const right = offScreen((w + GAP_X + block.w) / Math.max(h, block.h))
  const under = offScreen(Math.max(w, block.w) / (h + GAP_Y + block.h))
  return right <= under ? { x: left + w + GAP_X, y: top } : { x: left, y: top + h + GAP_Y }
}

/** Right of the rightmost of `reads`, at its top, else down that side to the first spot clear of `placed`. */
function rightOf(reads: Box[], { w, h }: Size, placed: Box[]): Point {
  const read = reads.reduce((most, one) => (one.x + one.w > most.x + most.w ? one : most))
  return roomIn({ x: read.x + read.w + GAP_X, y: read.y, w, h: Infinity }, w, h, placed)!
}

/**
 * A home for each name with none, once, clear of `placed`. One alone that reads a box there goes right of it (`rightOf`). Otherwise they're
 * laid out together (`tile`): with `on`, what's on screen, where the whole first lies clear in it, so what arrives while someone looks is
 * where they look (cl-885); else beside everything there.
 */
export function homesFor(names: string[], placed: Map<string, Box>, size: (name: string) => Size, reads: (name: string) => string[], on?: Box): Map<string, Card> {
  const boxes = [...placed.values()]
  const sizes = names.map(size)
  const read = names.length === 1 ? reads(names[0]).flatMap((name) => placed.get(name) ?? []) : []
  if (read.length) {
    const at = rightOf(read, sizes[0], boxes)
    return new Map([[names[0], { x: Math.round(at.x), y: Math.round(at.y), w: sizes[0].w }]])
  }
  const block = tile(sizes)
  const from = (on && roomIn(on, block.w, block.h, boxes)) ?? beside(boxes, block)
  return new Map(names.map((name, i) => [name, { x: Math.round(from.x + block.at[i].x), y: Math.round(from.y + block.at[i].y), w: sizes[i].w }]))
}
