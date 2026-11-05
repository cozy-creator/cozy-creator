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
