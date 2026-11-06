import { createContext, useCallback, useContext, useSyncExternalStore } from "react"
import type { CanvasStore, ElementView } from "./CanvasStore"

const Context = createContext<CanvasStore | null>(null)
export const CanvasStoreProvider = Context.Provider

export function useCanvasStore(): CanvasStore {
  const store = useContext(Context)
  if (!store) throw new Error("no canvas store")
  return store
}

/** One element's view: a card re-renders when its own name is notified, and for nothing else. */
export function useElement(name: string): ElementView | undefined {
  const store = useCanvasStore()
  const subscribe = useCallback((listener: () => void) => store.subscribe(name, listener), [store, name])
  return useSyncExternalStore(subscribe, () => store.get(name))
}
