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
 */
const RUNG = 4 / 3
const rungOf = (scale: number) => RUNG ** Math.ceil(Math.log(scale) / Math.log(RUNG) - 1e-9)

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
 * layer of its own, so it repaints nothing. On paper, a plain sheet is under it, the same but its sky's: a zoom out fades the sky away by its layer's opacity.
 */
function World({ washed }: { washed: boolean }) {
  const flow = useStoreApi()
  const plain = useRef<HTMLDivElement>(null)
  const sheet = useRef<HTMLDivElement>(null)
  useEffect(() => {
    let [last, held] = [flow.getState().transform, 0]
    const place = ([x, y, zoom]: typeof last) => {
      const rung = rungOf(zoom)
      const period = PERIOD * zoom
      const wrap = (value: number) => (((value % period) + period) % period) - period
      for (const element of [plain.current, sheet.current]) {
        if (!element) continue
        if (rung !== held) element.style.setProperty("--rung", String(rung))
        element.style.transform = `translate3d(${wrap(x)}px, ${wrap(y)}px, 0) scale(${zoom / rung})`
      }
      held = rung
      if (plain.current && sheet.current) sheet.current.style.opacity = String(Math.min(1, Math.max(0, (zoom - UNWASHED) / (WASHED - UNWASHED))))
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
    // The land is laid out at its rung by `--landscape-scale`, its tiles `--width` wide at 1; the sky at 1, its tiles the paper's.
    const widths = layers.map((layer) => read(layer, "--width"))
    const sky = widths.map((width) => Number.isNaN(width))
    const still = matchMedia("(prefers-reduced-motion: reduce)")
    const at = layers.map(() => 0)
    const rungs = layers.map(() => 0)
    let [x, y, zoom] = flow.getState().transform
    let [size, reach, vertical] = [1, 1, 0]
    // A taller window draws it larger (paper.css), and lets it go further up and down.
    const measure = () => {
      size = read(whole, "--landscape-scale")
      reach = read(whole, "--reach") * size
      vertical = Math.max(-reach, Math.min(reach, vertical))
      rungs.fill(0)
    }
    const place = () => {
      const eased = vertical - vertical ** 3 / (3 * reach * reach)
      layers.forEach((layer, i) => {
        const scale = still.matches ? 1 : zoom ** across[i]
        const rung = sky[i] ? 1 : rungOf(scale)
        if (rung !== rungs[i]) layer.style.setProperty("--landscape-scale", String(size * (rungs[i] = rung)))
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

/**
 * The dots, over what the canvas is drawn on and under every box, the same size at every zoom. They're on a layer of their own that a pan
 * moves by less than a step, so a pan draws none of them: React Flow's own background draws them all again each frame, which in WebKit is
 * most of a pan's frame far out. A zoom draws them again. The layer is a step at the most zoom (4) wider and taller than the window: `.canvas-dots`.
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
      let gap = GAP * zoom
      while (gap < LEAST) gap *= 2
      if (gap !== step) {
        step = gap
        for (const at of ["width", "height"]) cell.current.setAttribute(at, String(gap))
        for (const at of ["cx", "cy"]) dot.current.setAttribute(at, String(gap / 2))
      }
      // The layer on a whole pixel, so a pan doesn't soften the dots.
      const wrap = (value: number) => Math.round(devicePixelRatio * ((((value % gap) + gap) % gap) - gap)) / devicePixelRatio
      const off = (GAP / 2) * zoom - gap / 2
      layer.current.style.transform = `translate3d(${wrap(x + off)}px, ${wrap(y + off)}px, 0)`
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
        <circle ref={dot} r={DOT / 2} />
      </pattern>
      <rect width="100%" height="100%" fill="url(#canvas-dots)" />
    </svg>
  )
}
