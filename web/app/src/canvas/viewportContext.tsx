import { createContext, type ReactNode, useContext, useMemo, useSyncExternalStore } from "react"
import type { CanvasController, CanvasNode, Viewport } from "./viewport"

const Context = createContext<CanvasController | null>(null)

export function CanvasViewportProvider({ controller, children }: { controller: CanvasController; children: ReactNode }) {
  return <Context value={controller}>{children}</Context>
}

export function useCanvasViewport(): CanvasController {
  const controller = useContext(Context)
  if (!controller) throw new Error("no canvas viewport")
  return controller
}

function useSelected<Source, Value>(subscribe: (listener: () => void) => () => void, get: () => Source, read: (source: Source) => Value): Value {
  const snapshot = useMemo(() => {
    let previous: { source: Source; value: Value } | undefined
    return () => {
      const source = get()
      if (!previous || !Object.is(previous.source, source)) previous = { source, value: read(source) }
      return previous.value
    }
  }, [get, read])
  return useSyncExternalStore(subscribe, snapshot)
}

/** Select a stable value (for example zoom) so a pan does not render React controls again. */
export function useCanvasViewportValue<T>(read: (view: Viewport) => T): T {
  const controller = useCanvasViewport()
  return useSelected(controller.subscribeCamera, controller.getViewport, read)
}

/** Logical scene changes are independent of camera movement and rendered-object lifetimes. */
export function useCanvasNodesValue<T>(read: (nodes: readonly CanvasNode[]) => T): T {
  const controller = useCanvasViewport()
  return useSelected(controller.subscribeNodes, controller.getNodes, read)
}
