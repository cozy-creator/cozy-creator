import { createContext, useCallback, useContext, useSyncExternalStore } from "react"
import type { CanvasStore, ElementView } from "./CanvasStore"

const Context = createContext<CanvasStore | null>(null)
export const CanvasStoreProvider = Context.Provider

export function useCanvasStore(): CanvasStore {
  const store = useContext(Context)
  if (!store) throw new Error("no canvas store")
  return store
}

/** What `read` makes of a box, read again as it changes: what reads it renders only when that does. `read` returns a string or a number. */
export function useOfElement<T extends string | number | undefined>(name: string, read: (view: ElementView | undefined) => T): T {
  const store = useCanvasStore()
  const subscribe = useCallback((listener: () => void) => store.subscribe(name, listener), [store, name])
  return useSyncExternalStore(subscribe, () => read(store.get(name)))
}

/** One element's view: a card re-renders when its own name is notified, and for nothing else. */
export function useElement(name: string): ElementView | undefined {
  const store = useCanvasStore()
  const subscribe = useCallback((listener: () => void) => store.subscribe(name, listener), [store, name])
  return useSyncExternalStore(subscribe, () => store.get(name))
}
