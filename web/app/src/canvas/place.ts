import { sha256 } from "@noble/hashes/sha2.js"
import { bytesToHex } from "@noble/hashes/utils.js"
import { ApiError, type Client } from "../api/client"
import { say } from "../ui/notes"
import { CARD_WIDTH, type CanvasStore, type MediaKind, type Stage } from "./CanvasStore"
import { stemName } from "./names"

// Small enough that hashing a chunk fits between frames, on a page that may be panning.
const READ_BYTES = 512 * 1024
const PART_BYTES = 8 * 1024 * 1024
const GAP = 32

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

async function hash(file: File, signal: AbortSignal, onDone: (done: number) => void): Promise<string> {
  const digest = sha256.create()
  for (let at = 0; at < file.size; at += READ_BYTES) {
    signal.throwIfAborted()
    digest.update(new Uint8Array(await file.slice(at, at + READ_BYTES).arrayBuffer()))
    onDone(Math.min(1, (at + READ_BYTES) / file.size))
    await yieldToPage()
  }
  return `sha256:${bytesToHex(digest.digest())}`
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

function mediaOf(file: File): MediaKind {
  const [type] = file.type.split("/")
  return type === "image" || type === "video" || type === "audio" ? type : "file"
}

export interface Hooks {
  /** The first import on an empty canvas made this project. */
  adopt(id: string): void
  refresh(id: string): void
}

/**
 * One act, from the drop to a constant in the project. The element is there at once; the name and the bytes go on side by side, and
 * `POST imports` goes when both are ready. A tab closed before that leaves nothing in the project.
 */
export class Placer {
  private store: CanvasStore
  private client: Client
  private hooks: Hooks
  private line: Promise<unknown> = Promise.resolve()
  private made?: Promise<string>
  private nextKey = 0

  constructor(store: CanvasStore, client: Client, hooks: Hooks) {
    this.store = store
    this.client = client
    this.hooks = hooks
  }

  /** One file is named by its field, focused; several take their stems, laid out from the point, with no focus. */
  add(files: File[], point: { x: number; y: number }) {
    const columns = Math.ceil(Math.sqrt(files.length))
    const left = point.x - CARD_WIDTH / 2
    const top = point.y - 16
    files.forEach((file, i) => {
      const key = `placing:${++this.nextKey}`
      const media = mediaOf(file)
      const stem = stemName(file.name) || "file"
      const abort = new AbortController()
      const at = {
        x: Math.round(left + (i % columns) * (CARD_WIDTH + GAP)),
        y: Math.round(top + Math.floor(i / columns) * (CARD_WIDTH * 0.75 + 30 + GAP)),
        w: CARD_WIDTH,
      }
      this.store.addPlacing(key, stem, { media, stage: "waiting", done: 0, stem, abort }, at, {
        naming: files.length === 1,
        preview: media === "image" ? URL.createObjectURL(file) : undefined,
      })
      void this.run(key, file, abort.signal, at)
    })
  }

  private async run(key: string, file: File, signal: AbortSignal, dropped: { x: number; y: number; w: number }) {
    try {
      const digest = await this.inLine(() => this.hashAndSend(key, file, signal))
      await this.store.whenNamed(key, signal)
      try {
        await this.ask(key, digest, dropped)
      } catch (error) {
        // What it held went since it looked: moved by another import, or dropped after a day. Send it again.
        if (!(error instanceof ApiError && error.body.code === "file_not_held")) throw error
        await this.inLine(() => this.send(key, file, digest, signal))
        await this.ask(key, digest, dropped)
      }
    } catch (error) {
      if (signal.aborted) return
      say(file.name, error)
      this.store.cancel(key)
    }
  }

  private async ask(key: string, digest: string, dropped: { x: number; y: number; w: number }) {
    this.stage(key, "adding", 1)
    const at = this.store.position(key) ?? dropped
    const name = this.store.get(key)!.name
    this.store.beginAdding()
    try {
      const id = await this.project()
      const tried = (attempt: number) => this.store.patchPlacing(key, { attempt: attempt || undefined })
      const answer = await this.client.import(id, `import-${crypto.randomUUID()}`, { source: digest, name, at }, tried)
      this.store.land(key, answer, at)
      if (answer.note) say(answer.note)
      this.hooks.refresh(id)
    } finally {
      this.store.endAdding()
    }
  }

  /** Bytes move one file at a time, on one connection: the page has few to spare. */
  private inLine<T>(work: () => Promise<T>): Promise<T> {
    const turn = this.line.then(work, work)
    this.line = turn.catch(() => {})
    return turn
  }

  private stage(key: string, stage: Stage, done: number) {
    this.store.patchPlacing(key, { stage, done, attempt: undefined })
  }

  private async hashAndSend(key: string, file: File, signal: AbortSignal): Promise<string> {
    signal.throwIfAborted()
    this.stage(key, "reading", 0)
    let shown = 0
    const digest = await hash(file, signal, (done) => {
      if (done - shown < 0.01 && done < 1) return
      shown = done
      this.stage(key, "reading", done)
    })
    // A file the project has needs no bytes: its import moved them out of what's held, so `held` wouldn't know.
    if (!this.store.holdsFile(digest.slice("sha256:".length))) await this.send(key, file, digest, signal)
    return digest
  }

  private async send(key: string, file: File, digest: string, signal: AbortSignal) {
    signal.throwIfAborted()
    this.stage(key, "sending", 0)
    const tried = (attempt: number) => this.store.patchPlacing(key, { attempt: attempt || undefined })
    if (file.size === 0) {
      await this.client.put(digest, file, 0, 0, signal, tried)
      return
    }
    // Sent whole and still not whole means what it held expired under it, so ask again; a third time isn't chance.
    for (let pass = 0; pass < 3; pass++) {
      const held = await this.client.held(digest, signal, tried)
      if (held === true) return
      const missing = gaps(held, file.size)
      let sent = file.size - missing.reduce((sum, [from, to]) => sum + to - from, 0)
      this.stage(key, "sending", sent / file.size)
      for (const [from, to] of missing) {
        for (let at = from; at < to; at += PART_BYTES) {
          const end = Math.min(to, at + PART_BYTES)
          const whole = await this.client.put(digest, file.slice(at, end), at, file.size, signal, tried)
          sent += end - at
          this.stage(key, "sending", sent / file.size)
          if (whole) return
        }
      }
    }
    throw new Error("Calcifer took every part and doesn't hold the file")
  }

  private project(): Promise<string> {
    if (this.store.projectId) return Promise.resolve(this.store.projectId)
    this.made ??= this.client.newProject("untitled", `project-${crypto.randomUUID()}`).then(({ id }) => {
      this.store.projectId = id
      this.hooks.adopt(id)
      return id
    })
    return this.made
  }
}
