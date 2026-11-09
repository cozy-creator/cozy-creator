import type { Fit } from "../api/calcifer"
import { useCanvasStore } from "./context"

/** This computer would render it poorly, and nothing's said for that: how, in a line, then here anyway or a GPU to rent. */
export function Asking({ name, fit, all }: { name?: string; fit: Fit; all?: boolean }) {
  const store = useCanvasStore()
  return (
    <div className="status asking" role="group" aria-label="Render here?">
      {fit.tools
        .filter((tool) => !tool.here)
        .map((tool) => (
          <p key={tool.tool}>{tool.line}</p>
        ))}
      {fit.rent && <p>{fit.rent.line}</p>}
      <div className="row">
        <button type="button" className="tb nodrag" onClick={() => (all ? store.renderAllHere() : name && store.renderHere(name))}>
          Render here
        </button>
        <button type="button" className="tb primary nodrag" onClick={() => (all ? store.rentForAll() : name && store.rentInstead(name))}>
          Rent a GPU
        </button>
      </div>
    </div>
  )
}
