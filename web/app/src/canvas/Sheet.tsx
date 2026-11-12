import { useStoreApi } from "@xyflow/react"
import { useEffect, useRef } from "react"
import { useSignal } from "../ui/signal"
import { landscape, theme } from "../ui/theme"

/** What the paper's tiles all repeat on, so wrapping by it can't be seen. */
const PERIOD = 2048
/**
 * A background is laid out at a rung, each 4/3 of the one below, and its transform scales it by the rest: (0.75, 1]. Chrome rasters a layer
 * that will change its transform at the scale it first drew it, and never below 1: drawn larger it blurs, and drawn below 0.75 a 4K window
 * shows more of it than Chrome will raster, and parts go blank. A zoom across a rung lays out and draws that one layer again.
 *
 * The rung is the layer's `data-rung`, 4/3 to that power, which paper.css turns into `--rung`. A layer's inline style is its transform alone:
 * a custom property there makes Chrome restyle the layer whole each time the transform is set, and paint its pictures again, every frame of
 * a pan or a zoom (e2e 43174362261, 43184991423: the dark sheet, some 45 tiles a frame).
 */
const RUNG = 4 / 3
/** The rungs paper.css has, for every zoom (0.05 to 4). */
const [LOWEST, HIGHEST] = [-10, 5]
const stepOf = (scale: number) => Math.min(HIGHEST, Math.max(LOWEST, Math.ceil(Math.log(scale) / Math.log(RUNG) - 1e-9)))

/** What the canvas is drawn on: on paper a landscape unless it's hidden, else a sheet that goes where the canvas goes. */
export function Sheet() {
  const [now, shown] = [useSignal(theme), useSignal(landscape)]
  return now === "paper" && shown ? <Landscape /> : <World key={now} washed={now === "paper"} />
}

/** Paper's sky comes and goes down the sheet every 2048 px, and its clouds repeat on that: far out they'd be stripes. It's all on from here in, and gone from here out, the plain sheet under it. */
const WASHED = 0.6
const UNWASHED = 0.3

/**
 * The sheet, which goes where the canvas goes, at every zoom. The store is read here and the transform set by hand, so a pan renders nothing, and the sheet is on a
 * layer of its own, so it repaints nothing. On paper, a plain sheet is under it, the same paper without its sky: a zoom out fades the sky away by its layer's opacity.
 */
function World({ washed }: { washed: boolean }) {
  const flow = useStoreApi()
  const plain = useRef<HTMLDivElement>(null)
  const sheet = useRef<HTMLDivElement>(null)
  useEffect(() => {
    let [last, held, washing] = [flow.getState().transform, NaN, -1]
    const place = ([x, y, zoom]: typeof last) => {
      const step = stepOf(zoom)
      const period = PERIOD * zoom
      const wrap = (value: number) => (((value % period) + period) % period) - period
      for (const element of [plain.current, sheet.current]) {
        if (!element) continue
        if (step !== held) element.dataset.rung = String(step)
        element.style.transform = `translate3d(${wrap(x)}px, ${wrap(y)}px, 0) scale(${zoom / RUNG ** step})`
      }
      held = step
      const wash = Math.min(1, Math.max(0, (zoom - UNWASHED) / (WASHED - UNWASHED)))
      if (plain.current && sheet.current && wash !== washing) {
        // Only one is drawn but while it fades: the other, under all of it or under nothing, is hidden.
        sheet.current.style.opacity = String((washing = wash))
        sheet.current.style.visibility = wash > 0 ? "" : "hidden"
        plain.current.style.visibility = wash < 1 ? "" : "hidden"
      }
    }
    place(last)
    return flow.subscribe(({ transform }) => {
      if (transform === last) return
      last = transform
      place(transform)
    })
  }, [flow])
  return (
    <>
      {washed && <div ref={plain} className="paper-world paper-world-plain" aria-hidden="true" />}
      <div ref={sheet} className="paper-world" aria-hidden="true" />
    </>
  )
}

/**
 * The sky, mountains, hills and meadow (`@cozy-creator/paper`), each following the canvas by its own `--rate`: that of a pan across, and the
 * zoom to that power, so the farthest moves and grows least. A zoom scales each about where it's zoomed across and about the bottom of the
 * window up and down, where the land stays. Up and down a pan moves each by its `--rate-y`, eased to a stop `--reach` from where it began;
 * each has more of itself below than the one in front can uncover, and the sky reaches above the window (ui/paper.css), so no pan or zoom
 * leaves a gap. With reduced motion they hold still. A pan sets four transforms and draws nothing.
 */
function Landscape() {
  const flow = useStoreApi()
  const land = useRef<HTMLDivElement>(null)
  useEffect(() => {
    const whole = land.current
    if (!whole) return
    const layers = [...whole.children] as HTMLElement[]
    const read = (element: HTMLElement, name: string) => parseFloat(getComputedStyle(element).getPropertyValue(name))
    const [across, down] = [layers.map((layer) => read(layer, "--rate")), layers.map((layer) => read(layer, "--rate-y"))]
    // The land is laid out at its rung (paper.css: `--landscape-scale`), its tiles `--width` wide at 1; the sky at 1, its tiles the paper's.
    const widths = layers.map((layer) => read(layer, "--width"))
    const sky = widths.map((width) => Number.isNaN(width))
    const still = matchMedia("(prefers-reduced-motion: reduce)")
    const at = layers.map(() => 0)
    const steps = layers.map(() => NaN)
    let [x, y, zoom] = flow.getState().transform
    let [size, reach, vertical] = [1, 1, 0]
    // A taller window draws it larger (paper.css), and lets it go further up and down.
    const measure = () => {
      size = read(whole, "--landscape-scale")
      reach = read(whole, "--reach") * size
      vertical = Math.max(-reach, Math.min(reach, vertical))
    }
    const place = () => {
      const eased = vertical - vertical ** 3 / (3 * reach * reach)
      layers.forEach((layer, i) => {
        const scale = still.matches ? 1 : zoom ** across[i]
        const step = sky[i] ? 0 : stepOf(scale)
        if (step !== steps[i]) layer.dataset.rung = String((steps[i] = step))
        const rung = RUNG ** step
        const period = (sky[i] ? PERIOD : widths[i] * size) * scale
        at[i] = ((at[i] % period) - period) % period
        layer.style.transform = `translate3d(${Math.round(at[i])}px, ${Math.round(down[i] * eased * scale)}px, 0) scale(${scale / rung})`
      })
    }
    measure()
    place()
    const resized = new ResizeObserver(() => {
      measure()
      place()
    })
    resized.observe(whole)
    const stop = flow.subscribe(({ transform }) => {
      const [nx, ny, nz] = transform
      if (nx === x && ny === y && nz === zoom) return
      if (!still.matches) {
        // A pan and a zoom are one move of the canvas: by k about the point it leaves where it was. Each layer moves by k to its rate
        // about that point: for a pan (k = 1), by its rate of it.
        const k = nz / zoom
        across.forEach((rate, i) => {
          const scaled = k ** rate
          at[i] = at[i] * scaled + (nx - k * x) * (Math.abs(1 - k) < 1e-9 ? rate : (1 - scaled) / (1 - k))
        })
        if (nz === zoom) vertical = Math.max(-reach, Math.min(reach, vertical + ny - y))
      }
      ;[x, y, zoom] = transform
      place()
    })
    return () => {
      stop()
      resized.disconnect()
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

/**
 * The dots' step on the canvas, their size on screen, and the least step on screen: a step smaller doubles, so far out every 2nd, 4th… is drawn, each one
 * of the dots nearer in. They're half a step off the canvas's origin, so not on the round numbers boxes are put at.
 */
const GAP = 24
const DOT = 2
const LEAST = 12
/** How far above and below a row of dots its band reaches: the dot and its softened edge. */
const ROW = 3

/**
 * The dots, over what the canvas is drawn on and under every box, the same size at every zoom. They're on a layer of their own that a pan
 * moves by less than a step, so a pan draws none of them: React Flow's own background draws them all again each frame, which in WebKit is
 * most of a pan's frame far out. A zoom draws them again. Scaled, the layer would be filtered each frame of a pan, and kept on a layer through a zoom it's
 * drawn slower (e2e 43144421327). The pattern fills only a band along each row of dots, so a zoom draws at most half of the layer, not all of it
 * (e2e 43177889306: 370 → 210 ms of raster a second). The layer is a step at the most zoom (4) wider and taller than the window: `.canvas-dots`.
 */
export function Dots() {
  const flow = useStoreApi()
  const layer = useRef<SVGSVGElement>(null)
  const cell = useRef<SVGPatternElement>(null)
  const dot = useRef<SVGCircleElement>(null)
  const rows = useRef<SVGPathElement>(null)
  useEffect(() => {
    const svg = layer.current
    if (!svg) return
    let [last, step, wide, tall] = [flow.getState().transform, 0, 0, 0]
    const place = ([x, y, zoom]: typeof last) => {
      if (!cell.current || !dot.current || !rows.current) return
      let gap = GAP * zoom
      while (gap < LEAST) gap *= 2
      if (gap !== step) {
        step = gap
        for (const at of ["width", "height"]) cell.current.setAttribute(at, String(gap))
        for (const at of ["cx", "cy"]) dot.current.setAttribute(at, String(gap / 2))
        let bands = ""
        for (let at = gap / 2 - ROW; at < tall; at += gap) bands += `M0 ${at.toFixed(2)}h${wide}v${2 * ROW}H0z`
        rows.current.setAttribute("d", bands)
      }
      // The layer on a whole pixel, so a pan doesn't soften the dots.
      const wrap = (value: number) => Math.round(devicePixelRatio * ((((value % gap) + gap) % gap) - gap)) / devicePixelRatio
      const off = (GAP / 2) * zoom - gap / 2
      svg.style.transform = `translate3d(${wrap(x + off)}px, ${wrap(y + off)}px, 0)`
    }
    const measured = new ResizeObserver(([{ contentRect }]) => {
      ;[wide, tall, step] = [Math.ceil(contentRect.width), Math.ceil(contentRect.height), 0]
      place(last)
    })
    measured.observe(svg)
    const stop = flow.subscribe(({ transform }) => {
      if (transform === last) return
      last = transform
      place(transform)
    })
    return () => {
      stop()
      measured.disconnect()
    }
  }, [flow])
  return (
    <svg ref={layer} className="canvas-dots" aria-hidden="true">
      <pattern ref={cell} id="canvas-dots" patternUnits="userSpaceOnUse">
        <circle ref={dot} r={DOT / 2} />
      </pattern>
      <path ref={rows} fill="url(#canvas-dots)" shapeRendering="crispEdges" />
    </svg>
  )
}
