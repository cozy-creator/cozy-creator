import { ApiError, type Client } from "../api/client"
import { hashFile, inLine, sendFile } from "../api/upload"
import { say } from "../ui/notes"
import type { Tool } from "../types"
import { CARD_WIDTH, type CanvasStore, type MediaKind, type Stage } from "./CanvasStore"
import { stemName } from "./names"

const GAP = 32

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

  /** A tool dropped at a point: an asset of it, opening on its controls, and as wide as what it makes. Local code is named by Calcifer, from its function. */
  async addTool(tool: Tool, point: { x: number; y: number }) {
    const at = { x: Math.round(point.x - CARD_WIDTH / 2), y: Math.round(point.y - 16) }
    try {
      const id = await this.project()
      const made = await this.client.newAsset(id, `asset-${crypto.randomUUID()}`, { tool: tool.id, at, name: tool.local ? undefined : tool.label })
      this.store.expect(made.name, at)
      this.hooks.refresh(id)
    } catch (error) {
      say(`Couldn't place ${tool.label}`, error)
    }
  }

  /** A take or version dragged out: its own asset with the same file and what it says it is, and no new bytes. */
  async addFile(file: { sha256: string; name: string; description?: string }, point: { x: number; y: number }) {
    const at = { x: Math.round(point.x - CARD_WIDTH / 2), y: Math.round(point.y - 16), w: CARD_WIDTH }
    try {
      const id = await this.project()
      const made = await this.client.import(id, `import-${crypto.randomUUID()}`, { source: file.sha256, name: file.name, at })
      this.store.expect(made.name, at)
      this.store.focusing = null
      if (file.description) await this.client.patchAsset(id, made.name, { fields: { description: file.description } })
      this.hooks.refresh(id)
    } catch (error) {
      say(`Couldn't take out ${file.name}`, error)
    }
  }

  private async run(key: string, file: File, signal: AbortSignal, dropped: { x: number; y: number; w: number }) {
    try {
      const digest = await inLine(() => this.hashAndSend(key, file, signal))
      await this.store.whenNamed(key, signal)
      try {
        await this.ask(key, digest, dropped)
      } catch (error) {
        // What it held went since it looked: moved by another import, or dropped after a day. Send it again.
        if (!(error instanceof ApiError && error.body.code === "file_not_held")) throw error
        await inLine(() => this.send(key, file, digest, signal))
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

  private stage(key: string, stage: Stage, done: number) {
    this.store.patchPlacing(key, { stage, done, attempt: undefined })
  }

  private async hashAndSend(key: string, file: File, signal: AbortSignal): Promise<string> {
    signal.throwIfAborted()
    this.stage(key, "reading", 0)
    let shown = 0
    const digest = await hashFile(file, signal, (done) => {
      if (done - shown < 0.01 && done < 1) return
      shown = done
      this.stage(key, "reading", done)
    })
    // A file the project has needs no bytes: its import moved them out of what's held, so `held` wouldn't know.
    if (!this.store.holdsFile(digest.slice("sha256:".length))) await this.send(key, file, digest, signal)
    return digest
  }

  private send(key: string, file: File, digest: string, signal: AbortSignal) {
    this.stage(key, "sending", 0)
    return sendFile(
      this.client,
      file,
      digest,
      signal,
      (done) => this.stage(key, "sending", done),
      (attempt) => this.store.patchPlacing(key, { attempt: attempt || undefined }),
    )
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
