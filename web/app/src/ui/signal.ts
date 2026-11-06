import { useSyncExternalStore } from "react"

export interface Signal<T> {
  get(): T
  set(value: T): void
  subscribe(listener: () => void): () => void
}

export function signal<T>(value: T): Signal<T> {
  const listeners = new Set<() => void>()
  return {
    get: () => value,
    set(next) {
      if (Object.is(value, next)) return
      value = next
      for (const listener of listeners) listener()
    },
    subscribe(listener) {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
  }
}

export function useSignal<T>(source: Signal<T>): T {
  return useSyncExternalStore(source.subscribe, source.get)
}
