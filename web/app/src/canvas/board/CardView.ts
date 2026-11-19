import { Container, Graphics, Rectangle, Sprite, Texture, TilingSprite } from "pixi.js"
import type { Box } from "../graph"
import { BODY, FAR, HEAD, nameTopOf, SMALL, SUB_ROW, TITLE, TRASH_TOP, UNDER, UNTOOL, hexOf } from "../shown"
import { chipWords, withMentions } from "../text"
import { pictureBox, type Presentation, type SceneNode, type ScenePicture } from "./presentation"
import type { LatentHandle, Media, MediaHandle, MediaSource, VideoHandle } from "./Media"
import { ellipsis, Label, type LabelStyle, width, wrap } from "./text"

export type Part = "body" | "title" | "delete" | "cancel" | "corner" | "untool" | "dot" | "waits"
export interface Hit { name: string; part: Part }

/** Every colour comes from the page's tokens (tokens.css), read once per theme. */
export interface Palette {
  card: number; well: number; line: number; ink: number; mute: number; select: number
  queued: number; running: number; unseen: number; failed: number; ember: number; ember2: number; check: string; check2: string
  box: number; edge: number; edgeAlpha: number; shade: number; shadeAlpha: number
  line2: number; mark: number; markAlpha: number; mark2: number; mark2Alpha: number
}
export interface CardState { hover: Part | null; dragging: boolean; offering: boolean; resizing: string | null; dropTarget: boolean; broken: boolean }
export interface CardEnv {
  palette: Palette; theme: string; zoom: number; band: 0 | 1 | 2; titleSize: number; resolution: number; epoch: number; moving: boolean
  media: Media; reduced: boolean
  /** A software renderer, or frames measured slow: decorative animation stands still. */
  software: boolean; invalidate(): void
  request(picture: ScenePicture, width: number): { source: MediaSource; width: number; local: boolean } | null
  onRevealed(name: string, aspect: number | undefined, local: boolean): void
}

const hits = new WeakMap<Container, Hit>()
/** One-line labels in small type share a glyph atlas (BitmapText) instead of a canvas and texture each: a card's open costs its title and words. */
const SMALL_LABELS = new Set(["version", "tool", "shape", "machine", "time", "count", "duration", "kind", "ratio"])
/** The card part a federated event's target belongs to. */
export function hitOf(target: Container | null): Hit | null {
  for (let at: Container | null = target; at; at = at.parent) { const hit = hits.get(at); if (hit) return hit }
  return null
}

interface Picture { sprite: Sprite; key: string; handle?: MediaHandle; next?: MediaHandle; nextKey?: string; shownKey?: string; crop?: Texture; cover: boolean; box: Box; main: boolean; local: boolean; aspect?: number }

/** One card's display objects, made once and changed in place: a hover, a store echo or a move never rebuilds them. */
export class CardView {
  readonly root = new Container({ eventMode: "passive" })
  private ground = new Graphics()
  private pixels = new Container()
  private ink = new Graphics()
  private pulse = new Graphics()
  private labels = new Container()
  private parts = new Map<Part, Container>()
  /** The X beside a box's tool that takes it off (Paul, 11-16: "You are not deleting anything; you are closing it"): drawn in screen
   *  pixels, an 8 px glyph in an 18 px hit area at every zoom, where it sits after the tool's name. */
  private untool = { glyph: new Graphics(), at: null as { x: number; y: number } | null, limit: 0, color: -1, inked: -1 }
  private texts = new Map<string, Label>()
  private pictures = new Map<string, Picture>()
  private checker?: TilingSprite
  private latent?: { run: number; handle: LatentHandle; sprite: Sprite }
  private video?: { sha: string; handle: VideoHandle; sprite: Sprite }
  private signature = ""
  private inputs: unknown[] = []
  private name = ""
  private input?: { node: SceneNode; show: Presentation; state: CardState; env: CardEnv }
  private began = performance.now()
  private pulsing = false
  /** For inspection: each label's full text, the chips and controls as drawn, in card-local units. */
  readonly drawn = { full: new Map<string, string>(), chips: [] as { text: string; struck: boolean; box: Box }[], controls: [] as { part: Part; box: Box }[], parts: new Map<Part, Box>(), meter: null as Box | null, dot: null as Box | null, unseen: null as Box | null, orbit: null as Box | null }

  constructor() {
    this.pulse.eventMode = this.ground.eventMode = this.ink.eventMode = "none"
    this.pixels.eventMode = this.labels.eventMode = "none"
    this.root.addChild(this.pulse, this.ground, this.pixels, this.ink, this.labels)
    for (const part of ["body", "title", "delete", "cancel", "corner", "untool", "dot", "waits"] as Part[]) {
      const area = new Container({ eventMode: "static" })
      area.hitArea = new Rectangle(0, 0, 0, 0)
      area.cursor = part === "corner" ? "nwse-resize" : part === "body" || part === "dot" ? "default" : "pointer"
      area.visible = false
      this.parts.set(part, area)
      this.root.addChild(area)
    }
    const untool = this.parts.get("untool")!
    untool.hitArea = new Rectangle(-UNTOOL.hit / 2, -UNTOOL.hit / 2, UNTOOL.hit, UNTOOL.hit)
    this.untool.glyph.eventMode = "none"
    untool.addChild(this.untool.glyph)
  }

  get picture() { return this.pictures.get("main") }
  get referencesShown() {
    return [...this.pictures].filter(([key]) => key.startsWith("ref-")).map(([key, picture]) => ({ key, ready: picture.sprite.visible, box: { x: picture.sprite.x, y: picture.sprite.y, w: picture.sprite.width, h: picture.sprite.height }, sha: JSON.parse(picture.key || "[{}]")[0]?.sha256 as string | undefined }))
  }
  get latentShown() { const latent = this.latent; return latent?.handle.texture ? { width: latent.handle.texture.width, opacity: latent.sprite.alpha } : null }
  get previewing() { return this.video ? { active: true, ready: Boolean(this.video.handle.texture), muted: true, loop: true } : null }

  update(node: SceneNode, show: Presentation, state: CardState, env: CardEnv) {
    this.input = { node, show, state, env }
    if (node.name !== this.name) { this.name = node.name; for (const [part, area] of this.parts) hits.set(area, { name: node.name, part }) }
    // Compared in place, not joined into a string: a frame of a pan allocates nothing per card.
    const inputs = this.inputs
    const next = [node.box.w, node.box.h, node.selected, node.view.draft !== undefined, state.hover, state.resizing, state.dropTarget, state.broken,
      env.theme, env.band, env.titleSize, env.resolution, env.epoch] as const
    let same = this.shown === show && this.signature !== "" && inputs.length === next.length
    for (let i = 0; i < next.length; i++) if (inputs[i] !== next[i]) { inputs[i] = next[i]; same = false }
    if (same) return
    this.signature = "drawn"
    this.shown = show
    this.draw()
  }
  private shown?: Presentation

  /** Draw again from scratch on the next update (context restored, fonts loaded). */
  refresh() {
    this.signature = ""; this.shown = undefined
    // After a lost context, every texture this card shows uploads again from its kept source.
    for (const picture of this.pictures.values()) { picture.handle?.texture?.source.update(); picture.crop?.source.update() }
  }

  /** A card whose drawing threw: a plain tile, still selectable and openable. */
  fault(node: SceneNode, env: CardEnv) {
    this.ground.clear(); this.ink.clear(); this.pulse.clear()
    for (const label of this.texts.values()) label.view.visible = false
    for (const picture of this.pictures.values()) picture.sprite.visible = false
    this.ground.rect(0, HEAD, node.box.w, Math.max(1, node.box.h - HEAD)).fill(env.palette.well)
    this.label("note", "Couldn't draw this box", 12, HEAD + 12, node.box.w - 24, { size: BODY, color: env.palette.failed }, env)
    for (const area of this.parts.values()) area.visible = false
    this.area("body", { x: 0, y: 0, w: node.box.w, h: node.box.h })
    this.signature = ""
  }

  private draw() {
    const { node, show, state, env } = this.input!
    const colors = env.palette
    const far = env.band > 0
    const w = node.box.w
    const body = pictureBox(node)
    body.x -= node.box.x; body.y -= node.box.y
    const used = new Set<string>(), pictures = new Set<string>()
    this.ground.clear(); this.ink.clear()
    this.drawn.chips = []; this.drawn.controls = []; this.drawn.parts.clear(); this.drawn.meter = null; this.drawn.dot = null; this.drawn.unseen = null; this.drawn.orbit = null
    for (const area of this.parts.values()) area.visible = false
    const label = (key: string, text: string, x: number, y: number, room: number, style: LabelStyle, lines = 1) => { used.add(key); return this.label(key, text, x, y, room, style, env, lines) }
    this.untool.at = null

    if (show.entity) {
      this.ground.roundRect(0, 0, w, node.box.h, 8).fill(colors.card).stroke({ width: 1, color: colors.line })
      this.entity(show, w, colors, label, pictures, env)
    } else {
      // A box stands off the sheet: a soft shadow and an edge; one with nothing in it is a lighter sheet, not a hole.
      const lift = 1 / Math.max(0.25, env.zoom)
      if (!far) for (const [grow, alpha] of [[6, 0.25], [3, 0.45], [1, 0.8]]) this.ground.rect(body.x - grow * lift, body.y - grow * lift + 2 * lift, body.w + grow * 2 * lift, body.h + grow * 2 * lift).fill({ color: colors.shade, alpha: colors.shadeAlpha * alpha * 0.35 })
      const blank = !(show.picture || show.note || show.error || show.references.length)
      this.ground.rect(body.x, body.y, body.w, body.h).fill(blank ? colors.box : colors.well)
        .stroke({ width: lift, color: colors.edge, alpha: colors.edgeAlpha, alignment: 1 })
      // Nothing made yet: a viewfinder's marks, darker under the pointer.
      if (blank) this.corners(this.ground, body, state.hover ? colors.mark2 : colors.mark, state.hover ? colors.mark2Alpha : colors.markAlpha)
      this.alphaBacking(show.picture?.alpha ? body : null, env)
      if (show.picture) this.picturePart("main", show.picture, body, true, false, pictures, env)
      if (show.references.length) this.references(show, body, pictures, colors, env)
      const failed = this.pictures.get("main")
      const unavailable = Boolean(failed?.handle?.failed && !failed.handle.texture)
      const sentence = unavailable ? "This image isn't available" : show.error ?? show.note
      // Far out a box is its picture and its name: words too small to read aren't drawn (look-at-your-own-ui).
      if (sentence && !far && show.picture && !unavailable && !show.error) {
        // Over a picture, a note sits on a pill along its foot.
        const room = Math.min(body.w - 16, width(sentence, SMALL) + 16)
        this.ink.roundRect(body.x + (body.w - room) / 2, body.y + body.h - SMALL * 1.6 - 8, room, SMALL * 1.6, 5).fill({ color: colors.card, alpha: 0.9 })
        label("note", ellipsis(sentence, room - 16, SMALL), body.x + (body.w - room) / 2 + 8, body.y + body.h - SMALL * 1.5 - 8, room - 16, { size: SMALL, color: colors.mute })
      } else if (sentence && !far) label(unavailable ? "unavailable" : "note", sentence, body.x + 12, body.y + Math.max(8, body.h / 2 - BODY * 1.25), body.w - 24, { size: BODY, color: show.error ? colors.failed : colors.mute }, 2)
      if (!far && show.words && (!show.picture || node.view.asset?.standing === "making")) this.words(show, body, colors, used, env)
      if (show.duration && !far) {
        const room = width(show.duration, SMALL, true, true) + 12
        this.ink.roundRect(body.x + body.w - room - 6, body.y + body.h - SMALL * 1.5 - 6, room, SMALL * 1.5, 4).fill({ color: 0x000000, alpha: 0.55 })
        label("duration", show.duration, body.x + body.w - room, body.y + body.h - SMALL * 1.5 - 5, room, { size: SMALL, color: 0xffffff, mono: true, bold: true })
      }
      if (show.orbit && !far) this.orbitMark(body)
      if (show.progress?.phase === "local") {
        this.ink.rect(body.x, body.y, body.w * (show.progress.fraction ?? 0), body.h).fill({ color: colors.running, alpha: 0.2 })
        if (!far) label("progress", show.progress.state, body.x + 12, body.y + body.h / 2 - BODY / 2, body.w - 24, { size: BODY, color: colors.ink, bold: true })
      }
      this.header(show, node, state, w, far, colors, label, env)
      if (!far) this.under(show, body, w, colors, label, env.zoom)
    }
    if (show.progress?.fraction !== undefined && !node.view.canceling) {
      const meter = { x: body.x, y: body.y, w: body.w * Math.min(0.99, show.progress.fraction), h: Math.min(body.h, 3 / env.zoom) }
      this.ink.rect(meter.x, meter.y, meter.w, meter.h).fill(colors.running)
      this.drawn.meter = meter
    }
    this.hot(node, body, colors)
    this.untool.color = colors.mute
    this.pin(env.zoom)
    if (state.dropTarget) this.dashed(this.ink, { x: body.x - 4, y: body.y - 4, w: body.w + 8, h: body.h + 8 }, colors.ember, 2 / Math.min(1, env.zoom))
    this.area("body", body)
    const canResize = Boolean(node.view.asset) && !node.view.placing && !far && !show.entity
    const corner = { x: body.x + body.w - 18, y: body.y + body.h - 18, w: 22, h: 22 }
    if (canResize && (state.hover || node.selected || state.resizing)) {
      const c = corner
      this.ink.moveTo(c.x + 16, c.y + 6).lineTo(c.x + 6, c.y + 16).moveTo(c.x + 16, c.y + 11).lineTo(c.x + 11, c.y + 16).stroke({ width: 1.6, color: colors.ink, alpha: 0.75 })
    }
    this.area("corner", canResize ? corner : null)
    if (state.resizing) {
      const room = width(state.resizing, BODY, false, true) + 16
      this.ink.roundRect(body.x + (body.w - room) / 2, body.y + body.h - BODY * 2.2, room, BODY * 1.6, 6).fill({ color: colors.card, alpha: 0.92 })
      label("ratio", state.resizing, body.x + (body.w - room) / 2 + 8, body.y + body.h - BODY * 2.05, room - 16, { size: BODY, color: colors.ink, bold: true })
    }
    for (const [key, text] of this.texts) if (!used.has(key)) { text.view.visible = false; this.drawn.full.delete(key) }
    for (const [key, picture] of this.pictures) if (!pictures.has(key)) this.dropPicture(key, picture)
    this.syncLatent(env)
    this.syncVideo(state, env)
  }

  private header(show: Presentation, node: SceneNode, state: CardState, w: number, far: boolean, colors: Palette, label: LabelFn, env: CardEnv) {
    const size = env.titleSize
    const y = nameTopOf(size)
    let left = 2
    const naming = node.view.draft !== undefined
    // Green first, at its top left, while a version is unseen (cl-1105); a run's queued, running or failed dot beside it.
    const dots = naming ? [] : [show.unseen ? colors.unseen : -1, show.state && show.state !== "unseen" ? colors[show.state] : -1].filter((color) => color >= 0)
    if (dots.length) {
      // As tall as the name's capitals, and apart by half of one, so two read as two at a fitted zoom (look-at-your-own-ui).
      const dot = Math.max(12, size * 0.6), gap = dot / 2, cy = y + size * 0.62
      dots.forEach((color, i) => this.ink.circle(2 + dot / 2 + i * (dot + gap), cy, dot / 2).fill(color))
      const span = dots.length * dot + (dots.length - 1) * gap
      this.drawn.dot = { x: 2, y: cy - dot / 2, w: span, h: dot }
      if (show.unseen) this.drawn.unseen = { x: 2, y: cy - dot / 2, w: dot, h: dot }
      this.area("dot", { x: 0, y: cy - dot, w: span + 4, h: dot * 2 })
      left = span + 8
    } else this.area("dot", null)
    const hasControl = !far && !naming && (node.view.asset || node.view.landed || node.view.placing)
    const hasVersion = !far && !naming && show.version ? width(show.version, SMALL) + 10 : 0
    const room = w - left - (hasControl ? 32 : 0) - hasVersion
    // A cut name under the pointer is read whole, over its own version and trash.
    const spills = state.hover === "title" && width(show.title, size, true, true) > room
    const control = hasControl && !spills
    const version = spills ? 0 : hasVersion
    // While its name is being typed, the field over it (Stage) is the name.
    // Under the pointer a cut name reads whole, on the card's colour over whatever it runs past (zoom-never-moves-nodes).
    const whole = spills
    if (whole && !naming) this.ink.roundRect(left - 4, y - 2, width(show.title, size, true, true) + 8, size * 1.25 + 4, 4).fill({ color: colors.card, alpha: 0.96 })
    const title = naming ? null : label("title", whole ? show.title : ellipsis(show.title, room, size, true, true), left, y, whole ? Infinity : room, { size, color: colors.ink, mono: true, bold: true })
    this.drawn.full.set("title", show.title)
    this.area("title", title ? { x: left, y, w: Math.max(16, width(this.texts.get("title")!.value, size, true, true)), h: size * 1.25 } : null)
    if (version) label("version", show.version!, w - (control ? 32 : 0) - version, HEAD - 10 - SMALL * 1.25, version, { size: SMALL, color: colors.mute })
    if (!control) return
    const x = w - 26, top = TRASH_TOP, hot = state.hover === "delete" || state.hover === "cancel"
    const color = hot ? colors.ink : colors.mute
    if (hot) this.ink.roundRect(x - 2, top, 28, 28, 6).fill({ color: colors.ink, alpha: 0.08 })
    if (node.view.placing) {
      this.ink.moveTo(x + 7, top + 8).lineTo(x + 19, top + 20).moveTo(x + 19, top + 8).lineTo(x + 7, top + 20).stroke({ width: 1.7, color })
      this.area("cancel", { x: x - 2, y: top, w: 28, h: 28 })
    } else {
      this.ink.moveTo(x + 6, top + 9).lineTo(x + 20, top + 9).moveTo(x + 10, top + 6).lineTo(x + 16, top + 6)
        .moveTo(x + 8, top + 11).lineTo(x + 9, top + 22).lineTo(x + 17, top + 22).lineTo(x + 18, top + 11).stroke({ width: 1.7, color })
      this.area("delete", { x: x - 2, y: top, w: 28, h: 28 })
    }
  }

  /** An orbit's mark at its picture's foot, left (Media.tsx's badge before the board): ui/Icon.tsx's `orbit`, a turn and its arrow, white on a
   *  dark disc as tall as the duration's pill. */
  private orbitMark(body: Box) {
    const d = SMALL * 1.5, x = body.x + 6, y = body.y + body.h - d - 6, s = (d - 8) / 20
    const at = (px: number, py: number): [number, number] => [x + 4 + px * s, y + 4 + py * s]
    // The icon's arc, `A6.5 6.5 0 1 0` from (16.2, 8.6) to (4, 12.6), about its centre.
    const [cx, cy] = at(9.78, 9.63)
    this.ink.circle(x + d / 2, y + d / 2, d / 2).fill({ color: 0x000000, alpha: 0.67 })
    this.ink.moveTo(...at(16.2, 8.6)).arc(cx, cy, 6.5 * s, -0.159, 2.667, true)
      .moveTo(...at(16.5, 3.8)).lineTo(...at(16.5, 8.6)).lineTo(...at(11.7, 8.6))
      .stroke({ width: 1.6 * s, color: 0xffffff, cap: "round", join: "round" })
    this.drawn.orbit = { x, y, w: d, h: d }
  }

  /** Under a box at rest (cl-932, cl-898): its tool and shape; while it renders, where and how long, then what it's doing with Cancel. */
  private under(show: Presentation, body: Box, w: number, colors: Palette, label: LabelFn, zoom: number) {
    let y = body.y + body.h + UNDER.gap
    if (show.tool || show.shape || show.from) {
      const shape = show.shape ? Math.min(w * 0.5, width(show.shape, SMALL)) : 0
      // Room for the X after the tool's name at the farthest zoom this line is drawn at.
      // The tool's name leaves its X the room it takes on screen at this zoom (the line isn't drawn under 45%).
      const room = w - shape - 10 - (show.tool ? (UNTOOL.gap + UNTOOL.hit) / Math.max(zoom, FAR) : 0)
      this.untool.limit = w - shape - 10
      const said = ellipsis([show.tool, show.from].filter(Boolean).join(" · "), room, SMALL)
      label("tool", said, 0, y, room, { size: SMALL, color: colors.mute })
      if (show.tool) this.untool.at = { x: width(said, SMALL), y: y + UNTOOL.middle }
      if (shape) label("shape", ellipsis(show.shape, shape, SMALL), w - shape, y, shape, { size: SMALL, color: colors.mute })
      y += UNDER.line
    }
    const progress = show.progress
    if (!progress || progress.phase === "local") return
    const time = progress.time
    const timeWidth = time ? width(time, SMALL) : 0
    label("machine", ellipsis(progress.machine, w - timeWidth - 8, SMALL), 0, y, w - timeWidth - 8, { size: SMALL, color: colors.mute })
    if (time) label("time", time, w - timeWidth, y, timeWidth, { size: SMALL, color: colors.mute })
    y += UNDER.machine
    const cancel = progress.cancel ? 70 : 0
    const count = progress.count ? width(progress.count, BODY, false, true) : 0
    const words = ellipsis(progress.state, Math.max(0, w - cancel - count - (count ? 6 : 0) - 8), BODY, false, true, true)
    // Waiting on a download, its words lead to it on its machine (cl-1205).
    const color = progress.phase === "failed" ? colors.failed : progress.leads ? colors.ember2 : colors.ink
    label("progress", words, 0, y + 4, w - cancel - count, { size: BODY, color, bold: true })
    if (progress.leads) this.area("waits", { x: 0, y, w: width(words, BODY, false, true), h: UNDER.cancel })
    if (count) label("count", progress.count!, width(words, BODY, false, true) + (words ? 6 : 0), y + 4, count, { size: BODY, color: colors.ink, bold: true })
    if (cancel) {
      const x = w - cancel
      this.ink.roundRect(x, y, cancel, UNDER.cancel, 6).fill(colors.card).stroke({ width: 1, color: colors.line })
      label("cancel", "Cancel", x + (cancel - width("Cancel", BODY)) / 2, y + 4, cancel, { size: BODY, color: colors.ink })
      this.area("cancel", { x, y, w: cancel, h: UNDER.cancel })
    }
  }

  /** Words with their mentions as chips; a mention the tool can't use, or that's gone, is struck through (Paul, 11-12). */
  private words(show: Presentation, body: Box, colors: Palette, used: Set<string>, env: CardEnv) {
    const x0 = body.x + 12, y0 = body.y + 12, room = body.w - 24
    // Over a picture, four lines; a box of words is its words, as many lines as it holds (xs-204: an LLM's answer, readable at rest).
    const over = show.picture || show.references.length
    const lines = Math.max(1, Math.floor(Math.min(body.h - 16, over ? BODY * 1.35 * 4 : Infinity) / (BODY * 1.35)))
    if (show.picture || show.references.length) this.ink.roundRect(body.x + 6, y0 - 6, body.w - 12, lines * BODY * 1.35 + 12, 5).fill({ color: colors.card, alpha: 0.86 })
    const runs: { text: string; chip: boolean; struck: boolean; x: number; row: number }[] = []
    let row = 0, cursor = 0, cut = false
    const place = (text: string, chip: boolean, struck: boolean) => {
      if (cut) return
      const size = chip ? SMALL : BODY
      let w = width(text, size, chip, chip) + (chip ? 10 : 0)
      if (cursor && cursor + w > room) { row++; cursor = 0; if (!chip) text = text.trimStart(); w = width(text, size, chip, chip) + (chip ? 10 : 0) }
      if (row >= lines) { cut = true; return }
      if (!text) return
      if (w > room) { text = ellipsis(text, room - (chip ? 10 : 0), size, chip, chip); w = room }
      runs.push({ text, chip, struck, x: cursor, row })
      cursor += w
    }
    for (const part of withMentions(show.words)) {
      if (part.mention) { const mention = part.text.slice(1, -1); place(chipWords(mention), true, show.struck?.includes(mention) ?? false); continue }
      for (const piece of part.text.split(/(\n)/)) {
        if (piece === "\n") { row++; cursor = 0; if (row >= lines) cut = true; continue }
        for (const word of piece.split(/(?<=\s)/)) place(word, false, false)
      }
    }
    if (cut && runs.length) { const last = runs[runs.length - 1]; last.text = ellipsis(last.text + "…", room - last.x - (last.chip ? 10 : 0), last.chip ? SMALL : BODY, last.chip, last.chip) }
    // Plain runs on a row join into one label; each chip is its own.
    let index = 0, plain = 0, pending: { text: string; x: number; row: number } | null = null
    const flush = () => {
      if (!pending) return
      const key = plain++ ? `words-${index++}` : (index++, "words")
      used.add(key)
      this.label(key, pending.text, x0 + pending.x, y0 + pending.row * BODY * 1.35, room - pending.x, { size: BODY, color: colors.ink }, env)
      pending = null
    }
    for (const run of runs) {
      if (!run.chip) { if (pending && pending.row === run.row) pending.text += run.text; else { flush(); pending = { text: run.text, x: run.x, row: run.row } } continue }
      flush()
      const box = { x: x0 + run.x, y: y0 + run.row * BODY * 1.35 + 1, w: width(run.text, SMALL, true, true) + 10, h: SMALL * 1.3 }
      const color = run.struck ? colors.failed : colors.ember2
      this.ink.roundRect(box.x, box.y, box.w, box.h, 4).fill({ color: run.struck ? colors.failed : colors.ember, alpha: 0.14 })
      const key = `words-chip-${index++}`
      used.add(key)
      this.label(key, run.text, box.x + 5, box.y + 1, box.w - 10, { size: SMALL, color, mono: true, bold: true }, env)
      if (run.struck) this.ink.moveTo(box.x + 4, box.y + box.h / 2).lineTo(box.x + box.w - 4, box.y + box.h / 2).stroke({ width: 1.2, color: colors.failed })
      this.drawn.chips.push({ text: run.text, struck: run.struck, box })
    }
    flush()
  }

  private entity(show: Presentation, w: number, colors: Palette, label: LabelFn, pictures: Set<string>, env: CardEnv) {
    const entity = show.entity!
    const kind = Math.min(90, width(entity.kind, SMALL))
    label("title", ellipsis(show.title, w - 36 - kind, TITLE, true, true), 12, 6, w - 36 - kind, { size: TITLE, color: colors.ink, mono: true, bold: true })
    this.drawn.full.set("title", show.title)
    this.area("title", { x: 12, y: 6, w: Math.max(16, width(this.texts.get("title")!.value, TITLE, true, true)), h: TITLE * 1.25 })
    label("kind", entity.kind, w - kind - 18, 12, kind, { size: SMALL, color: colors.mute })
    const description = wrap(entity.description, w - 24, SMALL, 2)
    label("description", description.join("\n"), 12, HEAD + 6, w - 24, { size: SMALL, color: colors.mute })
    let y = HEAD + 60
    if (entity.slots.length) { this.slots("entity", entity.slots, y, w, colors, label, pictures, env); y += 72 }
    entity.rows.forEach((row, index) => {
      this.ground.moveTo(8, y).lineTo(w - 8, y).stroke({ width: 1, color: colors.line })
      label(`sub-${index}`, ellipsis(row.name, w - 24, BODY, false, true), 12, y + 6, w - 24, { size: BODY, color: colors.ink, bold: true })
      label(`subdesc-${index}`, wrap(row.description, w - 24, SMALL, 2).join("\n"), 12, y + 30, w - 24, { size: SMALL, color: colors.mute })
      this.slots(`sub-${index}`, row.slots, y + 64, w, colors, label, pictures, env)
      y += SUB_ROW
    })
    if (show.state) this.ink.circle(w - 10, 12, 4).fill(colors[show.state])
  }

  private slots(key: string, slots: NonNullable<Presentation["entity"]>["slots"], y: number, w: number, colors: Palette, label: LabelFn, pictures: Set<string>, env: CardEnv) {
    if (!slots.length) return
    const each = (w - 24 - (slots.length - 1) * 6) / slots.length
    slots.forEach((slot, index) => {
      const box = { x: 12 + index * (each + 6), y, w: Math.max(1, each), h: 44 }
      this.ground.rect(box.x, box.y, box.w, box.h).fill(colors.well)
      if (slot.picture) this.picturePart(`${key}-slot-${index}`, slot.picture, box, false, true, pictures, env)
      else label(`${key}-empty-${index}`, ellipsis(slot.label, each - 4, SMALL), box.x + 2, y + 12, each - 4, { size: SMALL, color: colors.mute })
      label(`${key}-field-${index}`, ellipsis(slot.field, each, SMALL), box.x, y + 46, each, { size: SMALL, color: colors.mute })
    })
  }

  /** The least-cropped rows for the references a render is made from (Media.Spread's rule). */
  private references(show: Presentation, body: Box, pictures: Set<string>, colors: Palette, env: CardEnv) {
    const refs = show.references
    const aspects = refs.map((ref, index) => this.pictures.get(`ref-${index}`)?.handle?.aspect ?? ref.shape ?? 1)
    let rows = 1, best = -1
    for (let n = 1; n <= refs.length; n++) {
      const per = Math.ceil(refs.length / n)
      let kept = 0
      for (let r = 0; r < n; r++) for (const aspect of aspects.slice(r * per, (r + 1) * per)) {
        const cell = body.w / body.h * n / Math.max(1, Math.min(per, refs.length - r * per))
        kept += Math.min(aspect / cell, cell / aspect)
      }
      if (kept / refs.length > best + 1e-6) { best = kept / refs.length; rows = n }
    }
    const per = Math.ceil(refs.length / rows)
    refs.forEach((ref, index) => {
      const row = Math.floor(index / per), count = Math.min(per, refs.length - row * per)
      const cell = { x: body.x + (index % per) * body.w / count, y: body.y + row * body.h / rows, w: body.w / count, h: body.h / rows }
      if (ref.sha256) this.picturePart(`ref-${index}`, ref, cell, false, true, pictures, env)
      else this.corners(this.ground, cell, colors.mute)
    })
  }

  private picturePart(key: string, source: ScenePicture, box: Box, main: boolean, cover: boolean, used: Set<string>, env: CardEnv) {
    const request = env.request(source, box.w)
    if (!request) return
    used.add(key)
    let picture = this.pictures.get(key)
    if (!picture) {
      picture = { sprite: new Sprite({ texture: Texture.EMPTY, eventMode: "none" }), key: "", cover, box, main, local: request.local }
      picture.sprite.visible = false
      this.pixels.addChild(picture.sprite)
      this.pictures.set(key, picture)
    }
    picture.box = box; picture.cover = cover
    picture.aspect = source.kind === "video" && !cover ? source.shape : undefined
    const wanted = JSON.stringify([request.source, request.width])
    if (picture.key !== wanted) {
      picture.key = wanted
      picture.local = request.local
      picture.next?.release()
      const held = picture
      // The old picture stays until its replacement has decoded: no flash between versions or sizes.
      held.nextKey = wanted
      held.next = env.media.acquire({ source: request.source, width: request.width }, () => { this.reveal(held, env); env.invalidate() })
    }
    this.reveal(picture, env)
  }

  private reveal(picture: Picture, env: CardEnv) {
    let revealed = false
    if (picture.next?.texture || (picture.next?.failed && !picture.handle?.texture)) {
      picture.handle?.release()
      picture.handle = picture.next
      picture.shownKey = picture.nextKey
      picture.next = picture.nextKey = undefined
      revealed = Boolean(picture.handle.texture)
    }
    const texture = picture.handle?.texture
    picture.sprite.visible = Boolean(texture)
    if (!texture) return
    let shown = texture
    if (picture.cover) {
      const scale = Math.max(picture.box.w / texture.width, picture.box.h / texture.height)
      const fw = picture.box.w / scale, fh = picture.box.h / scale
      const fx = (texture.width - fw) / 2, fy = (texture.height - fh) / 2
      const crop = picture.crop
      if (!crop || crop.source !== texture.source || crop.frame.x !== fx || crop.frame.y !== fy || crop.frame.width !== fw || crop.frame.height !== fh) {
        crop?.destroy()
        picture.crop = new Texture({ source: texture.source, frame: new Rectangle(fx, fy, fw, fh) })
      }
      shown = picture.crop!
    }
    picture.sprite.texture = shown
    // A poster is decoded to square pixels by the image loader, which ignores a video's pixel shape.
    const width = picture.aspect ? shown.height * picture.aspect : shown.width
    const fit = Math.min(picture.box.w / width, picture.box.h / shown.height)
    picture.sprite.width = width * fit
    picture.sprite.height = shown.height * fit
    picture.sprite.position.set(picture.box.x + (picture.box.w - picture.sprite.width) / 2, picture.box.y + (picture.box.h - picture.sprite.height) / 2)
    if (revealed && picture.main && this.input) env.onRevealed(this.input.node.name, picture.handle?.aspect, picture.local)
  }

  private dropPicture(key: string, picture: Picture) {
    picture.crop?.destroy()
    picture.next?.release()
    picture.handle?.release()
    picture.sprite.destroy()
    this.pictures.delete(key)
  }

  private alphaBacking(box: Box | null, env: CardEnv) {
    if (!box) { this.checker?.destroy(); this.checker = undefined; return }
    if (!this.checker) { this.checker = new TilingSprite({ texture: checkerOf(env.palette), width: box.w, height: box.h }); this.pixels.addChildAt(this.checker, 0) }
    this.checker.texture = checkerOf(env.palette)
    this.checker.position.set(box.x, box.y)
    this.checker.width = box.w; this.checker.height = box.h
  }

  private syncLatent(env: CardEnv) {
    const { node } = this.input!
    const view = node.view, run = node.runContext?.run ?? view.run, kind = view.asset?.kind
    const wanted = run?.status === "in_progress" && run.machine_state !== "queued" && view.viewed === undefined && !view.glimpse && !view.asset?.pinned && (kind === "image" || kind === "video") ? run.number : undefined
    if (this.latent && this.latent.run !== wanted) { this.latent.handle.release(); this.latent.sprite.destroy(); this.latent = undefined }
    if (wanted !== undefined && !this.latent) {
      const sprite = new Sprite({ texture: Texture.EMPTY, eventMode: "none" })
      sprite.alpha = 0.6
      this.pixels.addChild(sprite)
      const latent = { run: wanted, sprite, handle: undefined as unknown as LatentHandle }
      latent.handle = env.media.latent(wanted, () => { this.fitOver(latent.sprite, latent.handle.texture); env.invalidate() })
      this.latent = latent
    }
    if (this.latent) {
      this.latent.handle.newest(run?.progress?.preview ?? 0)
      this.fitOver(this.latent.sprite, this.latent.handle.texture)
    }
  }

  /** A video's preview plays over its poster on hover; the poster stays until its first frame. */
  private syncVideo(state: CardState, env: CardEnv) {
    const { node, show } = this.input!
    // A preview plays only while the canvas holds still under the pointer: a pan stops it, its poster drawn.
    const sha = state.hover === "body" && !env.moving && show.picture?.kind === "video" && show.picture.sha256 && env.band === 0 ? hexOf(show.picture.sha256) : undefined
    if (this.video && this.video.sha !== sha) { this.video.handle.release(); this.video.sprite.destroy(); this.video = undefined }
    if (sha && !this.video && !node.dragging) {
      const sprite = new Sprite({ texture: Texture.EMPTY, eventMode: "none" })
      this.pixels.addChild(sprite)
      const video = { sha, sprite, handle: undefined as unknown as VideoHandle }
      video.handle = env.media.video(sha, () => { this.fitOver(video.sprite, video.handle.texture, this.input?.show.picture?.shape); env.invalidate() })
      this.video = video
    }
    if (this.video) this.fitOver(this.video.sprite, this.video.handle.texture, show.picture?.shape)
  }

  private fitOver(sprite: Sprite, texture: Texture | undefined, aspect?: number) {
    if (!this.input) return
    sprite.visible = Boolean(texture)
    if (!texture) return
    const body = pictureBox(this.input.node)
    body.x -= this.input.node.box.x; body.y -= this.input.node.box.y
    sprite.texture = texture
    const width = aspect ? texture.height * aspect : texture.width
    const fit = Math.min(body.w / width, body.h / texture.height)
    sprite.width = width * fit; sprite.height = texture.height * fit
    sprite.position.set(body.x + (body.w - sprite.width) / 2, body.y + (body.h - sprite.height) / 2)
  }

  /** The X beside the tool, kept its size on screen as the canvas zooms: a scale and a place, no redraw of the card. */
  pin(zoom: number) {
    const area = this.parts.get("untool")!, at = this.untool.at
    if (!at || !this.root.visible || !this.input?.node.view.asset?.tool) {
      area.visible = false
      this.drawn.parts.delete("untool")
      this.drawn.controls = this.drawn.controls.filter((one) => one.part !== "untool")
      return
    }
    const unit = 1 / Math.max(zoom * this.root.scale.x, 0.0001)
    if (this.untool.inked !== this.untool.color) {
      this.untool.inked = this.untool.color
      const half = UNTOOL.glyph / 2, glyph = this.untool.glyph
      glyph.clear().moveTo(-half, -half).lineTo(half, half).moveTo(half, -half).lineTo(-half, half).stroke({ width: 1.5, color: this.untool.color, cap: "round" })
    }
    area.visible = true
    area.scale.set(unit)
    // After the tool's name, never into the shape beside it.
    area.position.set(Math.min(at.x + (UNTOOL.gap + UNTOOL.hit / 2) * unit, this.untool.limit - (UNTOOL.hit / 2) * unit), at.y)
    const box = { x: area.x - (UNTOOL.hit / 2) * unit, y: at.y - (UNTOOL.hit / 2) * unit, w: UNTOOL.hit * unit, h: UNTOOL.hit * unit }
    this.drawn.parts.set("untool", box)
    const control = this.drawn.controls.find((one) => one.part === "untool")
    if (control) control.box = box
    else this.drawn.controls.push({ part: "untool", box })
  }

  /** What's held over a box that takes it (app.css .card.hot): an ember outline 2px off it, in a soft 6px ember halo. The selection and the
   *  hover are the scene's (Scene.rings), in screen pixels over every card. */
  private hot(node: SceneNode, body: Box, colors: Palette) {
    if (node.selected || !node.view.hot) return
    const edge = node.view.refused ? colors.failed : colors.ember
    this.ink.rect(body.x - 4, body.y - 4, body.w + 8, body.h + 8).stroke({ width: 8, color: colors.ember, alpha: 0.22, alignment: 0.5 })
    this.ink.rect(body.x - 3, body.y - 3, body.w + 6, body.h + 6).stroke({ width: 2, color: edge, alignment: 0.5 })
  }

  /** Animations a card runs while in view; returns whether it needs another frame. */
  animate(now: number, env: CardEnv): boolean {
    const input = this.input
    if (this.pulsing) { this.pulse.clear(); this.pulsing = false }
    if (!input) return false
    const { node, show } = input
    const view = node.view
    const body = pictureBox(node)
    body.x -= node.box.x; body.y -= node.box.y
    const rendering = !view.here && Boolean(view.asset?.standing === "making" || view.queued || view.asked !== undefined)
    const main = this.pictures.get("main")
    const loading = Boolean(main && !main.handle?.texture && main.next && !main.next.failed && !main.local)
    const still = env.reduced || env.software
    this.pulsing = loading || rendering
    if (loading) this.pulse.rect(body.x, body.y, body.w, body.h).fill({ color: env.palette.ink, alpha: still ? 0.04 : 0.08 * (0.5 - 0.5 * Math.cos(((now - this.began) % 2800) / 2800 * Math.PI * 2)) })
    if (rendering) {
      const color = show.state === "queued" ? env.palette.queued : env.palette.running
      const pulse = (spread: number, alpha: number) => {
        const out = spread / env.zoom
        this.pulse.roundRect(body.x - out / 2, body.y - out / 2, body.w + out, body.h + out, 2 + out / 2).stroke({ width: out, color, alpha, alignment: 0.5 })
      }
      if (still) pulse(4, 0.6)
      else for (const delay of [0, 1200]) {
        const elapsed = now - this.began - delay
        if (elapsed < 0) continue
        const phase = (elapsed % 2400) / 2400
        pulse(24 * phase, 0.75 * (1 - phase))
      }
    }
    return !still && (loading || rendering)
  }

  private label(key: string, text: string, x: number, y: number, room: number, style: LabelStyle, env: CardEnv, lines = 1): Container | null {
    let label = this.texts.get(key)
    const value = lines > 1 ? wrap(text, room, style.size, lines).join("\n") : text
    if (!value || room <= 0) { if (label) label.view.visible = false; return null }
    // Far out, where thousands of names show at once, a name is drawn from one shared glyph atlas, not a texture of its own.
    const bitmap = (key === "title" && env.band === 2) || SMALL_LABELS.has(key)
    if (label && label.bitmap !== bitmap) { label.view.destroy(); this.texts.delete(key); label = undefined }
    if (!label) {
      label = new Label(bitmap)
      this.texts.set(key, label)
      this.labels.addChild(label.view)
    }
    label.set(value, style, env.resolution)
    label.view.position.set(x, y)
    if (!this.drawn.full.has(key) || key !== "title") this.drawn.full.set(key, text)
    return label.view
  }

  private area(part: Part, box: Box | null) {
    const area = this.parts.get(part)!
    area.visible = Boolean(box)
    if (!box) return
    const rect = area.hitArea as Rectangle
    rect.x = box.x; rect.y = box.y; rect.width = box.w; rect.height = box.h
    this.drawn.parts.set(part, box)
    if (part === "delete" || part === "cancel" || part === "untool") this.drawn.controls.push({ part, box })
  }

  private corners(graphics: Graphics, box: Box, color: number, alpha = 0.65) {
    const length = Math.min(18, box.w / 4, box.h / 4)
    for (const [x, dx] of [[box.x, 1], [box.x + box.w, -1]]) for (const [y, dy] of [[box.y, 1], [box.y + box.h, -1]]) graphics.moveTo(x, y + dy * length).lineTo(x, y).lineTo(x + dx * length, y)
    graphics.stroke({ width: 1.5, color, alpha })
  }

  private dashed(graphics: Graphics, box: Box, color: number, thick: number) {
    const dash = 8 * thick, gap = 5 * thick
    const edge = (x1: number, y1: number, x2: number, y2: number) => {
      const length = Math.hypot(x2 - x1, y2 - y1)
      for (let at = 0; at < length; at += dash + gap) {
        const a = at / length, b = Math.min(1, (at + dash) / length)
        graphics.moveTo(x1 + (x2 - x1) * a, y1 + (y2 - y1) * a).lineTo(x1 + (x2 - x1) * b, y1 + (y2 - y1) * b)
      }
    }
    edge(box.x, box.y, box.x + box.w, box.y); edge(box.x + box.w, box.y, box.x + box.w, box.y + box.h)
    edge(box.x + box.w, box.y + box.h, box.x, box.y + box.h); edge(box.x, box.y + box.h, box.x, box.y)
    graphics.stroke({ width: thick, color })
  }

  /** Inspection only: label views by role. */
  labelViews(): Map<string, Container> {
    return new Map([...this.texts].filter(([, label]) => label.view.visible).map(([key, label]) => [key, label.view]))
  }

  destroy() {
    for (const [key, picture] of [...this.pictures]) this.dropPicture(key, picture)
    this.latent?.handle.release(); this.video?.handle.release()
    this.latent = undefined; this.video = undefined
    this.root.destroy({ children: true })
  }
}
type LabelFn = (key: string, text: string, x: number, y: number, room: number, style: LabelStyle, lines?: number) => Container | null

const checkers = new Map<string, Texture>()
function checkerOf(palette: Palette): Texture {
  const key = `${palette.check}:${palette.check2}`
  let texture = checkers.get(key)
  if (texture) return texture
  const canvas = document.createElement("canvas")
  canvas.width = canvas.height = 18
  const context = canvas.getContext("2d")!
  context.fillStyle = palette.check; context.fillRect(0, 0, 18, 18)
  context.fillStyle = palette.check2; context.fillRect(0, 0, 9, 9); context.fillRect(9, 9, 9, 9)
  texture = Texture.from(canvas, true)
  checkers.set(key, texture)
  return texture
}
