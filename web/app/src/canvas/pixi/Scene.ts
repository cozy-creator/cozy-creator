import { CanvasTextMetrics, Color, Container, Graphics, Rectangle, Sprite, Text, Texture, TilingSprite } from "pixi.js"
import type { Client } from "../../api/client"
import type { CanvasStore } from "../CanvasStore"
import type { Box } from "../graph"
import { bandOf, HEAD, nameZoomOf, SUB_ROW, hexOf } from "../shown"
import { chipWords, withMentions } from "../text"
import { MediaCache, MIN_TEXTURE_WIDTH, THUMB_SOURCE_WIDTH, type MediaHandle, type MediaRequest } from "./MediaCache"
import { pictureBox, present, type EntitySlot, type Presentation, type SceneNode, type ScenePicture } from "./presentation"

export type { SceneNode } from "./presentation"
export type HitPart = "body" | "title" | "delete" | "cancel"
export interface SceneHit { name: string; part: HitPart }
export interface SceneUpdate {
  nodes: readonly SceneNode[]
  camera: { x: number; y: number; zoom: number }
  screen: { width: number; height: number; dpr: number }
  theme: "dark" | "paper"
  hiddenNames?: ReadonlySet<string>
}
interface Options {
  world: Container
  store: CanvasStore
  client: Client
  invalidate(): void
}

const TITLE = 15.5 * 1.35
const BODY = 12.5 * 1.35
const MONO = "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace"
const SANS = '"M PLUS Rounded 1c", "Trebuchet MS", system-ui, sans-serif'
const COLORS = {
  paper: { card: 0xfdf8ef, well: 0xe9dfcc, line: 0xcdbc9f, ink: 0x3f2f24, mute: 0x5f4e3f, select: 0x1f63e0, gap: 0xfffaf0, failed: 0xc4361d, queued: 0x8a8277, running: 0x2f7fd8, unseen: 0x1f9d4c, check: "#fbfbfb", check2: "#e8e8e8" },
  dark: { card: 0x17181b, well: 0x0a0a0b, line: 0x2b2d32, ink: 0xe8e6e1, mute: 0xa3a5ac, select: 0x3d8bff, gap: 0x0b0b0c, failed: 0xf0625d, queued: 0x9a9ca3, running: 0x5aa9ff, unseen: 0x4ade80, check: "#fbfbfb", check2: "#e8e8e8" },
}
type Palette = (typeof COLORS)["paper"]

interface Picture {
  sprite: Sprite
  box: Box
  wanted: string
  current?: MediaHandle
  pending?: MediaHandle
  main: boolean
  local: boolean
  content: string
  displayed?: string
  failed: boolean
  cover: boolean
  crop?: Texture
}
interface Latent {
  run: number
  revision: number
  sprite: Sprite
  controller?: AbortController
  timer?: ReturnType<typeof setTimeout>
  texture?: Texture
  bitmap?: ImageBitmap
  tag?: string
}
interface Drawn {
  node: SceneNode
  container: Container
  ground: Graphics
  pixels: Container
  ink: Graphics
  labels: Container
  pictures: Map<string, Picture>
  texts: Map<string, Text>
  fullTexts: Map<string, string>
  chips: { text: string; box: Box; missing: boolean }[]
  checker?: TilingSprite
  hits: { box: Box; part: HitPart }[]
  presentation: Presentation
  signature: string
  tools: unknown
  context: unknown
  fontEpoch: number
  latent?: Latent
  wave?: { graphics: Graphics; began: number; alpha: number; spread: number }
  loadingPulse?: { graphics: Graphics; began: number; alpha: number; shape?: string }
  landing?: { began: number; scale: number; alpha: number }
  meter?: { run: number; fraction: number; since: number }
  stateDot?: Box
  meterBox?: Box
  alive: boolean
}

const inside = (point: { x: number; y: number }, box: Box) => point.x >= box.x && point.y >= box.y && point.x <= box.x + box.w && point.y <= box.y + box.h
const intersects = (a: Box, b: Box) => a.x <= b.x + b.w && a.x + a.w >= b.x && a.y <= b.y + b.h && a.y + a.h >= b.y

/** One passive WebGL scene. Its host owns the renderer, camera transform, gestures and demand-frame scheduler. */
export class PixiScene {
  private layer = new Container({ eventMode: "none", sortableChildren: true })
  private records = new Map<string, Drawn>()
  private landed = new Set<string>()
  private cache: MediaCache
  private input?: SceneUpdate
  private visible = true
  private stopped = false
  private clock?: ReturnType<typeof setTimeout>
  private fontEpoch = 0
  private checkers = new Map<string, Texture>()
  private palette?: { theme: SceneUpdate["theme"]; colors: Palette; glow: number; alpha: number; chip: number; chipInk: number }
  private measure = document.createElement("canvas").getContext("2d")!
  private segmenter = new Intl.Segmenter(undefined, { granularity: "grapheme" })
  private reducedMotion = matchMedia("(prefers-reduced-motion: reduce)")

  constructor(private options: Options) {
    options.world.addChild(this.layer)
    this.cache = new MediaCache(options.client, options.store.projectId ?? "", options.invalidate)
    document.fonts.addEventListener("loadingdone", this.fontsChanged)
    this.reducedMotion.addEventListener("change", this.motionChanged)
  }

  private motionChanged = () => { this.advance(performance.now()); this.options.invalidate() }

  private fontsChanged = () => {
    if (this.stopped) return
    this.fontEpoch++
    if (this.input) this.update(this.input)
    this.options.invalidate()
  }

  update(input: SceneUpdate) {
    if (this.stopped) return
    this.input = input
    if (this.palette?.theme !== input.theme) {
      // CSS remains the source of the selection colors. Read once for the whole scene when its theme changes.
      const css = getComputedStyle(document.documentElement)
      const base = COLORS[input.theme]
      const select = new Color(css.getPropertyValue("--select").trim() || base.select)
      const gap = new Color(css.getPropertyValue("--select-gap").trim() || base.gap)
      const glow = new Color(css.getPropertyValue("--select-glow").trim() || base.select)
      this.palette = { theme: input.theme, colors: { ...base, select: select.toNumber(), gap: gap.toNumber() }, glow: glow.toNumber(), alpha: glow.alpha,
        chip: new Color(css.getPropertyValue("--ember").trim() || base.select).toNumber(), chipInk: new Color(css.getPropertyValue("--ember2").trim() || base.ink).toNumber() }
    }
    const zoom = Math.max(0.0001, input.camera.zoom)
    const margin = 96 / zoom
    const screen: Box = { x: -input.camera.x / zoom - margin, y: -input.camera.y / zoom - margin, w: input.screen.width / zoom + margin * 2, h: input.screen.height / zoom + margin * 2 }
    const live = new Set<string>()
    const tools = this.options.store.tools.get()
    for (const node of input.nodes) {
      if (input.hiddenNames?.has(node.name) || !intersects({ ...node.box, y: node.box.y - 40, h: node.box.h + 160 }, screen)) continue
      live.add(node.id)
      let drawn = this.records.get(node.id)
      if (!drawn) {
        const container = new Container({ eventMode: "none" })
        const ground = new Graphics(), pixels = new Container(), ink = new Graphics(), labels = new Container()
        container.addChild(ground, pixels, ink, labels)
        this.layer.addChild(container)
        drawn = { node, container, ground, pixels, ink, labels, pictures: new Map(), texts: new Map(), fullTexts: new Map(), chips: [], hits: [], presentation: present(this.options.store, node), signature: "", tools, context: node.runContext, fontEpoch: this.fontEpoch, alive: true }
        this.records.set(node.id, drawn)
      }
      const previous = drawn.node
      const density = this.resolution(node.box.w)
      const titleScale = Math.max(TITLE, BODY * nameZoomOf(zoom))
      // World position alone never rebuilds labels or reloads textures. Camera changes only touch scale-sensitive drawing buckets.
      const signature = [node.box.w, node.box.h, node.selected, input.theme, bandOf(zoom), titleScale, density, this.textResolution(), node.selected ? 2 ** Math.ceil(Math.log2(zoom)) : "", this.fontEpoch].join(":")
      const changed = previous.view !== node.view || drawn.tools !== tools || drawn.context !== node.runContext || drawn.signature !== signature || Boolean(node.view.entity)
      drawn.node = node
      drawn.container.position.set(node.box.x, node.box.y)
      drawn.container.zIndex = node.selected ? 1 : 0
      if (changed || !drawn.signature) {
        drawn.signature = signature
        drawn.tools = tools
        drawn.context = node.runContext
        drawn.presentation = present(this.options.store, node)
        this.draw(drawn)
      }
      this.latentFor(drawn)
      this.waveFor(drawn)
      this.loadingFor(drawn)
      this.landingFor(drawn)
    }
    for (const [id, drawn] of this.records) if (!live.has(id)) { this.remove(drawn); this.records.delete(id) }
    this.scheduleClock()
  }

  private resolution(width: number): number {
    const input = this.input!
    return Math.max(MIN_TEXTURE_WIDTH, Math.min(4096, 2 ** Math.ceil(Math.log2(Math.max(1, width * input.camera.zoom * input.screen.dpr)))))
  }

  private textResolution(): number {
    return Math.max(1, Math.min(4, 2 ** Math.ceil(Math.log2(Math.max(0.001, this.input!.camera.zoom * this.input!.screen.dpr)))))
  }

  private truncate(value: string, width: number, size: number, family: string, bold = false): string {
    if (width <= 0) return ""
    this.measure.font = `${bold ? "700" : "400"} ${size}px ${family}`
    if (this.measure.measureText(value).width <= width) return value
    const chars = [...this.segmenter.segment(value)].map((part) => part.segment)
    let low = 0, high = chars.length
    while (low < high) {
      const mid = Math.ceil((low + high) / 2)
      if (this.measure.measureText(chars.slice(0, mid).join("") + "…").width <= width) low = mid
      else high = mid - 1
    }
    return low ? chars.slice(0, low).join("") + "…" : ""
  }

  private wrap(value: string, width: number, size: number, lines: number): string {
    this.measure.font = `400 ${size}px ${SANS}`
    const result: string[] = []
    let line = ""
    const words = value.split(/(\s+)/)
    for (let i = 0; i < words.length; i++) {
      if (words[i].includes("\n") || (line && this.measure.measureText(line + words[i]).width > width)) {
        result.push(this.truncate(line.trim(), width, size, SANS))
        line = ""
        if (result.length >= lines - 1) {
          result.push(this.truncate(words.slice(i).join("").replace(/\s+/g, " ").trim(), width, size, SANS))
          return result.join("\n")
        }
      }
      line += words[i].replace(/\s+/g, " ")
    }
    if (line.trim()) result.push(this.truncate(line.trim(), width, size, SANS))
    return result.slice(0, lines).join("\n")
  }

  private label(drawn: Drawn, used: Set<string>, key: string, value: string, x: number, y: number, width: number, size: number, color: number, mono = false, bold = false, lines = 1) {
    if (!value || width <= 0) return
    used.add(key)
    drawn.fullTexts.set(key, value)
    const family = mono ? MONO : SANS
    // A changed fallback key also retires Pixi's text texture/metrics cache after a native font finishes loading.
    const rasterFamily = `${family}, "cozy-font-epoch-${this.fontEpoch}"`
    const text = lines > 1 ? this.wrap(value, width, size, lines) : this.truncate(value, width, size, family, bold)
    let label = drawn.texts.get(key)
    if (!label) {
      // Native Canvas text preserves shaping, fallback fonts and color emoji. This is text rasterization, never a DOM screenshot.
      label = new Text({ text, style: { fontFamily: rasterFamily, fontSize: size, fontWeight: bold ? "700" : "400", fill: color, lineHeight: size * 1.25 }, resolution: this.textResolution() })
      drawn.texts.set(key, label)
      drawn.labels.addChild(label)
    } else {
      label.text = text
      label.style = { fontFamily: rasterFamily, fontSize: size, fontWeight: bold ? "700" : "400", fill: color, lineHeight: size * 1.25 }
      label.resolution = this.textResolution()
    }
    if (drawn.fontEpoch !== this.fontEpoch) label.onViewUpdate()
    label.position.set(x, y)
    return label
  }

  private draw(drawn: Drawn) {
    const input = this.input!
    const zoom = Math.max(0.0001, input.camera.zoom)
    const far = bandOf(zoom) > 0
    const colors = this.palette!.colors
    const { node, presentation: show } = drawn
    const body = pictureBox(node)
    body.x -= node.box.x; body.y -= node.box.y
    const { ground, ink } = drawn
    ground.clear(); ink.clear()
    drawn.chips = []
    drawn.stateDot = undefined; drawn.meterBox = undefined
    drawn.hits = []
    const labels = new Set<string>()
    const pictures = new Set<string>()
    if (show.entity) {
      ground.roundRect(0, 0, node.box.w, node.box.h, 8).fill(colors.card).stroke({ width: 1, color: colors.line })
      this.drawEntity(drawn, labels, pictures, colors)
    } else {
      if (show.picture || show.note || show.error) ground.rect(body.x, body.y, body.w, body.h).fill(colors.well)
      else this.corners(ground, body, colors.mute)
      if (show.picture?.alpha) {
        if (!drawn.checker) { drawn.checker = new TilingSprite({ texture: this.checker(input.theme), width: body.w, height: body.h }); drawn.pixels.addChildAt(drawn.checker, 0) }
        drawn.checker.texture = this.checker(input.theme)
        drawn.checker.position.set(body.x, body.y)
        drawn.checker.width = body.w; drawn.checker.height = body.h
      } else if (drawn.checker) { drawn.checker.destroy(); drawn.checker = undefined }
      if (show.picture) this.picture(drawn, pictures, "main", show.picture, body, true)
      if (show.references.length) this.references(drawn, pictures, body)
      if (drawn.pictures.get("main")?.failed) this.label(drawn, labels, "unavailable", "Selected image unavailable", 12, body.y + body.h / 2 - BODY, body.w - 24, BODY, colors.mute, false, false, 2)
      const sentence = show.error ?? show.note ?? ""
      if (sentence) this.label(drawn, labels, "note", sentence, 12, body.y + Math.max(8, (body.h - BODY * 2.5) / 2), body.w - 24, BODY, show.error ? colors.failed : colors.mute, false, false, 2)
      if (show.words && (!show.picture || node.view.asset?.standing === "making")) {
        const textHeight = Math.min(body.h - 16, BODY * 1.35 * 4)
        const overPicture = show.picture || show.references.length
        const y = body.y + 12
        if (overPicture) ink.roundRect(body.x + 6, y - 6, body.w - 12, textHeight + 12, 5).fill({ color: colors.card, alpha: 0.84 })
        this.words(drawn, labels, 12, y, body.w - 24, Math.max(1, Math.floor(textHeight / (BODY * 1.35))), colors)
      }
      if (show.duration && !far) this.label(drawn, labels, "duration", show.duration, Math.max(8, body.w - 72), body.y + body.h - 28, 64, BODY, 0xffffff, true)
      if (show.progress?.phase === "local") {
        ink.rect(body.x, body.y, body.w * (show.progress.fraction ?? 0), body.h).fill({ color: colors.running, alpha: 0.2 })
        this.label(drawn, labels, "local-progress", show.progress.state, 12, body.y + body.h / 2 - BODY, body.w - 24, BODY, colors.ink, false, true)
      }
      this.drawHeader(drawn, labels, colors, far)
      if (!far) this.drawUnder(drawn, labels, colors, zoom)
    }
    drawn.hits.unshift({ box: body, part: "body" })
    if (show.progress?.fraction !== undefined && !node.view.canceling) {
      const w = body.w * this.meter(drawn), h = Math.min(body.h, 3 / zoom)
      ink.rect(body.x, body.y, w, h).fill(colors.running)
      drawn.meterBox = { x: body.x, y: body.y, w, h }
    }
    if (node.selected || node.view.hot) {
      const selectionZoom = node.selected ? 2 ** Math.ceil(Math.log2(zoom)) : zoom
      const thickness = (node.selected ? 6 : 2) / selectionZoom
      const inset = far ? Math.min(thickness, Math.min(body.w, body.h) / 4) : -6 / selectionZoom
      const box = { x: body.x + inset, y: body.y + inset, w: Math.max(1, body.w - inset * 2), h: Math.max(1, body.h - inset * 2) }
      if (node.selected) {
        // Three inexpensive feathered strokes keep the existing screen-space glow without one blur filter per selected card.
        for (const [width, alpha] of [[48, 0.08], [32, 0.18], [20, 0.32]]) ink.rect(body.x, body.y, body.w, body.h).stroke({ width: width / selectionZoom, color: this.palette!.glow, alpha: this.palette!.alpha * alpha, alignment: 0.5 })
      }
      ink.rect(box.x, box.y, box.w, box.h).stroke({ width: thickness + 2 / selectionZoom, color: colors.gap, alignment: 0.5 })
      ink.rect(box.x, box.y, box.w, box.h).stroke({ width: thickness, color: colors.select, alignment: 0.5 })
    }
    for (const [key, label] of drawn.texts) if (!labels.has(key)) { label.destroy(); drawn.texts.delete(key); drawn.fullTexts.delete(key) }
    for (const [key, picture] of drawn.pictures) if (!pictures.has(key)) { this.dropPicture(picture); drawn.pictures.delete(key) }
    drawn.fontEpoch = this.fontEpoch
  }

  private drawHeader(drawn: Drawn, labels: Set<string>, colors: Palette, far: boolean) {
    const { node, presentation: show, ink } = drawn
    const zoom = this.input!.camera.zoom
    const size = Math.max(TITLE, BODY * nameZoomOf(zoom))
    const rise = node.selected && !far ? Math.min(0, 9 - 9 / zoom) : 0
    const y = HEAD - 12 - size * 1.25 + rise
    const dot = show.state ? size * 0.42 : 0
    if (show.state) {
      ink.circle(2 + dot / 2, y + size * 0.66, dot / 2).fill(colors[show.state])
      drawn.stateDot = { x: 2, y: y + size * 0.66 - dot / 2, w: dot, h: dot }
    }
    const left = show.state ? dot + 8 : 2
    const controls = far ? 0 : 32 + (show.version ? 40 : 0)
    const title = this.label(drawn, labels, "title", show.title, left, y, node.box.w - left - controls, size, colors.ink, true, true)
    if (title) drawn.hits.push({ box: { x: left, y, w: Math.max(title.width, 16), h: size * 1.25 }, part: "title" })
    if (!far) {
      if (show.version) this.label(drawn, labels, "version", show.version, node.box.w - 72, HEAD - 12 - BODY * 1.25 + rise, 38, BODY, colors.mute, true)
      if (node.view.asset || node.view.landed || node.view.placing) {
        const x = node.box.w - 26, top = HEAD - 40 + rise
        if (node.view.placing) {
          ink.moveTo(x + 6, top + 9).lineTo(x + 18, top + 21).moveTo(x + 18, top + 9).lineTo(x + 6, top + 21).stroke({ width: 1.7, color: colors.mute })
        } else {
          ink.moveTo(x + 5, top + 10).lineTo(x + 19, top + 10).moveTo(x + 9, top + 7).lineTo(x + 15, top + 7).moveTo(x + 7, top + 12).lineTo(x + 8, top + 23).lineTo(x + 16, top + 23).lineTo(x + 17, top + 12).stroke({ width: 1.7, color: colors.mute })
        }
        drawn.hits.push({ box: { x: x - 2, y: top, w: 30, h: 30 }, part: node.view.placing ? "cancel" : "delete" })
      }
    }
  }

  /** Native text shaping with actual chip geometry; it owns no editor, HTML snapshot or additional interaction model. */
  private words(drawn: Drawn, labels: Set<string>, x: number, y: number, width: number, lines: number, colors: Palette) {
    const runs: { text: string; chip: boolean; missing: boolean; x: number; row: number; width: number }[] = []
    let row = 0, cursor = 0, cut = false
    const add = (text: string, chip: boolean, missing: boolean) => {
      const font = chip ? BODY * 0.9 : BODY
      let length = this.textWidth(text, font, chip ? MONO : SANS, chip) + (chip ? 10 : 0)
      if (cursor && cursor + length > width) { row++; cursor = 0 }
      if (row >= lines) { cut = true; return false }
      if (!cursor && !text.trim()) return true
      if (length > width) { text = this.truncate(text, width - (chip ? 10 : 0), font, chip ? MONO : SANS, chip); length = Math.min(width, this.textWidth(text, font, chip ? MONO : SANS, chip) + (chip ? 10 : 0)) }
      const prior = runs.at(-1)
      if (!chip && prior && !prior.chip && prior.row === row) { prior.text += text; prior.width += length }
      else runs.push({ text, chip, missing, x: cursor, row, width: length })
      cursor += length
      return true
    }
    for (const part of withMentions(drawn.presentation.words)) {
      if (cut) break
      if (part.mention) {
        const mention = part.text.slice(1, -1)
        const missing = Boolean(drawn.node.view.asset?.missing?.includes(mention) || this.options.store.mentionOf(mention, drawn.node.name)?.gone)
        add(chipWords(mention), true, missing)
      } else for (const token of part.text.split(/(\s+)/)) {
        if (cut) break
        if (token.includes("\n")) { row++; cursor = 0; if (row >= lines) cut = true; continue }
        if (this.textWidth(token, BODY, SANS) <= width) { add(token, false, false); continue }
        let piece = ""
        for (const { segment } of this.segmenter.segment(token)) {
          if (piece && this.textWidth(piece + segment, BODY, SANS) > width) { if (!add(piece, false, false)) break; piece = "" }
          piece += segment
        }
        if (!cut && piece) add(piece, false, false)
      }
    }
    if (cut && runs.length) {
      const last = runs.at(-1)!
      last.text = this.truncate(last.text + "…", Math.min(width - last.x, last.width + BODY), last.chip ? BODY * 0.9 : BODY, last.chip ? MONO : SANS, last.chip)
    }
    runs.forEach((run, index) => {
      const size = run.chip ? BODY * 0.9 : BODY
      const top = y + run.row * BODY * 1.35 + (run.chip ? BODY * 0.05 : 0)
      if (run.chip) {
        const box = { x: x + run.x, y: top, w: run.width, h: size * 1.25 }
        drawn.ink.roundRect(box.x, box.y, box.w, box.h, 4).fill({ color: run.missing ? colors.failed : this.palette!.chip, alpha: 0.13 })
        this.label(drawn, labels, `words-chip-${index}`, run.text, box.x + 5, top, Math.max(0, box.w - 10), size, run.missing ? colors.failed : this.palette!.chipInk, true, true)
        if (run.missing) drawn.ink.moveTo(box.x + 5, box.y + box.h / 2).lineTo(box.x + box.w - 5, box.y + box.h / 2).stroke({ width: 1, color: colors.failed })
        drawn.chips.push({ text: run.text, box, missing: run.missing })
      } else this.label(drawn, labels, index ? `words-${index}` : "words", run.text, x + run.x, top, Math.min(width - run.x, run.width + 1), size, colors.ink)
    })
  }

  private drawUnder(drawn: Drawn, labels: Set<string>, colors: Palette, zoom: number) {
    const { node, presentation: show } = drawn
    let y = node.box.h + 2 + 9 / zoom
    if (show.tool || show.shape || show.from) {
      const shapeWidth = show.shape ? Math.min(node.box.w * 0.55, this.textWidth(show.shape, BODY, SANS)) : 0
      this.label(drawn, labels, "tool", [show.tool, show.from].filter(Boolean).join(" · "), 0, y, node.box.w - shapeWidth - 10, BODY, colors.mute)
      this.label(drawn, labels, "shape", show.shape, node.box.w - shapeWidth, y, shapeWidth, BODY, colors.mute)
      y += BODY * 1.25 + 2
    }
    const progress = show.progress
    if (!progress || progress.phase === "local") return
    const timeWidth = this.textWidth(progress.time, BODY, SANS)
    this.label(drawn, labels, "machine", progress.machine, 0, y, Math.max(0, node.box.w - timeWidth), BODY, colors.mute)
    this.label(drawn, labels, "time", progress.time, Math.max(0, node.box.w - timeWidth), y, timeWidth, BODY, colors.mute)
    y += BODY * 1.35 + 2
    const cancel = progress.cancel ? 74 : 0
    const countWidth = Math.min(node.box.w - cancel, this.textWidth(progress.count ?? "", BODY, SANS, true))
    this.label(drawn, labels, "progress", progress.state, 0, y + 3, node.box.w - cancel - countWidth - 4, BODY, progress.phase === "failed" ? colors.failed : colors.ink, false, true)
    this.label(drawn, labels, "count", progress.count ?? "", node.box.w - cancel - countWidth, y + 3, countWidth, BODY, colors.ink, false, true)
    if (progress.cancel) {
      const x = node.box.w - 70
      drawn.ink.roundRect(x, y, 70, 28, 5).fill(colors.card).stroke({ width: 1, color: colors.line })
      this.label(drawn, labels, "cancel", "Cancel", x + 10, y + 3, 52, BODY, colors.ink)
      drawn.hits.push({ box: { x, y, w: 70, h: 28 }, part: "cancel" })
    }
  }

  private textWidth(text: string, size: number, family: string, bold = false) {
    this.measure.font = `${bold ? "700" : "400"} ${size}px ${family}`
    return Math.ceil(this.measure.measureText(text).width)
  }

  private drawEntity(drawn: Drawn, labels: Set<string>, pictures: Set<string>, colors: Palette) {
    const entity = drawn.presentation.entity!
    const kindWidth = Math.min(80, this.textWidth(entity.kind, 11, SANS))
    this.label(drawn, labels, "title", drawn.presentation.title, 12, 5, drawn.node.box.w - 36 - kindWidth, TITLE, colors.ink, true, true)
    this.label(drawn, labels, "kind", entity.kind, drawn.node.box.w - kindWidth - 16, 10, kindWidth, 11, colors.mute)
    this.label(drawn, labels, "description", entity.description, 12, HEAD + 4, drawn.node.box.w - 24, BODY, colors.mute, false, false, 2)
    let y = HEAD + 60
    if (entity.slots.length) { this.slots(drawn, labels, pictures, "entity", entity.slots, y, colors); y += 72 }
    entity.rows.forEach((row, index) => {
      drawn.ground.moveTo(8, y).lineTo(drawn.node.box.w - 8, y).stroke({ width: 1, color: colors.line })
      this.label(drawn, labels, `sub-${index}`, row.name, 12, y + 6, drawn.node.box.w - 24, BODY, colors.ink, true, true)
      this.label(drawn, labels, `desc-${index}`, row.description, 12, y + 30, drawn.node.box.w - 24, BODY * 0.8, colors.mute, false, false, 2)
      this.slots(drawn, labels, pictures, `sub-${index}`, row.slots, y + 64, colors)
      y += SUB_ROW
    })
    if (drawn.presentation.state) drawn.ink.circle(drawn.node.box.w - 10, 12, 4).fill(colors[drawn.presentation.state])
  }

  private slots(drawn: Drawn, labels: Set<string>, pictures: Set<string>, key: string, slots: EntitySlot[], y: number, colors: Palette) {
    if (!slots.length) return
    const width = (drawn.node.box.w - 24 - Math.max(0, slots.length - 1) * 6) / slots.length
    slots.forEach((slot, index) => {
      const x = 12 + index * (width + 6)
      const box = { x, y, w: Math.max(1, width), h: 44 }
      drawn.ground.rect(x, y, box.w, box.h).fill(colors.well)
      if (slot.picture) this.picture(drawn, pictures, `${key}-slot-${index}`, slot.picture, box, false)
      else this.label(drawn, labels, `${key}-empty-${index}`, slot.label, x + 2, y + 10, width - 4, 11, colors.mute)
      this.label(drawn, labels, `${key}-field-${index}`, slot.field, x, y + 46, width, 11, colors.mute)
    })
  }

  private corners(graphics: Graphics, box: Box, color: number) {
    const length = Math.min(18, box.w / 4, box.h / 4)
    for (const [x, dx] of [[box.x, 1], [box.x + box.w, -1]]) for (const [y, dy] of [[box.y, 1], [box.y + box.h, -1]]) graphics.moveTo(x, y + dy * length).lineTo(x, y).lineTo(x + dx * length, y)
    graphics.stroke({ width: 1.5, color, alpha: 0.65 })
  }

  /** The same least-cropped rows as Media.Spread; every reference participates. */
  private references(drawn: Drawn, used: Set<string>, box: Box) {
    const pictures = drawn.presentation.references
    const aspects = pictures.map((picture, index) => drawn.pictures.get(`reference-${index}`)?.current?.aspect ?? picture.shape ?? 1)
    let rows = 1, best = -1
    for (let n = 1; n <= pictures.length; n++) {
      const per = Math.ceil(pictures.length / n)
      let kept = 0
      for (let row = 0; row < n; row++) {
        const these = aspects.slice(row * per, (row + 1) * per)
        const cell = box.w / box.h * n / Math.max(1, these.length)
        for (const aspect of these) kept += Math.min(aspect / cell, cell / aspect)
      }
      if (kept / aspects.length > best + 1e-6) { best = kept / aspects.length; rows = n }
    }
    const per = Math.ceil(pictures.length / rows)
    pictures.forEach((picture, index) => {
      const row = Math.floor(index / per)
      const count = Math.min(per, pictures.length - row * per)
      const cell = { x: box.x + (index % per) * box.w / count, y: box.y + row * box.h / rows, w: box.w / count, h: box.h / rows }
      if (picture.sha256) this.picture(drawn, used, `reference-${index}`, picture, cell, false, true)
      else this.corners(drawn.ground, cell, COLORS[this.input!.theme].mute)
    })
  }

  private checker(theme: "paper" | "dark") {
    let texture = this.checkers.get(theme)
    if (texture) return texture
    const canvas = document.createElement("canvas")
    canvas.width = 18; canvas.height = 18
    const context = canvas.getContext("2d")!
    context.fillStyle = COLORS[theme].check; context.fillRect(0, 0, 18, 18)
    context.fillStyle = COLORS[theme].check2; context.fillRect(0, 0, 9, 9); context.fillRect(9, 9, 9, 9)
    texture = Texture.from(canvas, true)
    this.checkers.set(theme, texture)
    return texture
  }

  private request(picture: ScenePicture, width: number): MediaRequest | undefined {
    const project = this.options.store.projectId
    const size = this.resolution(width)
    if (picture.url) return { path: picture.url, local: true, width: size }
    if (!project) return undefined
    if (picture.glimpse) {
      const glimpse = picture.glimpse
      return { path: this.options.client.output({ project, run: glimpse.run }, glimpse.output, glimpse.index, glimpse.rev), width: size }
    }
    if (!picture.sha256) return undefined
    const original = size > THUMB_SOURCE_WIDTH && picture.kind === "image" && picture.path
    return { path: original ? this.options.client.file(project, picture.path!) : this.options.client.thumb(project, picture.sha256), sha256: picture.sha256, width: original ? size : Math.min(size, THUMB_SOURCE_WIDTH) }
  }

  private picture(drawn: Drawn, used: Set<string>, id: string, source: ScenePicture, box: Box, main: boolean, cover = false) {
    const request = this.request(source, box.w)
    if (!request) return
    used.add(id)
    let picture = drawn.pictures.get(id)
    if (!picture) {
      const sprite = new Sprite({ texture: Texture.EMPTY, eventMode: "none" })
      sprite.visible = false
      drawn.pixels.addChild(sprite)
      picture = { sprite, box, wanted: "", main, local: Boolean(request.local), content: "", failed: false, cover }
      drawn.pictures.set(id, picture)
    }
    const held = picture
    held.box = box
    const key = this.cache.key(request)
    if (held.wanted !== key) {
      held.pending?.release()
      held.pending = undefined
      held.wanted = key
      const hash = source.sha256 && hexOf(source.sha256)
      const preview = drawn.node.view.preview
      if (hash && held.local && preview?.sha256 === hash && held.displayed === preview.url) held.displayed = hash
      held.local = Boolean(request.local)
      held.content = hash ?? source.url ?? request.path
      held.failed = false
      if (held.current?.key !== key) held.pending = this.cache.acquire(request, () => this.showPicture(drawn, held, true))
    }
    this.showPicture(drawn, held)
  }

  private showPicture(drawn: Drawn, picture: Picture, notify = false) {
    if (!drawn.alive || this.stopped) return
    let revealed = false
    if (picture.pending?.texture) {
      const previous = picture.current
      picture.current = picture.pending
      picture.pending = undefined
      picture.displayed = picture.content
      picture.sprite.texture = picture.current.texture!
      // The ready replacement owns its bitmap before the old texture can be destroyed.
      previous?.release()
      revealed = true
    }
    const failed = Boolean(picture.pending?.failed)
    const failureChanged = failed !== picture.failed
    picture.failed = failed
    if (failed && picture.displayed !== picture.content) { picture.crop?.destroy(); picture.crop = undefined; picture.current?.release(); picture.current = undefined }
    const texture = picture.current?.texture
    picture.sprite.visible = Boolean(texture)
    if (texture) {
      let drawnTexture = texture
      if (picture.cover) {
        const scale = Math.max(picture.box.w / texture.width, picture.box.h / texture.height)
        const width = picture.box.w / scale, height = picture.box.h / scale
        const frame = new Rectangle((texture.width - width) / 2, (texture.height - height) / 2, width, height)
        if (!picture.crop || picture.crop.source !== texture.source || !picture.crop.frame.equals(frame)) {
          picture.crop?.destroy()
          picture.crop = new Texture({ source: texture.source, frame })
        }
        drawnTexture = picture.crop
      }
      picture.sprite.texture = drawnTexture
      const fit = Math.min(picture.box.w / drawnTexture.width, picture.box.h / drawnTexture.height)
      picture.sprite.width = drawnTexture.width * fit
      picture.sprite.height = drawnTexture.height * fit
      picture.sprite.position.set(picture.box.x + (picture.box.w - picture.sprite.width) / 2, picture.box.y + (picture.box.h - picture.sprite.height) / 2)
      if (revealed && picture.main) {
        // An import's own URL remains the importer's until a canonical thumbnail replaces it.
        if (!picture.local) this.options.store.showThumb(drawn.node.name)
        const view = drawn.node.view
        const measured = picture.current?.aspect
        if (measured && !view.aspect && !view.asset?.width && !view.entity && !view.asset?.timeline) this.options.store.setAspect(drawn.node.name, measured)
      }
    }
    if (notify && (failureChanged || (revealed && picture.cover))) this.draw(drawn)
    if (revealed || failureChanged) this.options.invalidate()
  }

  private dropPicture(picture: Picture) {
    picture.crop?.destroy()
    picture.pending?.release()
    picture.current?.release()
    picture.sprite.destroy()
  }

  private meter(drawn: Drawn): number {
    const run = drawn.node.runContext?.run ?? drawn.node.view.run
    if (!run) return drawn.presentation.progress?.fraction ?? 0
    const left = run.status === "completed" ? run.arriving?.eta_s : run.progress?.eta_s
    const since = Math.max(0, (Date.now() - run.created_at_ms) / 1000)
    const was = drawn.meter?.run === run.number ? drawn.meter : undefined
    const passed = was ? Math.max(0, since - was.since) : 0
    const fraction = !was ? drawn.presentation.progress?.fraction ?? 0 : left != null && left > 0 ? was.fraction + (1 - was.fraction) * passed / (passed + left) : was.fraction
    drawn.meter = { run: run.number, fraction: Math.min(0.99, fraction), since }
    return drawn.meter.fraction
  }

  /** The native implementation's keepalive, only for visible cards whose own run is actually making latent previews. */
  private latentFor(drawn: Drawn) {
    const view = drawn.node.view
    const run = drawn.node.runContext?.run ?? view.run
    const eligible = this.visible && this.onScreen(drawn) && run?.status === "in_progress" && run.machine_state !== "queued" && view.viewed === undefined && !view.glimpse && !view.asset?.pinned && ["image", "video"].includes(view.asset?.kind ?? "") && Boolean(this.options.store.projectId)
    if (!eligible || !run) { this.dropLatent(drawn); return }
    if (drawn.latent?.run !== run.number) {
      this.dropLatent(drawn)
      const sprite = new Sprite({ texture: Texture.EMPTY, eventMode: "none" })
      sprite.visible = false
      sprite.alpha = 0.6
      drawn.pixels.addChild(sprite)
      drawn.latent = { run: run.number, revision: run.progress?.preview ?? 0, sprite }
      this.askLatent(drawn)
    } else if (drawn.latent.revision !== (run.progress?.preview ?? 0)) {
      drawn.latent.revision = run.progress?.preview ?? 0
      this.askLatent(drawn)
    }
    this.fitLatent(drawn)
  }

  private waveFor(drawn: Drawn) {
    const view = drawn.node.view
    const running = !view.here && Boolean(view.asset?.standing === "making" || view.queued || view.asked !== undefined)
    if (!this.visible || !this.onScreen(drawn) || !running) {
      drawn.wave?.graphics.destroy()
      drawn.wave = undefined
      return
    }
    if (!drawn.wave) {
      const graphics = new Graphics()
      drawn.container.addChildAt(graphics, 0)
      drawn.wave = { graphics, began: performance.now(), alpha: 0, spread: 0 }
    }
  }

  private loadingFor(drawn: Drawn) {
    const main = drawn.pictures.get("main")
    // Import bytes have their own already-decoded preview, with no import-phase pulse (cl-918).
    const waiting = main?.pending && !main.current && !main.failed && !main.local
    if (!this.visible || !this.onScreen(drawn) || !waiting) {
      drawn.loadingPulse?.graphics.destroy()
      drawn.loadingPulse = undefined
    } else if (!drawn.loadingPulse) {
      const graphics = new Graphics()
      drawn.pixels.addChild(graphics)
      drawn.loadingPulse = { graphics, began: performance.now(), alpha: 0 }
    }
  }

  private landingFor(drawn: Drawn) {
    const view = drawn.node.view
    if (!this.visible || !this.onScreen(drawn)) { this.finishLanding(drawn); return }
    if (!drawn.landing && view.landing && !view.placing && !view.preview && !this.landed.has(drawn.node.id)) {
      this.landed.add(drawn.node.id)
      drawn.landing = { began: performance.now(), scale: 0.82, alpha: 0 }
    }
  }

  private finishLanding(drawn: Drawn) {
    if (!drawn.landing) return
    drawn.landing = undefined
    drawn.container.scale.set(1)
    drawn.container.alpha = 1
    drawn.container.position.set(drawn.node.box.x, drawn.node.box.y)
    const current = this.options.store.get(drawn.node.name)
    if (this.options.store.active && current === drawn.node.view && current.landing) this.options.store.patch(drawn.node.name, { landing: undefined })
  }

  /** Host-owned demand scheduling: a visible render breathes; idle, hidden and reduced-motion scenes request no animation frames. */
  advance(now: number): boolean {
    if (!this.visible || this.stopped || !this.input) return false
    let animated = false
    const zoom = Math.max(0.0001, this.input.camera.zoom)
    // CSS render-wave's cubic-bezier(0.25, 0.6, 0.3, 1), solved only for the few cards actually rendering.
    const ease = (x: number, x1 = 0.25, y1 = 0.6, x2 = 0.3, y2 = 1) => {
      let lo = 0, hi = 1
      const curve = (t: number, a: number, b: number) => 3 * (1 - t) ** 2 * t * a + 3 * (1 - t) * t * t * b + t ** 3
      for (let n = 0; n < 12; n++) { const t = (lo + hi) / 2; if (curve(t, x1, x2) < x) lo = t; else hi = t }
      return curve((lo + hi) / 2, y1, y2)
    }
    for (const drawn of this.records.values()) {
      const landing = drawn.landing
      if (landing) {
        const phase = Math.max(0, now - landing.began) / 260
        if (phase >= 1 || this.reducedMotion.matches) this.finishLanding(drawn)
        else {
          const progress = phase < 0.6 ? ease(phase / 0.6, 0.2, 0.9, 0.3, 1) : ease((phase - 0.6) / 0.4, 0.2, 0.9, 0.3, 1)
          landing.scale = phase < 0.6 ? 0.82 + 0.22 * progress : 1.04 - 0.04 * progress
          landing.alpha = phase < 0.6 ? progress : 1
          drawn.container.scale.set(landing.scale)
          drawn.container.alpha = landing.alpha
          drawn.container.position.set(drawn.node.box.x + drawn.node.box.w * (1 - landing.scale) / 2, drawn.node.box.y + drawn.node.box.h * (1 - landing.scale) / 2)
          animated = true
        }
      }
      const loading = drawn.loadingPulse
      if (loading) {
        const body = pictureBox(drawn.node)
        const phase = Math.max(0, now - loading.began) % 2800 / 1400
        loading.alpha = this.reducedMotion.matches ? 0.04 : 0.08 * ease(phase <= 1 ? phase : 2 - phase, 0.42, 0, 0.58, 1)
        const shape = `${body.w}:${body.h}:${this.palette!.colors.ink}`
        if (loading.shape !== shape) {
          loading.shape = shape
          loading.graphics.clear().rect(body.x - drawn.node.box.x, body.y - drawn.node.box.y, body.w, body.h).fill(this.palette!.colors.ink)
        }
        loading.graphics.alpha = loading.alpha
        if (!this.reducedMotion.matches) animated = true
      }
      const wave = drawn.wave
      if (!wave) continue
      const body = pictureBox(drawn.node), graphics = wave.graphics
      const color = drawn.presentation.state === "queued" ? this.palette!.colors.queued : this.palette!.colors.running
      graphics.clear()
      const pulse = (spread: number, alpha: number) => {
        if (spread <= 0 || alpha <= 0) return
        const out = spread / zoom
        graphics.roundRect(body.x - drawn.node.box.x - out / 2, body.y - drawn.node.box.y - out / 2, body.w + out, body.h + out, 2 + out / 2).stroke({ width: out, color, alpha, alignment: 0.5 })
      }
      if (this.reducedMotion.matches) {
        wave.alpha = 0.6; wave.spread = 4
        pulse(4, 0.6)
      } else {
        animated = true
        const elapsed = Math.max(0, now - wave.began)
        for (const delay of [0, 1200]) {
          if (elapsed < delay) continue
          const phase = ease(((elapsed - delay) % 2400) / 2400)
          const spread = 24 * phase, alpha = 0.75 * (1 - phase)
          if (!delay) { wave.alpha = alpha; wave.spread = spread }
          pulse(spread, alpha)
        }
      }
    }
    return animated
  }

  private askLatent(drawn: Drawn) {
    const latent = drawn.latent
    const project = this.options.store.projectId
    if (!latent || !project || !this.visible || this.stopped) return
    clearTimeout(latent.timer)
    latent.controller?.abort()
    const controller = new AbortController()
    latent.controller = controller
    void (async () => {
      let bitmap: ImageBitmap | undefined
      try {
        const response = await this.options.client.transport.fetch(this.options.client.runPreview({ project, run: latent.run }), { signal: controller.signal, headers: latent.tag ? { "If-None-Match": latent.tag } : {} })
        const tag = response.headers.get("etag")
        if (response.status !== 200 || !tag || tag === latent.tag) { void response.body?.cancel(); return }
        const blob = await response.blob()
        if (controller.signal.aborted) return
        bitmap = await createImageBitmap(blob)
        if (controller.signal.aborted || this.stopped || !this.visible || !drawn.alive || drawn.latent !== latent || !this.options.store.active) return
        const texture = Texture.from(bitmap, true)
        const previous = latent.texture, pixels = latent.bitmap
        latent.texture = texture; latent.bitmap = bitmap; latent.tag = tag
        bitmap = undefined
        latent.sprite.texture = texture
        latent.sprite.visible = true
        previous?.destroy(true); pixels?.close()
        this.fitLatent(drawn)
        this.options.invalidate()
      } catch { /* A preview is optional while its canonical result is still being made. */ }
      finally {
        bitmap?.close()
        if (drawn.alive && drawn.latent === latent && this.visible && !this.stopped && !controller.signal.aborted) latent.timer = setTimeout(() => this.askLatent(drawn), 8000)
      }
    })()
  }

  private fitLatent(drawn: Drawn) {
    const latent = drawn.latent
    if (!latent?.texture) return
    const box = pictureBox(drawn.node)
    const fit = Math.min(box.w / latent.texture.width, box.h / latent.texture.height)
    latent.sprite.width = latent.texture.width * fit
    latent.sprite.height = latent.texture.height * fit
    latent.sprite.position.set((box.w - latent.sprite.width) / 2, HEAD + (box.h - latent.sprite.height) / 2)
  }

  private dropLatent(drawn: Drawn) {
    const latent = drawn.latent
    if (!latent) return
    drawn.latent = undefined
    latent.controller?.abort()
    clearTimeout(latent.timer)
    latent.sprite.destroy()
    latent.texture?.destroy(true)
    latent.bitmap?.close()
  }

  private scheduleClock() {
    if (this.clock || !this.visible || this.stopped) return
    const ticking = (drawn: Drawn) => this.onScreen(drawn) && Boolean(drawn.presentation.progress?.time)
    if (![...this.records.values()].some(ticking)) return
    this.clock = setTimeout(() => {
      this.clock = undefined
      for (const drawn of this.records.values()) if (ticking(drawn)) {
        drawn.presentation = present(this.options.store, drawn.node)
        this.draw(drawn)
      }
      this.options.invalidate()
      this.scheduleClock()
    }, 1000)
  }

  private onScreen(drawn: Drawn): boolean {
    const { camera, screen } = this.input!
    const zoom = Math.max(0.0001, camera.zoom)
    return intersects(drawn.node.box, { x: -camera.x / zoom, y: -camera.y / zoom, w: screen.width / zoom, h: screen.height / zoom })
  }

  hitTest(point: { x: number; y: number }): SceneHit | null {
    const records = [...this.records.values()]
    for (const selected of [true, false]) for (let i = records.length - 1; i >= 0; i--) {
      const drawn = records[i]
      if (drawn.node.selected !== selected) continue
      const local = { x: (point.x - drawn.container.x) / drawn.container.scale.x, y: (point.y - drawn.container.y) / drawn.container.scale.y }
      for (let n = drawn.hits.length - 1; n >= 0; n--) if (inside(local, drawn.hits[n].box)) return { name: drawn.node.name, part: drawn.hits[n].part }
    }
    return null
  }

  setVisible(visible: boolean) {
    if (this.visible === visible || this.stopped) return
    this.visible = visible
    this.layer.visible = visible
    this.cache.setVisible(visible)
    if (!visible) {
      clearTimeout(this.clock); this.clock = undefined
      for (const drawn of this.records.values()) { this.dropLatent(drawn); this.finishLanding(drawn) }
    } else if (this.input) this.update(this.input)
    this.options.invalidate()
  }

  resourceStats() {
    let labels = 0, labelBytes = 0, loadedCards = 0, latentBytes = 0, activeLatents = 0
    for (const drawn of this.records.values()) {
      labels += drawn.texts.size
      for (const label of drawn.texts.values()) labelBytes += Math.ceil(label.width) * Math.ceil(label.height) * (label.resolution ?? 1) ** 2 * 4
      if (drawn.pictures.get("main")?.current?.texture) loadedCards++
      if (drawn.latent) activeLatents++
      if (drawn.latent?.bitmap) latentBytes += drawn.latent.bitmap.width * drawn.latent.bitmap.height * 4
    }
    return { ...this.cache.resourceStats(), totalCards: this.input?.nodes.length ?? 0, visibleCards: this.records.size, loadedCards, labels, estimatedLabelBytes: labelBytes, latentBytes, activeLatents, activeDecoders: 0 }
  }

  /** Read-only geometry of actual retained display objects. This is diagnostics, never proof that the GPU presented their pixels. */
  inspect(name: string) {
    const drawn = [...this.records.values()].find((record) => record.node.name === name)
    if (!drawn || !this.visible) return null
    const body = pictureBox(drawn.node)
    const main = drawn.pictures.get("main")
    const texture = main?.current?.texture
    // The internal cache key names transport paths. Only its width/source category can leave this renderer.
    const [path, width, local] = main?.current ? JSON.parse(main.current.key) as [string, number, boolean] : ["", 0, false]
    const displayed = main?.displayed ?? ""
    const sha = displayed.startsWith("blob:") ? drawn.node.view.preview?.sha256 : /^(sha256:)?[0-9a-f]{64}$/i.test(displayed) ? displayed : undefined
    const latent = drawn.latent
    const scale = drawn.container.scale.x
    const world = (box?: Box) => box ? { x: drawn.container.x + box.x * scale, y: drawn.container.y + box.y * scale, width: box.w * scale, height: box.h * scale } : null
    const paintedBody = world({ x: body.x - drawn.node.box.x, y: body.y - drawn.node.box.y, w: body.w, h: body.h })!
    return {
      body: paintedBody,
      labels: Object.fromEntries([...drawn.texts].map(([key, label]) => [key, {
        text: label.text, fullText: drawn.fullTexts.get(key) ?? label.text, ...world({ x: label.x, y: label.y, w: label.width, h: label.height })!,
        fontSize: Number(label.style.fontSize) * scale, fontFamily: label.style.fontFamily, weight: label.style.fontWeight,
        ...(key === "title" ? { title: drawn.presentation.title } : {}),
      }])),
      state: drawn.presentation.state, unseen: drawn.presentation.unseen,
      progress: drawn.presentation.progress ? { ...drawn.presentation.progress, fraction: drawn.meter?.fraction ?? drawn.presentation.progress.fraction } : null,
      stateDot: world(drawn.stateDot), meter: world(drawn.meterBox),
      controls: drawn.hits.filter(hit => hit.part === "delete" || hit.part === "cancel").map(hit => ({ part: hit.part, ...world(hit.box)! })),
      wave: drawn.wave ? { active: true, ...paintedBody, animation: this.reducedMotion.matches ? "none" : "render-wave", alpha: drawn.wave.alpha * drawn.container.alpha, spread: drawn.wave.spread * scale, cycleMs: 2400, reducedMotion: this.reducedMotion.matches } : null,
      loadingPulse: drawn.loadingPulse ? { active: true, alpha: drawn.loadingPulse.alpha, cycleMs: 2800, reducedMotion: this.reducedMotion.matches } : null,
      landing: drawn.landing ? { active: true, scale: drawn.landing.scale, alpha: drawn.landing.alpha, durationMs: 260 } : null,
      references: [...drawn.pictures].filter(([key]) => key.startsWith("reference-")).map(([key, picture]) => ({ key, ready: picture.sprite.visible, ...world({ x: picture.sprite.x, y: picture.sprite.y, w: picture.sprite.width, h: picture.sprite.height })!, ...(/^(sha256:)?[0-9a-f]{64}$/i.test(picture.displayed ?? "") ? { sha: picture.displayed!.replace(/^sha256:/, "") } : {}) })),
      words: drawn.presentation.words,
      chips: drawn.chips.map(chip => ({ text: chip.text, missing: chip.missing, ...world(chip.box)! })),
      media: {
        ready: Boolean(texture), failed: Boolean(main?.failed), local, width: texture?.width ?? 0, height: texture?.height ?? 0,
        requestedWidth: this.resolution(body.w), resolution: local ? "local" : path.includes("/thumbs/") ? String(width) : "original",
        ...(sha ? { sha: sha.replace(/^sha256:/, "") } : {}),
      },
      latent: latent?.texture ? { tag: latent.tag, width: latent.texture.width, opacity: latent.sprite.alpha } : null,
    }
  }

  private remove(drawn: Drawn) {
    drawn.alive = false
    this.finishLanding(drawn)
    this.dropLatent(drawn)
    for (const picture of drawn.pictures.values()) this.dropPicture(picture)
    drawn.pictures.clear()
    drawn.container.destroy({ children: true })
  }

  destroy() {
    if (this.stopped) return
    this.stopped = true
    clearTimeout(this.clock)
    document.fonts.removeEventListener("loadingdone", this.fontsChanged)
    this.reducedMotion.removeEventListener("change", this.motionChanged)
    for (const drawn of this.records.values()) this.remove(drawn)
    this.records.clear()
    this.landed.clear()
    this.cache.destroy()
    this.layer.destroy({ children: true })
    for (const texture of this.checkers.values()) texture.destroy(true)
    this.checkers.clear()
    this.input = undefined
    this.measure.canvas.width = 0
    this.measure.canvas.height = 0
    // Pixi 8.23's global LRU retains the actual last 1,000 title/prompt strings after renderer.destroy(true). Its public clearMetrics
    // only drops font measurements; retire the text measurements as well at this project's private-view boundary.
    ;(CanvasTextMetrics as unknown as { _measurementCache: { clear(): void } })._measurementCache.clear()
  }
}

export const createPixiScene = (options: Options) => new PixiScene(options)
