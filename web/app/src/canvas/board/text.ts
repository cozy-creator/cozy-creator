import { BitmapText, CanvasTextMetrics, type Container, Text } from "pixi.js"

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
  const loaded = () => { epoch++; families = null; listener() }
  document.fonts.addEventListener("loadingdone", loaded)
  return () => document.fonts.removeEventListener("loadingdone", loaded)
}

const measurer = document.createElement("canvas").getContext("2d")!
const segmenter = new Intl.Segmenter(undefined, { granularity: "grapheme" })
const font = (size: number, mono: boolean, bold: boolean) => `${bold ? 700 : 400} ${size}px ${family(mono)}`

export function width(text: string, size: number, mono = false, bold = false): number {
  measurer.font = font(size, mono, bold)
  return Math.ceil(measurer.measureText(text).width)
}

/** One line, cut to `room` with an ellipsis at a grapheme boundary. */
export function ellipsis(text: string, room: number, size: number, mono = false, bold = false): string {
  if (room <= 0) return ""
  if (width(text, size, mono, bold) <= room) return text
  const parts = [...segmenter.segment(text)].map((part) => part.segment)
  let low = 0, high = parts.length
  while (low < high) {
    const mid = Math.ceil((low + high) / 2)
    if (width(parts.slice(0, mid).join("") + "…", size, mono, bold) <= room) low = mid
    else high = mid - 1
  }
  return low ? parts.slice(0, low).join("") + "…" : ""
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

/** A label kept for its card's life: its text and style change in place. Small labels share one dynamic bitmap font per style. */
export class Label {
  readonly view: Container
  private text = ""
  private key = ""
  constructor(readonly bitmap: boolean) {
    this.view = bitmap ? new BitmapText({ text: "", style: { fontFamily: family(false), fontSize: 12, fill: 0xffffff } }) : new Text({ text: "", style: { fontFamily: family(false), fontSize: 12 } })
    this.view.eventMode = "none"
  }

  set(text: string, style: LabelStyle, resolution: number) {
    const view = this.view as Text | BitmapText
    const key = `${style.size}:${style.color}:${style.mono}:${style.bold}:${resolution}:${epoch}`
    if (key !== this.key) {
      this.key = key
      if (this.bitmap) {
        view.style = { fontFamily: family(Boolean(style.mono)), fontSize: style.size, fontWeight: style.bold ? "700" : "400", fill: 0xffffff }
        view.tint = style.color
      } else {
        view.style = { fontFamily: family(Boolean(style.mono)), fontSize: style.size, fontWeight: style.bold ? "700" : "400", fill: style.color, lineHeight: style.size * 1.25 }
        ;(view as Text).resolution = resolution
      }
    }
    if (text !== this.text) { this.text = text; view.text = text }
    this.view.visible = Boolean(text)
  }

  get value() { return this.text }
}

/** Measured strings are private to a project: a vault lock retires Pixi's own measurement cache too. */
export function forgetMeasurements() {
  ;(CanvasTextMetrics as unknown as { _measurementCache?: { clear(): void } })._measurementCache?.clear()
}
