import { ApiError, type Client } from "../api/client"
import { lost } from "../api/outbox"
import { hashFile, inLine, sendFile } from "../api/upload"
import { say } from "../ui/notes"
import type { Asset, Imported, Card, Tool, Opened } from "../api/calcifer"
import { CARD_WIDTH, type CanvasStore, type CarriedFile, type MediaKind, type Pushes, type Stage, UNDO_MS } from "./CanvasStore"
import { startingFields, TEXT_TOOL } from "./controls"
import type { BasicNodeKind, Operation } from "./palette"
import { type CopiedBox, copyOf } from "./copy"
import { asName, nameKey, stemName } from "./names"
import { pictureSize } from "./pictureSize"
import { heightFor, underOf } from "./shown"
import { mentionsMoved } from "./text"

const GAP = 32
/** How long a drop waits for its pictures to decode before it's placed anyway, each then drawn as it's ready. */
const DECODE_MS = 1500

function mediaOf(file: File): MediaKind {
  const [type] = file.type.split("/")
  return type === "image" || type === "video" || type === "audio" ? type : "file"
}

type Readied = { url: string; aspect?: number }

/** A dropped picture's own bytes, to show until it's somewhere else: decoded, so its box is drawn with it, and its shape as it's drawn, from its header, else
 *  its decode. What the browser decodes says last, as that's what's shown. */
async function readied(file: File): Promise<Readied> {
  const url = URL.createObjectURL(file)
  const image = new Image()
  image.src = url
  const decoded = image.decode().then(
    () => (image.naturalWidth && image.naturalHeight ? image.naturalWidth / image.naturalHeight : undefined),
    () => undefined,
  )
  const [header, drawn] = await Promise.all([
    pictureSize(file).catch(() => null),
    Promise.race([decoded, new Promise<undefined>((done) => setTimeout(done, DECODE_MS))]),
  ])
  return { url, aspect: drawn ?? (header ? header.width / header.height : undefined) }
}

export interface Hooks {
  /** Collect local credentials and save recovery before sending an empty canvas's files. */
  create(): Promise<Opened>
  /** The first import on an empty canvas made this project. */
  adopt(id: string, code: string): void
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

  /** One file is named by its field, focused; several take their stems, laid out from the point, with no focus. With `then`, none asks: it's told the name each gets. `edit`: what they're part of.
   *  A picture is placed once its shape is read and it's decoded, all of a drop together: each box is there at its own shape, showing its own file, and stays as it is
   *  while the file goes to Calcifer (cl-918). */
  add(files: File[], point: { x: number; y: number }, then?: (name: string) => void, edit?: string) {
    void Promise.all(files.map((file) => (mediaOf(file) === "image" ? readied(file) : undefined))).then((pictures) => this.lay(files, pictures, point, then, edit))
  }

  private lay(files: File[], pictures: (Readied | undefined)[], point: { x: number; y: number }, then: ((name: string) => void) | undefined, edit: string | undefined) {
    const columns = Math.ceil(Math.sqrt(files.length))
    const left = point.x - CARD_WIDTH / 2
    let top = point.y - 16
    const placing = files.map((file, i) => {
      const media = mediaOf(file)
      const stem = stemName(file.name) || "file"
      const view = { name: stem, placing: { media, stage: "waiting" as const, stem, abort: new AbortController() }, aspect: pictures[i]?.aspect }
      // As tall as it is drawn, and the room it keeps under it: no box of the drop lies on another, so none of them moves.
      return { file, media, view, tall: heightFor(view, CARD_WIDTH) + underOf(view, 0) }
    })
    for (let row = 0; row * columns < placing.length; row++) {
      const boxes = placing.slice(row * columns, (row + 1) * columns)
      boxes.forEach(({ file, view }, column) => {
        const i = row * columns + column
        const key = `placing:${++this.nextKey}`
        const at = { x: Math.round(left + column * (CARD_WIDTH + GAP)), y: Math.round(top), w: CARD_WIDTH }
        // Its step is made when it's sent; what it pushes aside as it's dropped is part of it already.
        const by = edit ?? crypto.randomUUID()
        this.store.addPlacing(key, view.name, view.placing, at, { naming: files.length === 1 && !then, preview: pictures[i]?.url, aspect: view.aspect }, by)
        void this.run(key, file, view.placing.abort.signal, at, then, by, !edit)
      })
      top += Math.max(...boxes.map(({ tall }) => tall)) + GAP
    }
  }

  /** A tool dropped at a point: an asset of it, there at once, as wide as what it makes and called what its tool says, `fields` with what it starts with
   *  (a checkpoint dropped is on it). Dropping keeps the current canvas interaction mode. */
  addTool(tool: Tool, point: { x: number; y: number }, fields?: Record<string, unknown>, operation?: Operation) {
    const at = { x: Math.round(point.x - CARD_WIDTH / 2), y: Math.round(point.y - 16) }
    const text = tool === TEXT_TOOL
    const timeline = tool.fields.some((field) => field.type === "timeline") ? { fps: 24, tracks: [{ clips: [] }] } : undefined
    const name = this.box(asName(tool.name ?? tool.label), { kind: tool.makes, tool: text ? undefined : tool.id, fields: text ? { text: "", node_kind: "text" } : { ...startingFields(tool), ...fields, node_kind: tool.makes }, timeline }, at, `Couldn't place ${tool.label}`)
    if (operation) this.store.patch(name, { operation })
  }

  /** Basic media nodes retain their type before a model or file is assigned. */
  addNode(kind: BasicNodeKind, point: { x: number; y: number }) {
    if (kind === "text") return this.addTool(TEXT_TOOL, point)
    const at = { x: Math.round(point.x - CARD_WIDTH / 2), y: Math.round(point.y - 16) }
    this.box(kind, { kind, fields: { node_kind: kind } }, at, `Couldn't create ${kind}`)
  }

  /** An asset that holds these, there at once, with nothing made yet. */
  private box(wanted: string, { kind, tool, fields, timeline }: Pick<Asset, "kind" | "tool" | "fields" | "timeline">, at: Card, failed: string, edit?: string): string {
    const key = `asset-${crypto.randomUUID()}`
    const text = kind === "text"
    const body = { tool: tool ?? "", name: wanted, fields, kind }
    const row: Asset = { name: "", standing: text ? "current" : "missing", kind, noun: kind === "video" ? "take" : "version", tool, versions: 0, status: "", fields, timeline }
    return this.store.place(wanted, { asset: row }, at, async (clear, write, pushes) => this.client.newAsset(await this.project(), key, { ...body, at: clear, moves: pushes.moves, placed: pushes.placed }, write), (error) => say(failed, error), undefined, edit)
  }

  /**
   * Boxes copied, laid out as they were and together clear of what's there: their middle at `point`, else beside where they stood. A box of this project
   * is copied with its versions, all of them by one ask; one from elsewhere, or gone since, is made again. All of them are there at once, in the queue like
   * anything else, and a note offers Undo. The names they're placed under.
   */
  paste(boxes: CopiedBox[], point: { x: number; y: number } | null, verb = "Pasted"): string[] {
    const left = Math.min(...boxes.map(({ at }) => at.x))
    const top = Math.min(...boxes.map(({ at }) => at.y))
    const w = Math.max(...boxes.map(({ at }) => at.x + at.w)) - left
    const h = Math.max(...boxes.map(({ at }) => at.y + at.h)) - top
    const middle = point ?? { x: left + w * 1.5 + GAP, y: top + h / 2 }
    const edit = this.store.newStep(`${verb} ${boxes.length === 1 ? "a box" : `${boxes.length} boxes`}`)
    const spot = this.store.clearFor({ x: Math.round(middle.x - w / 2), y: Math.round(middle.y - h / 2), w, h }, edit)
    const at = (box: CopiedBox) => ({ x: Math.round(spot.x + box.at.x - left), y: Math.round(spot.y + box.at.y - top), w: Math.round(box.at.w) })
    const here = boxes.flatMap((box) => ("made" in box && box.from && this.store.get(box.from)?.asset ? [{ from: box.from, at: at(box) }] : []))
    const names = here.length ? this.copies(here, edit) : []
    for (const box of boxes) {
      if ("file" in box) names.push(this.file(box.file, at(box), edit))
      else if (!here.some(({ from }) => from === box.from)) names.push(this.box(box.made.name, box.made, at(box), `Couldn't paste ${box.made.name}`, edit))
    }
    this.pasted(names, edit, verb)
    return names
  }

  /** Each of `names` again beside where they stand, laid out as they are: Ctrl+D, and Duplicate on a box's menu. */
  duplicate(names: string[]): string[] {
    const boxes = names.flatMap((name) => copyOf(this.store, name, true) ?? [])
    return boxes.length ? this.paste(boxes, null, "Duplicated") : []
  }

  /**
   * Boxes of this project copied, each at its row, by one ask: one transaction and one entry of the history, their versions with them and no bytes copied.
   * Each shows its box's at once, a mention of another copied with it naming that one's copy, as Calcifer writes it.
   */
  private copies(boxes: { from: string; at: Card }[], edit: string): string[] {
    const key = `copies-${crypto.randomUUID()}`
    const placing = boxes.map(({ from, at }) => {
      const view = this.store.get(from)!
      const asset: Asset = { ...view.asset!, fields: { ...view.asset!.fields, ...view.sent, ...view.drafts }, etag: undefined, making: undefined, rendering: undefined }
      return { wanted: asset.name, view: { asset, aspect: view.aspect, operation: view.operation, viewed: view.viewed }, dropped: at }
    })
    const said = boxes.length === 1 ? boxes[0].from : `${boxes.length} boxes`
    const names = this.store.placeAll(
      placing,
      async (ats, write, pushes) => {
        const asked = boxes.map(({ from }, i) => ({ from: this.store.servedName(from), at: ats[i] }))
        return this.client.copies(await this.project(), key, { boxes: asked, moves: pushes.moves, placed: pushes.placed }, write)
      },
      (error) => say(`Couldn't copy ${said}`, error),
      undefined,
      edit,
    )
    const moved = new Map(boxes.map(({ from }, i) => [nameKey(from), names[i]]))
    for (const name of names) {
      const asset = this.store.get(name)?.asset
      if (asset) this.store.patch(name, { asset: { ...asset, fields: mentionsMoved(asset.fields, moved) as Asset["fields"] } })
    }
    return names
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

  private pasted(names: string[], edit: string, verb = "Pasted") {
    say(`${verb} ${names.length === 1 ? names[0] : `${names.length} assets`}`, undefined, { action: { label: "Undo", run: () => this.store.undo({ edit }) }, ms: UNDO_MS })
  }

  /** Files dropped on a box with no tool: each its next version, with its slot in the row until it's there. */
  join(files: File[], name: string) {
    const kind = this.store.get(name)?.asset?.kind
    for (const file of files) {
      if (kind && file.type && file.type.split("/")[0] !== kind) {
        say(`${name} is a ${kind} node and cannot take ${file.name}`)
        continue
      }
      const landed = this.store.joining(name)
      const owed = this.store.outbox.owing()
      const signal = new AbortController().signal
      const sent = async () => {
        const projectId = await this.project()
        const digest = await hashFile(file, signal, () => {})
        if (!this.store.holdsFile(digest.slice("sha256:".length))) await this.reaching(() => sendFile(this.client, file, digest, signal, () => {}, () => {}, projectId), signal)
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

  private file(file: CarriedFile, at: Card, edit?: string): string {
    const key = `import-${crypto.randomUUID()}`
    const { description, from, number } = file
    return this.store.place(
      asName(file.name),
      { landed: { sha256: file.sha256, field: file.field } },
      at,
      async (clear, write, pushes) => this.client.import(await this.project(), key, { source: file.sha256, name: file.name, at: clear, description, moves: pushes.moves, placed: pushes.placed }, write),
      (error) => say(`Couldn't take out ${file.name}`, error),
      (name, _, made) => !file.copy && from !== undefined && number !== undefined && this.store.movedOut(from, number, name, made),
      edit,
    )
  }

  /** `own`: `edit` is this file's alone, its step made as it's sent. */
  private async run(key: string, file: File, signal: AbortSignal, dropped: { x: number; y: number; w: number }, then: ((name: string) => void) | undefined, edit: string, own: boolean) {
    const owed = this.store.outbox.owing()
    try {
      await this.project()
      signal.throwIfAborted()
      const digest = await inLine(() => this.hashAndSend(key, file, signal))
      await this.store.whenNamed(key, signal)
      // Its import is in the queue from here, and counted there.
      owed()
      try {
        const name = await this.ask(key, digest, dropped, edit, own)
        then?.(name)
      } catch (error) {
        // What it held went since it looked: moved by another import, or dropped after a day. Send it again.
        if (!(error instanceof ApiError && error.body.code === "file_not_held")) throw error
        const again = this.store.outbox.owing()
        await inLine(() => this.send(key, file, digest, signal)).finally(again)
        const name = await this.ask(key, digest, dropped, edit, own)
        then?.(name)
      }
    } catch (error) {
      if (signal.aborted) return
      say(file.name, error)
      this.store.cancel(key, edit)
    } finally {
      owed()
    }
  }

  private async ask(key: string, digest: string, dropped: { x: number; y: number; w: number }, edit: string, own: boolean): Promise<string> {
    this.stage(key, "adding")
    const at = this.store.position(key) ?? dropped
    const name = this.store.get(key)!.name
    if (own && !this.store.history.stepOf({ edit })) this.store.newStep(`Added ${name}`, [], edit)
    this.store.joinStep(edit, [key])
    this.store.beginAdding()
    try {
      const once = `import-${crypto.randomUUID()}`
      // What its drop pushed aside goes with it, the same on every try: one request, one transaction, one entry.
      let pushes: Pushes | undefined
      const send = async (signal: AbortSignal) => {
        pushes ??= this.store.pushesFor(edit)
        return this.client.import(await this.project(), once, { source: digest, name, at, moves: pushes.moves, placed: pushes.placed }, { once: signal, edit })
      }
      const answer = await new Promise<Imported>((done, refused) => this.store.outbox.add({ send, done, refused }))
      if (pushes) this.store.pushesSent(pushes, answer.kept)
      this.store.land(key, answer, at, edit)
      return answer.name
    } finally {
      this.store.endAdding()
    }
  }

  /** Where it's got, which nothing shows: only Cancel reads it. */
  private stage(key: string, stage: Stage) {
    if (this.store.get(key)?.placing?.stage !== stage) this.store.patchPlacing(key, { stage })
  }

  private async hashAndSend(key: string, file: File, signal: AbortSignal): Promise<string> {
    signal.throwIfAborted()
    this.stage(key, "reading")
    const digest = await hashFile(file, signal, () => {})
    // A file the project has needs no bytes: its import moved them out of what's held, so `held` wouldn't know.
    if (!this.store.holdsFile(digest.slice("sha256:".length))) await this.send(key, file, digest, signal)
    return digest
  }

  private async send(key: string, file: File, digest: string, signal: AbortSignal) {
    const projectId = await this.project()
    return this.reaching(() => {
      this.stage(key, "sending")
      return sendFile(this.client, file, digest, signal, () => {}, () => {}, projectId)
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

  /** One setup for all pending imports. Credentials stay in the local setup form, outside the outbox. */
  private project(): Promise<string> {
    if (!this.store.active) return Promise.reject(new DOMException("Project view retired", "AbortError"))
    if (this.store.projectId) return Promise.resolve(this.store.projectId)
    this.made ??= this.hooks.create().then(
      ({ id, code, said }) => {
        if (!this.store.active) throw new DOMException("Project view retired", "AbortError")
        this.store.projectId = id
        for (const line of said ?? []) say(line)
        this.hooks.adopt(id, code ?? "")
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
