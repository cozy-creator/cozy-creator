import type { Fit } from "../api/calcifer"
import { useCanvasStore } from "./context"

/** "an RTX 4090", "a B200": by how the card's first letter is said. */
const article = (card: string) => (/^[aefhilmnorsx]/i.test(card) ? "an" : "a")

/** This computer would render it poorly, and nothing's said for that: why, in a line, then a GPU to rent first, or here anyway (cl-897). */
export function Asking({ name, fit, all }: { name?: string; fit: Fit; all?: boolean }) {
  const store = useCanvasStore()
  const card = fit.rent?.card
  return (
    <div className="status asking" role="group" aria-label="Render here?">
      {fit.tools
        .filter((tool) => !tool.here)
        .map((tool) => (
          <p key={tool.tool}>{tool.line}</p>
        ))}
      {fit.rent && <p>{fit.rent.line}</p>}
      <div className="row">
        <button type="button" className="tb primary nodrag" onClick={() => (all ? store.rentForAll() : name && store.rentInstead(name))}>
          {card ? `Rent ${article(card)} ${card}` : "Rent a GPU"}
        </button>
        <button type="button" className="tb nodrag" onClick={() => (all ? store.renderAllHere() : name && store.renderHere(name))}>
          Run here anyway
        </button>
      </div>
    </div>
  )
}
