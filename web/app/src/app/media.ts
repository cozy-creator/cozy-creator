import { useEffect, useRef, useState } from "react"
import { type Asking, asking } from "../api/thumbs"
import { useConnection } from "./Connection"

const epochs = new Map<string, number>()
/** How many times `project`'s view has closed here (`retireMedia`): what was asked before is let go. */
export const mediaEpoch = (project: string) => epochs.get(project) ?? 0
/** `project`'s view closed (its vault locked): what's still asked for of it is let go. */
export function retireMedia(project: string) {
  epochs.set(project, mediaEpoch(project) + 1)
}

/**
 * `project`'s file at `path` (of `sha256`) for an `<img>`: fetched into this page's memory, so a `202` (not made yet) is seen, and asked for
 * as every file is (`asking`). `onLoad` and `onError` go on the `<img>`; `failed` once it's unavailable. `live`: whether its asker still wants it.
 */
export function useAskedImage(project: string | null | undefined, sha256: string, path: string | undefined, live: () => boolean = () => true) {
  const { client, bridge, privateViewEpoch, signedOut } = useConnection()
  const [held, setHeld] = useState<{ path: string; src: string; request: number; asked: Asking }>()
  const [failed, setFailed] = useState<string>()
  const wanted = useRef(live)
  wanted.current = live
  const requests = useRef(0)
  useEffect(() => {
    if (!path || !project) return
    let on = true, url: string | undefined
    let fetching: AbortController | undefined
    const epoch = mediaEpoch(project)
    const projectEpoch = bridge.projectEpoch(project)
    const current = () => on && wanted.current() && !signedOut.get() && mediaEpoch(project) === epoch && bridge.projectEpoch(project) === projectEpoch
    const ask = () => {
      if (!current()) return
      const request = (fetching = new AbortController())
      void (async () => {
        const response = await client.transport.fetch(path, { signal: request.signal })
        if (!response.ok || response.status === 202) {
          void response.body?.cancel()
          if (current()) { if (response.status === 202) asked.pending(); else asked.failed() }
          return
        }
        const blob = await response.blob()
        if (!current()) return
        if (url) URL.revokeObjectURL(url)
        url = URL.createObjectURL(blob)
        setFailed(undefined)
        setHeld({ path, src: url, request: ++requests.current, asked })
      })().catch((error) => {
        if (!current()) return
        if (error instanceof DOMException && error.name === "AbortError") asked.held()
        else asked.failed()
      })
    }
    const asked = asking(project, sha256, path, ask, () => { if (current()) setFailed(path) })
    const stop = () => { on = false; fetching?.abort(); asked.stop() }
    const unwatch = privateViewEpoch.subscribe(() => { if (bridge.projectEpoch(project) !== projectEpoch) stop() })
    const unsign = signedOut.subscribe(() => { if (signedOut.get()) stop() })
    asked.start()
    return () => {
      unwatch(); unsign(); stop()
      if (url) URL.revokeObjectURL(url)
    }
  }, [client, bridge, privateViewEpoch, signedOut, project, sha256, path])
  const shown = held?.path === path && failed !== path ? held : undefined
  return {
    src: shown?.src,
    request: shown?.request ?? 0,
    failed: failed === path,
    onLoad: () => shown?.asked.shown(),
    onError: () => shown?.asked.failed(),
  }
}
