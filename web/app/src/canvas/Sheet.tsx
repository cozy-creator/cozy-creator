import { useStoreApi } from "@xyflow/react"
import { useEffect, useRef } from "react"

/** What the paper's tiles all repeat on, so wrapping by it can't be seen. */
const PERIOD = 2048
/** How far it follows a zoom. Any smaller and a 4K window shows more of it than Chrome will raster, and parts go blank; any larger and its tooth blurs. */
const SMALLEST = 0.75
const LARGEST = 1.5

/** The paper the canvas is drawn on, which goes where it goes. The store is read here and the transform set by hand, so a pan renders nothing, and the sheet is on a layer of its own, so it repaints nothing. */
export function Sheet() {
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
