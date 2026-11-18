import { useEffect, useSyncExternalStore } from "react"
import type { GpuReading } from "./calcifer"
import type { EventStream } from "./events"
import { tabShown } from "../ui/signal"

/** How far back a GPU's trace reaches. */
export const TRACE_MS = 60_000

/** One machine's GPU readings, oldest first, the last minute's: when the machine read each (its clock) and when this page heard it. */
export type GpuTrace = { at: number; heard: number; gpus: GpuReading[] }[]

const NONE: GpuTrace = []

/** Machines' GPU use as the events stream brings it (cl-1191): each machine's last minute of readings, in memory only, and whether
 *  something in this tab shows it, which is what has Calcifer read them. */
class GpuReadings {
  private traces = new Map<string, GpuTrace>()
  private listeners = new Set<() => void>()
  private shown = 0

  constructor(private stream: EventStream) {
    stream.subscribe((event) => {
      if (event.type === "machine.gpus") this.heard(event.name, event.reading.at_unix_ms, event.reading.gpus)
      else if (event.type === "machine.removed") this.forget(event.name)
    })
    tabShown.subscribe(() => this.tell())
  }

  private heard(name: string, at: number, gpus: GpuReading[]) {
    const since = at - TRACE_MS
    const before = this.traces.get(name) ?? NONE
    this.traces.set(name, [...before.filter((reading) => reading.at > since && reading.at < at), { at, heard: Date.now(), gpus }])
    for (const listener of this.listeners) listener()
  }

  private forget(name: string) {
    if (!this.traces.delete(name)) return
    for (const listener of this.listeners) listener()
  }

  /** Shown while something here shows it and this tab is in view. */
  private tell() {
    this.stream.showGpus(this.shown > 0 && tabShown.get())
  }

  show(): () => void {
    this.shown++
    this.tell()
    return () => {
      this.shown--
      this.tell()
    }
  }

  subscribe = (listener: () => void) => {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }

  trace(name: string): GpuTrace {
    return this.traces.get(name) ?? NONE
  }
}

const readings = new WeakMap<EventStream, GpuReadings>()

function of(stream: EventStream): GpuReadings {
  let held = readings.get(stream)
  if (!held) readings.set(stream, (held = new GpuReadings(stream)))
  return held
}

/** Machines' GPU use is read while this is mounted (and the tab is in view). */
export function useShowGpus(stream: EventStream) {
  useEffect(() => of(stream).show(), [stream])
}

/** Machine `name`'s last minute of GPU readings, oldest first; empty until its first comes, and from a machine that reports none. */
export function useGpuTrace(stream: EventStream, name: string): GpuTrace {
  const store = of(stream)
  return useSyncExternalStore(store.subscribe, () => store.trace(name))
}
