import { ratioOf } from "./shown"

// Film and media's own ratios (Paul, 11-13): video, TV, photo and social as N:M, cinema as X.XX:1; the more common first, as a tie goes.
export const STANDARD_RATIOS = ["16:9", "9:16", "1:1", "4:3", "3:4", "4:5", "5:4", "3:2", "2:3", "21:9", "9:21", "2:1", "1:2", "2.39:1", "1.85:1", "1.90:1", "1.43:1", "1.37:1", "1.66:1"]
/** Scope, which people also call 2.35:1. */
const ALSO: Record<string, number> = { "2.39:1": 2.35 }

/** How near a ratio is to a shape that names it: about 2%, as a model's multiple-of-64 sizes are (H3's 16:9 is 1344×768, 1.75). */
const NEAR = 0.02

const off = (ratio: number, shape: string) => Math.min(...[ratioOf(shape)!, ALSO[shape] ?? Infinity].map((at) => Math.abs(Math.log(ratio / at))))
const gcd = (a: number, b: number): number => (b ? gcd(b, a % b) : a)

/** Whole ratios up to 10:10, simplest first (3:1, 5:2), each near no standard one nor a simpler one, so each says itself. */
const SMALL = Array.from({ length: 100 }, (_, at) => [Math.floor(at / 10) + 1, (at % 10) + 1])
  .filter(([w, h]) => gcd(w, h) === 1)
  .sort((a, b) => a[0] + a[1] - (b[0] + b[1]))
  .map(([w, h]) => `${w}:${h}`)
  .reduce<string[]>((kept, shape) => ([...STANDARD_RATIOS, ...kept].some((near) => off(ratioOf(shape)!, near) <= NEAR) ? kept : [...kept, shape]), [])

const decimal = (ratio: number) => (ratio >= 1 ? `${ratio.toFixed(2)}:1` : `1:${(1 / ratio).toFixed(2)}`)

/** A ratio as people say it: the nearest standard one within about 2%, else the simplest whole one, else `2.35:1`. Text that isn't one is
 *  itself. Only what `fits` is said, as a tool's bounds hold it. What it says reads back as itself. */
export function ratioWords(value: number | string, fits: (ratio: number) => boolean = () => true): string {
  const ratio = typeof value === "number" ? value : ratioOf(value)
  if (!ratio || !Number.isFinite(ratio)) return String(value)
  const named = (at: number) => {
    const standard = STANDARD_RATIOS.filter((shape) => fits(ratioOf(shape)!) && off(at, shape) <= NEAR)
    if (standard.length) return standard.reduce((best, shape) => (off(at, shape) < off(at, best) ? shape : best))
    return SMALL.find((shape) => fits(ratioOf(shape)!) && off(at, shape) <= NEAR)
  }
  // Its decimal reads back as a ratio too: what names that names it.
  return named(ratio) ?? named(ratioOf(decimal(ratio))!) ?? decimal(ratio)
}
