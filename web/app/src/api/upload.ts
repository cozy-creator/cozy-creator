import { createSHA256 } from "hash-wasm"
import type { Client } from "./client"

// Small enough that hashing a chunk fits between frames, on a page that may be panning.
const READ_BYTES = 2 * 1024 * 1024
const PART_BYTES = 8 * 1024 * 1024

/** Lets the page paint and take input. A message, not a timer: a hidden tab throttles timers to a crawl. */
function yieldToPage(): Promise<void> {
  return new Promise((done) => {
    const { port1, port2 } = new MessageChannel()
    port1.onmessage = () => {
      port1.close()
      done()
    }
    port2.postMessage(0)
  })
}

export async function hashFile(file: File, signal: AbortSignal, onDone: (done: number) => void): Promise<string> {
  const digest = await createSHA256()
  for (let at = 0; at < file.size; at += READ_BYTES) {
    signal.throwIfAborted()
    digest.update(new Uint8Array(await file.slice(at, at + READ_BYTES).arrayBuffer()))
    onDone(Math.min(1, (at + READ_BYTES) / file.size))
    await yieldToPage()
  }
  return `sha256:${digest.digest("hex")}`
}

/** What `held` lacks of `size` bytes, as half-open ranges. */
function gaps(held: [number, number][], size: number): [number, number][] {
  const missing: [number, number][] = []
  let at = 0
  for (const [first, last] of [...held].sort((a, b) => a[0] - b[0])) {
    if (first > at) missing.push([at, first])
    at = Math.max(at, last + 1)
  }
  if (at < size) missing.push([at, size])
  return missing
}

let line: Promise<unknown> = Promise.resolve()

/** Bytes move one file at a time, on one connection: the page has few to spare. */
export function inLine<T>(work: () => Promise<T>): Promise<T> {
  const turn = line.then(work, work)
  line = turn.catch(() => {})
  return turn
}

/** Sends what Calcifer lacks of a file, resumably, until it holds it whole. */
export async function sendFile(client: Client, file: File, digest: string, signal: AbortSignal, onDone: (done: number) => void, onAttempt: (attempt: number) => void, projectId?: string) {
  signal.throwIfAborted()
  if (file.size === 0) {
    await client.put(digest, file, 0, 0, signal, onAttempt, projectId)
    return
  }
  // Sent whole and still not whole means what it held expired under it, so ask again; a third time isn't chance.
  for (let pass = 0; pass < 3; pass++) {
    const held = await client.held(digest, signal, onAttempt, projectId)
    if (held === true) return
    const missing = gaps(held, file.size)
    let sent = file.size - missing.reduce((sum, [from, to]) => sum + to - from, 0)
    onDone(sent / file.size)
    for (const [from, to] of missing) {
      for (let at = from; at < to; at += PART_BYTES) {
        const end = Math.min(to, at + PART_BYTES)
        const whole = await client.put(digest, file.slice(at, end), at, file.size, signal, onAttempt, projectId)
        sent += end - at
        onDone(sent / file.size)
        if (whole) return
      }
    }
  }
  throw new Error("Calcifer took every part and doesn't hold the file")
}

export type Moving = { stage: "reading" | "sending"; done: number }

/** Hashes a file as it reads it, and sends it: its `sha256:`, held by Calcifer for a day. */
export function moveBytes(client: Client, file: File, signal: AbortSignal, onMoving: (moving: Moving) => void, onAttempt: (attempt: number) => void, projectId?: string): Promise<string> {
  return inLine(async () => {
    onMoving({ stage: "reading", done: 0 })
    let shown = 0
    const digest = await hashFile(file, signal, (done) => {
      if (done - shown < 0.01 && done < 1) return
      shown = done
      onMoving({ stage: "reading", done })
    })
    onMoving({ stage: "sending", done: 0 })
    await sendFile(client, file, digest, signal, (done) => onMoving({ stage: "sending", done }), onAttempt, projectId)
    return digest
  })
}
