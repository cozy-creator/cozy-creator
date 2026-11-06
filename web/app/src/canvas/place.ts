import { ApiError, type Client } from "../api/client"
import { lost } from "../api/outbox"
import { hashFile, inLine, sendFile } from "../api/upload"
import { say } from "../ui/notes"
import type { Asset, Imported, Row, Tool } from "../types"
import { CARD_WIDTH, type CanvasStore, type CarriedFile, type MediaKind, type Stage, UNDO_MS } from "./CanvasStore"
import { startingFields, TEXT_TOOL } from "./controls"
import type { CopiedBox } from "./copy"
import { asName, stemName } from "./names"

const GAP = 32

function mediaOf(file: File): MediaKind {
  const [type] = file.type.split("/")
  return type === "image" || type === "video" || type === "audio" ? type : "file"
}

export interface Hooks {
  /** The first import on an empty canvas made this project. */
  adopt(id: string): void
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
  private projectKey = `project-${crypto.randomUUID()}`
  private nextKey = 0

  constructor(store: CanvasStore, client: Client, hooks: Hooks) {
    this.store = store
    this.client = client
    this.hooks = hooks
  }

  /** One file is named by its field, focused; several take their stems, laid out from the point, with no focus. With `then`, none asks: it's told the name each gets. `edit`: what they're part of. */
  add(files: File[], point: { x: number; y: number }, then?: (name: string) => void, edit?: string) {
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
        naming: files.length === 1 && !then,
        preview: media === "image" ? URL.createObjectURL(file) : undefined,
      })
      void this.run(key, file, abort.signal, at, then, edit)
    })
  }

  /** A tool dropped at a point: an asset of it, there at once, as wide as what it makes and called what its tool says. A tool's box opens on its words, the caret in them; a text box takes the keys where it stands, and an Edit stays shut. */
  addTool(tool: Tool, point: { x: number; y: number }) {
    const at = { x: Math.round(point.x - CARD_WIDTH / 2), y: Math.round(point.y - 16) }
    const text = tool === TEXT_TOOL
    const timeline = tool.controls.some((control) => control.type === "timeline") ? { fps: 24, tracks: [{ clips: [] }] } : undefined
    const name = this.box(asName(tool.name ?? tool.label), { kind: tool.kind, tool: text ? undefined : tool.id, fields: text ? { text: "" } : startingFields(tool), timeline }, at, `Couldn't place ${tool.label}`)
    this.store.focusing = name
    if (!text && !timeline) this.store.openHere(name)
  }

  /** An asset that holds these, there at once, with nothing made yet. */
  private box(wanted: string, { kind, tool, fields, timeline }: Pick<Asset, "kind" | "tool" | "fields" | "timeline">, at: Row, failed: string, edit?: string): string {
    const key = `asset-${crypto.randomUUID()}`
    const text = !tool
    const body = text ? { tool: "", name: wanted, fields } : { tool, name: wanted, fields, kind }
    const row: Asset = { name: "", standing: text ? "current" : "missing", kind, noun: kind === "video" ? "take" : "version", tool, versions: 0, status: "", fields, timeline }
    return this.store.place(wanted, { asset: row }, at, async (clear, write) => this.client.newAsset(await this.project(), key, { ...body, at: clear }, write), (error) => say(failed, error), undefined, edit)
  }

  /** Boxes copied, laid out as they were and together clear of what's there, around a point. All of them are there at once, in the queue like anything else, and a note offers Undo. */
  paste(boxes: CopiedBox[], point: { x: number; y: number }) {
    const left = Math.min(...boxes.map(({ at }) => at.x))
    const top = Math.min(...boxes.map(({ at }) => at.y))
    const w = Math.max(...boxes.map(({ at }) => at.x + at.w)) - left
    const h = Math.max(...boxes.map(({ at }) => at.y + at.h)) - top
    const spot = this.store.clearFor({ x: Math.round(point.x - w / 2), y: Math.round(point.y - h / 2), w, h })
    const edit = this.store.newStep(`Pasted ${boxes.length === 1 ? "a box" : `${boxes.length} boxes`}`)
    const names = boxes.map((box) => {
      const at = { x: spot.x + box.at.x - left, y: spot.y + box.at.y - top, w: box.at.w }
      return "file" in box ? this.file(box.file, at, edit) : this.box(box.made.name, box.made, at, `Couldn't paste ${box.made.name}`, edit)
    })
    this.pasted(names, edit)
  }

  /** Files from the system's clipboard: each a box near a point, named by what it's called there, and Undo. */
  pasteFiles(files: File[], point: { x: number; y: number }) {
    const names: string[] = []
    const edit = this.store.newStep(`Pasted ${files.length === 1 ? files[0].name : `${files.length} files`}`)
    this.add(
      files,
      point,
      (name) => {
        names.push(name)
        if (names.length === files.length) this.pasted(names, edit)
      },
      edit,
    )
  }

  private pasted(names: string[], edit: string) {
    say(`Pasted ${names.length === 1 ? names[0] : `${names.length} assets`}`, undefined, { action: { label: "Undo", run: () => this.store.undo({ edit }) }, ms: UNDO_MS })
  }

  /** Files dropped on a box with no tool: each its next version, with its slot in the row until it's there. */
  join(files: File[], name: string) {
    for (const file of files) {
      const landed = this.store.joining(name)
      const owed = this.store.outbox.owing()
      const signal = new AbortController().signal
      const sent = async () => {
        const digest = await hashFile(file, signal, () => {})
        if (!this.store.holdsFile(digest.slice("sha256:".length))) await this.reaching(() => sendFile(this.client, file, digest, signal, () => {}, () => {}), signal)
        return digest
      }
      void inLine(sent)
        .then((digest) => this.store.joinVersion(name, { sha256: digest.slice("sha256:".length), field: mediaOf(file) }, digest, file.name), (error) => say(file.name, error))
        .finally(() => {
          landed()
          owed()
        })
    }
  }

  /** A take or version dragged out: its own asset with the same file and what it says it is, there at once, and no new bytes. */
  /** A take carried out of its box onto the canvas moves there (`movedOut`), once its new box is made. */
  addFile(file: CarriedFile, point: { x: number; y: number }) {
    this.file(file, { x: Math.round(point.x - CARD_WIDTH / 2), y: Math.round(point.y - 16), w: CARD_WIDTH })
  }

  private file(file: CarriedFile, at: Row, edit?: string): string {
    const key = `import-${crypto.randomUUID()}`
    const { description, from, number } = file
    return this.store.place(
      asName(file.name),
      { landed: { sha256: file.sha256, field: file.field } },
      at,
      async (clear, write) => this.client.import(await this.project(), key, { source: file.sha256, name: file.name, at: clear, description }, write),
      (error) => say(`Couldn't take out ${file.name}`, error),
      (name, _, made) => from !== undefined && number !== undefined && this.store.movedOut(from, number, name, made),
      edit,
    )
  }

  private async run(key: string, file: File, signal: AbortSignal, dropped: { x: number; y: number; w: number }, then?: (name: string) => void, edit?: string) {
    const owed = this.store.outbox.owing()
    try {
      const digest = await inLine(() => this.hashAndSend(key, file, signal))
      await this.store.whenNamed(key, signal)
      // Its import is in the queue from here, and counted there.
      owed()
      try {
        const name = await this.ask(key, digest, dropped, edit)
        then?.(name)
      } catch (error) {
        // What it held went since it looked: moved by another import, or dropped after a day. Send it again.
        if (!(error instanceof ApiError && error.body.code === "file_not_held")) throw error
        const again = this.store.outbox.owing()
        await inLine(() => this.send(key, file, digest, signal)).finally(again)
        const name = await this.ask(key, digest, dropped, edit)
        then?.(name)
      }
    } catch (error) {
      if (signal.aborted) return
      say(file.name, error)
      this.store.cancel(key)
    } finally {
      owed()
    }
  }

  private async ask(key: string, digest: string, dropped: { x: number; y: number; w: number }, given?: string): Promise<string> {
    this.stage(key, "adding", 1)
    const at = this.store.position(key) ?? dropped
    const name = this.store.get(key)!.name
    const edit = given ?? this.store.newStep(`Added ${name}`)
    this.store.joinStep(edit, [key])
    this.store.beginAdding()
    try {
      const once = `import-${crypto.randomUUID()}`
      const answer = await new Promise<Imported>((done, refused) => this.store.outbox.add({ send: async (signal) => this.client.import(await this.project(), once, { source: digest, name, at }, { once: signal, edit }), done, refused }))
      this.store.land(key, answer, at, edit)
      if (answer.note) say(answer.note)
      return answer.name
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
    return this.reaching(() => {
      this.stage(key, "sending", 0)
      return sendFile(
        this.client,
        file,
        digest,
        signal,
        (done) => this.stage(key, "sending", done),
        (attempt) => this.store.patchPlacing(key, { attempt: attempt || undefined }),
      )
    }, signal)
  }

  /** Bytes go again each time they can't reach Calcifer, for as long as it's away: what's dropped meanwhile goes when it's back. */
  private async reaching(work: () => Promise<void>, signal: AbortSignal) {
    for (let tries = 1; ; tries++) {
      try {
        return await work()
      } catch (error) {
        if (signal.aborted || !lost(error)) throw error
        await this.store.outbox.wait(tries, signal)
      }
    }
  }

  /** The project this canvas makes with its first box, under one key however often it's asked: one that didn't reach Calcifer is asked again. */
  private project(): Promise<string> {
    if (this.store.projectId) return Promise.resolve(this.store.projectId)
    this.made ??= this.client.newProject("untitled", this.projectKey).then(
      ({ id, said }) => {
        this.store.projectId = id
        for (const line of said ?? []) say(line)
        this.hooks.adopt(id)
        return id
      },
      (error) => {
        this.made = undefined
        throw error
      },
    )
    return this.made
  }
}
