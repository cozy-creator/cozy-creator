import { useStoreApi } from "@xyflow/react"
import { useEffect, useRef } from "react"
import { useSignal } from "../ui/signal"
import { landscape, theme } from "../ui/theme"

/** What the paper's tiles all repeat on, so wrapping by it can't be seen. */
const PERIOD = 2048
/** How far it follows a zoom. Any smaller and a 4K window shows more of it than Chrome will raster, and parts go blank; any larger and its tooth blurs. */
const SMALLEST = 0.75
const LARGEST = 1.5

/** What the canvas is drawn on: on paper a landscape unless it's hidden, else a sheet that goes where the canvas goes. */
export function Sheet() {
  const [now, shown] = [useSignal(theme), useSignal(landscape)]
  return now === "paper" && shown ? <Landscape /> : <World />
}

/** The sheet, which goes where the canvas goes. The store is read here and the transform set by hand, so a pan renders nothing, and the sheet is on a layer of its own, so it repaints nothing. */
function World() {
  const flow = useStoreApi()
  const sheet = useRef<HTMLDivElement>(null)
  useEffect(() => {
    let last = flow.getState().transform
    const place = ([x, y, zoom]: typeof last) => {
      const scale = Math.min(LARGEST, Math.max(SMALLEST, zoom))
      const period = PERIOD * scale
      const wrap = (value: number) => (((value % period) + period) % period) - period
      if (sheet.current) sheet.current.style.transform = `translate3d(${wrap(x)}px, ${wrap(y)}px, 0) scale(${scale})`
    }
    place(last)
    return flow.subscribe(({ transform }) => {
      if (transform === last) return
      last = transform
      place(transform)
    })
  }, [flow])
  return <div ref={sheet} className="paper-world" aria-hidden="true" />
}

/**
 * The sky, mountains, hills and meadow (`@cozy-creator/paper`), each moved by its own `--rate` of a pan across and `--rate-y` of one up or
 * down: the farthest slowest. The sky is the paper, with its clouds on it. Up and down they ease to a stop `--reach` from where they began,
 * and each has more of itself below than the one in front can uncover, so no pan leaves a gap. A zoom moves none of them. With reduced
 * motion they hold still. A pan sets four transforms and draws nothing.
 */
function Landscape() {
  const flow = useStoreApi()
  const land = useRef<HTMLDivElement>(null)
  useEffect(() => {
    const whole = land.current
    if (!whole) return
    const layers = [...whole.children] as HTMLElement[]
    const read = (element: HTMLElement, name: string) => Number(getComputedStyle(element).getPropertyValue(name))
    const [across, down] = [layers.map((layer) => read(layer, "--rate")), layers.map((layer) => read(layer, "--rate-y"))]
    const at = layers.map(() => 0)
    let [periods, reach, vertical] = [[] as number[], 0, 0]
    const place = () => {
      const eased = vertical - vertical ** 3 / (3 * reach * reach)
      layers.forEach((layer, i) => {
        at[i] = ((at[i] % periods[i]) - periods[i]) % periods[i]
        layer.style.transform = `translate3d(${Math.round(at[i])}px, ${Math.round(down[i] * eased)}px, 0)`
      })
    }
    // Each wraps by its own width past the window's, and goes up and down as far as its fill allows: a taller window makes both larger.
    const width = (element: HTMLElement) => element.getBoundingClientRect().width
    const measure = new ResizeObserver(() => {
      periods = layers.map((layer) => width(layer) - width(whole))
      reach = read(whole, "--reach") * read(whole, "--landscape-scale")
      vertical = Math.max(-reach, Math.min(reach, vertical))
      place()
    })
    measure.observe(whole)
    const still = matchMedia("(prefers-reduced-motion: reduce)")
    let [x, y, zoom] = flow.getState().transform
    const stop = flow.subscribe(({ transform }) => {
      const panned = transform[2] === zoom && !still.matches && periods.length > 0
      const [dx, dy] = panned ? [transform[0] - x, transform[1] - y] : [0, 0]
      ;[x, y, zoom] = transform
      if (!dx && !dy) return
      across.forEach((rate, i) => {
        at[i] += dx * rate
      })
      vertical = Math.max(-reach, Math.min(reach, vertical + dy))
      place()
    })
    return () => {
      stop()
      measure.disconnect()
    }
  }, [flow])
  return (
    <div ref={land} className="paper-landscape" aria-hidden="true">
      <div className="paper-landscape-sky" />
      <div className="paper-landscape-mountains" />
      <div className="paper-landscape-hills" />
      <div className="paper-landscape-meadow" />
    </div>
  )
}

/** The dots' step and size at a zoom of 1. The layer is a step at the most zoom (4) wider and taller than the window: `.canvas-dots`. */
const GAP = 24
const DOT = 2

/**
 * The dots on the paper, on a layer of their own that a pan moves by less than a step, so a pan draws none of them: React Flow's own
 * background draws them all again each frame, which in WebKit is most of a pan's frame far out. A zoom draws them again. Further out
 * than `data-lod` 2 they're too small to see, and none is drawn.
 */
export function Dots() {
  const flow = useStoreApi()
  const layer = useRef<SVGSVGElement>(null)
  const cell = useRef<SVGPatternElement>(null)
  const dot = useRef<SVGCircleElement>(null)
  useEffect(() => {
    let [last, step] = [flow.getState().transform, 0]
    const place = ([x, y, zoom]: typeof last) => {
      if (!layer.current || !cell.current || !dot.current) return
      const gap = GAP * zoom
      if (gap !== step) {
        step = gap
        cell.current.setAttribute("width", String(gap))
        cell.current.setAttribute("height", String(gap))
        for (const at of ["cx", "cy", "r"]) dot.current.setAttribute(at, String((DOT * zoom) / 2))
      }
      const wrap = (value: number) => (((value % gap) + gap) % gap) - gap
      // Where React Flow's `<Background>` put them, to the pixel.
      layer.current.style.transform = `translate3d(${wrap(x - gap / 2)}px, ${wrap(y - gap / 2)}px, 0)`
    }
    place(last)
    return flow.subscribe(({ transform }) => {
      if (transform === last) return
      last = transform
      place(transform)
    })
  }, [flow])
  return (
    <svg ref={layer} className="canvas-dots" aria-hidden="true">
      <pattern ref={cell} id="canvas-dots" patternUnits="userSpaceOnUse">
        <circle ref={dot} />
      </pattern>
      <rect width="100%" height="100%" fill="url(#canvas-dots)" />
    </svg>
  )
}
