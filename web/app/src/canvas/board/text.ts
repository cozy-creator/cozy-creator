import { BitmapFontManager, BitmapText, CanvasTextMetrics, type Container, Text, TextStyle } from "pixi.js"

// Shared glyphs sharp on a high-density screen at 100%; the page's few small sizes keep the atlas small.
BitmapFontManager.defaultOptions.resolution = Math.max(2, Math.ceil(devicePixelRatio || 1))

/** The page's own families (tokens.css), with a font epoch so a late web font retires earlier rasters and bitmap glyphs. */
let epoch = 0
let families: Record<"mono" | "sans", string> | null = null
const family = (mono: boolean) => {
  if (!families) {
    const css = getComputedStyle(document.documentElement)
    const of = (name: string, fallback: string) => `${css.getPropertyValue(name).trim() || fallback}, "cozy-epoch-${epoch}"`
    families = { mono: of("--mono", "monospace"), sans: of("--sans", "sans-serif") }
  }
  return mono ? families.mono : families.sans
}
export const fontEpoch = () => epoch
export function onFontsLoaded(listener: () => void): () => void {
  const loaded = () => { epoch++; families = null; widths.clear(); cuts.clear(); styles.clear(); listener() }
  document.fonts.addEventListener("loadingdone", loaded)
  return () => document.fonts.removeEventListener("loadingdone", loaded)
}

const measurer = document.createElement("canvas").getContext("2d")!
const segmenter = new Intl.Segmenter(undefined, { granularity: "grapheme" })
const font = (size: number, mono: boolean, bold: boolean) => `${bold ? 700 : 400} ${size}px ${family(mono)}`

/** Each string measured once per font (cl-1018: measureText, its font set again each call, was most of an open's first frame on a Mac). */
const widths = new Map<string, number>()
/** What `ellipsis` cut each string to, per font and room. */
const cuts = new Map<string, string>()
const MEASURED_MOST = 20_000
let measuring = ""

export function width(text: string, size: number, mono = false, bold = false): number {
  const style = font(size, mono, bold), key = `${style}\u0000${text}`
  let measured = widths.get(key)
  if (measured === undefined) {
    if (measuring !== style) { measurer.font = style; measuring = style }
    measured = Math.ceil(measurer.measureText(text).width)
    if (widths.size >= MEASURED_MOST) widths.clear()
    widths.set(key, measured)
  }
  return measured
}

/** One line, cut to `room` with an ellipsis at a grapheme boundary. */
export function ellipsis(text: string, room: number, size: number, mono = false, bold = false): string {
  if (room <= 0) return ""
  if (width(text, size, mono, bold) <= room) return text
  const key = `${font(size, mono, bold)}\u0000${room}\u0000${text}`
  const held = cuts.get(key)
  if (held !== undefined) return held
  const parts = [...segmenter.segment(text)].map((part) => part.segment)
  let low = 0, high = parts.length
  while (low < high) {
    const mid = Math.ceil((low + high) / 2)
    if (width(parts.slice(0, mid).join("") + "…", size, mono, bold) <= room) low = mid
    else high = mid - 1
  }
  const cut = low ? parts.slice(0, low).join("") + "…" : ""
  if (cuts.size >= MEASURED_MOST) cuts.clear()
  cuts.set(key, cut)
  return cut
}

/** Up to `lines` wrapped lines, the last cut with an ellipsis. */
export function wrap(text: string, room: number, size: number, lines: number): string[] {
  const out: string[] = []
  for (const paragraph of text.split("\n")) {
    let line = ""
    for (const word of paragraph.split(/(\s+)/)) {
      const next = line + word
      if (line && width(next, size) > room) {
        out.push(line.trimEnd())
        line = word.trimStart()
      } else line = next
      if (out.length === lines) break
    }
    if (out.length < lines && line.trim()) out.push(line.trimEnd())
    if (out.length >= lines) break
  }
  const all = text.replace(/\s+/g, " ").trim()
  const shown = out.join(" ").replace(/\s+/g, " ").trim()
  if (out.length && shown.length < all.length) out[out.length - 1] = ellipsis(out[out.length - 1] + "…", room, size)
  return out.map((line) => ellipsis(line, room, size))
}

export interface LabelStyle { size: number; color: number; mono?: boolean; bold?: boolean }

/** One TextStyle per look, shared by every label in it (cl-1018: a style each, made twice, was ~3 ms of an open's first frame). */
const styles = new Map<string, TextStyle>()
function styleOf(bitmap: boolean, style: LabelStyle): TextStyle {
  const key = `${bitmap}:${style.size}:${style.color}:${style.mono}:${style.bold}`
  let shared = styles.get(key)
  if (!shared) {
    const look = { fontFamily: family(Boolean(style.mono)), fontSize: style.size, fontWeight: style.bold ? "700" : "400" } as const
    shared = new TextStyle(bitmap ? { ...look, fill: 0xffffff } : { ...look, fill: style.color, lineHeight: style.size * 1.25 })
    styles.set(key, shared)
  }
  return shared
}

/** A name and a small label as cards draw them, for a renderer's first frame while the page waits: the small labels' atlas gets its first
 *  page and the glyphs every picture's version and shape use (each costs ~0.5 ms to rasterize, so only those). */
export function warmLabels(): Container[] {
  return [new Text({ text: "pic", style: styleOf(false, { size: 13, color: 0, mono: true, bold: true }) }), new BitmapText({ text: "v0123456789:×", style: styleOf(true, { size: 12, color: 0 }) })]
}

/** A label kept for its card's life: its text and style change in place. Small labels share one dynamic bitmap font per style. */
export class Label {
  readonly view: Container
  private text = ""
  private key = ""
  constructor(readonly bitmap: boolean) {
    const style = styleOf(bitmap, { size: 12, color: 0 })
    this.view = bitmap ? new BitmapText({ text: "", style }) : new Text({ text: "", style })
    this.view.eventMode = "none"
  }

  set(text: string, style: LabelStyle, resolution: number) {
    const view = this.view as Text | BitmapText
    const key = `${style.size}:${style.color}:${style.mono}:${style.bold}:${resolution}:${epoch}`
    if (key !== this.key) {
      this.key = key
      view.style = styleOf(this.bitmap, style)
      if (this.bitmap) view.tint = style.color
      else (view as Text).resolution = resolution
    }
    if (text !== this.text) { this.text = text; view.text = text }
    this.view.visible = Boolean(text)
  }

  get value() { return this.text }
}

/** Measured strings are private to a project: a vault lock retires Pixi's own measurement cache too. */
export function forgetMeasurements() {
  widths.clear()
  cuts.clear()
  measuring = ""
  ;(CanvasTextMetrics as unknown as { _measurementCache?: { clear(): void } })._measurementCache?.clear()
}
