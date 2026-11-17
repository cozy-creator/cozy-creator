import { createContext, type ReactNode, useContext, useMemo, useSyncExternalStore } from "react"
import type { Board, CanvasNode } from "./Board"
import type { Viewport } from "./Camera"

const Context = createContext<Board | null>(null)

export function BoardProvider({ board, children }: { board: Board; children: ReactNode }) {
  return <Context value={board}>{children}</Context>
}

export function useBoard(): Board {
  const board = useContext(Context)
  if (!board) throw new Error("no canvas board")
  return board
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

/** A value of the camera (its zoom band, say): a pan that leaves it alone renders nothing. */
export function useCameraValue<T>(read: (view: Viewport) => T): T {
  const board = useBoard()
  return useSelected(board.camera.subscribeCamera, board.camera.getViewport, read)
}

export function useNodesValue<T>(read: (nodes: readonly CanvasNode[]) => T): T {
  const board = useBoard()
  return useSelected(board.subscribeNodes, board.getNodes, read)
}
