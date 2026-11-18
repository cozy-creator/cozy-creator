// The canvas's renderer: WebGL on a real GPU, else Pixi's canvas (2D) renderer. WebGL drawn in software (SwiftShader, llvmpipe) costs a
// compositor readback of 130-190 ms a frame (cl-1018); a 2D canvas on the CPU has none.
// CSP-safe shader uniforms (no eval): the WebGL renderer is made as this module loads.
import "pixi.js/unsafe-eval"
import { BatchTextureArray, CanvasGraphicsPipe, CanvasRenderer, Container, ExtensionType, extensions, GCManagedHash, Graphics, type InstructionSet, type Renderer, Sprite, Text, Texture, type TextureSource, WebGLRenderer } from "pixi.js"
import { warmLabels } from "./text"

/** Why this renderer: a hardware GPU; no WebGL at all; WebGL only in software (or only with a major performance caveat); WebGL refused to start. */
export type Why = "hardware" | "no-webgl" | "software-webgl" | "webgl-failed"
export interface Chosen { renderer: Renderer; backend: "webgl" | "canvas"; readonly why: Why; gpu: string }

const SOFTWARE = /swiftshader|llvmpipe|lavapipe|softpipe|swrast|software|basic render driver/i
/** Releases a Graphics from the canvas pipe's GC hash when the Graphics unloads (its destroy unloads it). */
const RELEASE = { destroy() {} }

/** Pixi 8.23's canvas graphics pipe keeps every Graphics it draws in a GC hash that lets go of one only through its `_gpuData` entry, which
 *  the canvas pipe never sets: a destroyed card's Graphics (and each bitmap label's) stayed reachable until the 60 s sweep. On a pan that
 *  builds and drops cards, that was 1.1 MB of JS heap a cycle at 1,000 boxes (e2e 43795140833, large2d). */
class CanvasGraphics extends CanvasGraphicsPipe {
  static extension = { type: [ExtensionType.CanvasPipes], name: "graphics" } as const
  addRenderable(graphics: Graphics, instructionSet: InstructionSet) {
    const data = graphics._gpuData as Record<number, unknown>
    data[this.renderer.uid] ??= RELEASE
    super.addRenderable(graphics, instructionSet)
  }
}
extensions.remove(CanvasGraphicsPipe)
extensions.add(CanvasGraphics)

/** Pixi's text system nulls a released label's entry but keeps its key, and no key comes back (each is the label's TextStyle uid): a pan
 *  that builds and drops cards grew that table without end (e2e 43803931154, 0.4 MB in ten cycles at 1,000 boxes). The GC's collection
 *  sweep drops the nulls, as it does for Pixi's own tables. */
function tidy(renderer: Renderer) {
  const { gc, canvasText } = renderer as unknown as { gc: { addCollection(context: object, collection: string, type: "hash"): void }; canvasText: object }
  gc.addCollection(canvasText, "_activeTextures", "hash")
}

// Two Pixi tables keyed by uids that never repeat, which drop an entry by nulling it and keep its key (Pixi 8.23 and its dev branch on
// 11-18). Both corrections go when Pixi deletes the keys itself (cl-1227).
/** Every batch in Pixi's module-wide pool keeps a table of the texture sources it holds, and clear() nulled each entry and kept its key, so
 *  each pooled batch gathered a key for every picture and label it ever drew: 288k keys on 1,296 batches after 100 pan-and-zoom cycles over
 *  1,000 boxes, and the heap still rising (e2e 44302102288). Pixi's clear() as it is, but deleting the keys it nulled: a new table at each
 *  clear rendered as fast but held 1.5 MB more in sparse arrays (e2e 44307308164). */
BatchTextureArray.prototype.clear = function (this: BatchTextureArray) {
  const held = this.textures as (TextureSource | null)[]
  for (let i = 0; i < this.count; i++) {
    delete this.ids[held[i]!.uid]
    held[i] = null
  }
  this.count = 0
}
/** Each pipe's GC table (Graphics, labels) kept a released entry's key until the table held 10,000 nulls: the heap swung by half a MB with
 *  where a pan stood in that count. A released entry's key goes with it. */
const managed = GCManagedHash.prototype as unknown as { remove(this: { items: Record<number, unknown> }, item: { uid: number }, ...args: unknown[]): void }
const removeManaged = managed.remove
managed.remove = function (item, ...args) {
  removeManaged.call(this, item, ...args)
  if (this.items[item.uid] === null) delete this.items[item.uid]
}

/** The GPU a context names. Firefox's RENDERER is already the device (and it warns on the debug extension); Chrome's says "WebKit WebGL". */
function gpuOf(gl: WebGLRenderingContext): string {
  const plain = String(gl.getParameter(gl.RENDERER))
  if (!/^webkit webgl$/i.test(plain)) return plain
  const info = gl.getExtension("WEBGL_debug_renderer_info")
  return info ? String(gl.getParameter(info.UNMASKED_RENDERER_WEBGL)) : plain
}

function context(canvas: HTMLCanvasElement, attributes: WebGLContextAttributes): WebGLRenderingContext | null {
  try { return (canvas.getContext("webgl2", attributes) ?? canvas.getContext("webgl", attributes)) as WebGLRenderingContext | null } catch { return null }
}

let slow: Why | undefined
/** With no context free of a major performance caveat: whether there is WebGL at all. Asked only when read (inspection), never at boot. */
function withoutFast(): Why {
  if (slow) return slow
  const gl = context(document.createElement("canvas"), {})
  gl?.getExtension("WEBGL_lose_context")?.loseContext()
  return (slow = gl ? "software-webgl" : "no-webgl")
}

async function start(width: number, height: number): Promise<Chosen> {
  const options = { width: Math.max(1, width), height: Math.max(1, height), resolution: devicePixelRatio || 1, autoDensity: true, backgroundAlpha: 0 }
  const twoD = async (why: Why | null, gpu: string): Promise<Chosen> => {
    const renderer = new CanvasRenderer()
    await renderer.init(options)
    tidy(renderer)
    return { renderer, backend: "canvas", gpu, get why() { return why ?? withoutFast() } }
  }
  // One context, the renderer's own (each costs ~145 ms on a Mac runner), asked for only where it's no major performance caveat.
  const canvas = document.createElement("canvas")
  const gl = context(canvas, { alpha: true, premultipliedAlpha: true, antialias: true, stencil: true, failIfMajorPerformanceCaveat: true })
  const gpu = gl ? gpuOf(gl) : ""
  if (gl && !SOFTWARE.test(gpu)) {
    try {
      const renderer = new WebGLRenderer()
      await renderer.init({ ...options, canvas, context: gl as WebGL2RenderingContext, antialias: true })
      tidy(renderer)
      // Its shaders compiled now, while nothing waits on it, and waited for here: left queued, the first call that waits on the GPU held
      // the page's first frame (cl-1018).
      const warm = new Container()
      warm.addChild(new Graphics().rect(0, 0, 2, 2).fill(0x808080), new Sprite(Texture.WHITE), new Text({ text: "a" }))
      renderer.render(warm)
      warm.destroy({ children: true })
      gl.getError()
      return { renderer, backend: "webgl", why: "hardware", gpu }
    } catch (error) {
      console.error("WebGL wouldn't start; drawing in 2D:", error)
      gl.getExtension("WEBGL_lose_context")?.loseContext()
      return twoD("webgl-failed", gpu)
    }
  }
  gl?.getExtension("WEBGL_lose_context")?.loseContext()
  return twoD(gl ? "software-webgl" : null, gpu)
}

/** A renderer begun as the page loads, before any project is read (cl-1018: one begun as the canvas mounts held the open's first frame
 *  60–90 ms past its state): the first canvas takes it. It holds nothing of any project until then. */
let early: Promise<Chosen> | null = start(innerWidth, innerHeight)
early.then(warm, () => {})

/** What a card draws, drawn once by the early renderer at idle, unless a canvas took it first (cl-1018): otherwise a cold page's first frame
 *  of a project runs the renderer's paths for a box, a name and a small label for the first time, and rasterizes the labels' glyphs. */
function warm(chosen: Chosen) {
  const self = early
  const idle = window.requestIdleCallback ?? ((run: () => void) => setTimeout(run, 1))
  idle(() => {
    if (early !== self) return
    const box = new Container()
    box.addChild(new Graphics().roundRect(0, 0, 8, 8, 2).fill(0x808080).stroke({ width: 1, color: 0x404040 }), new Sprite(Texture.WHITE), ...warmLabels())
    chosen.renderer.render(box)
    box.destroy({ children: true })
    chosen.renderer.render(new Container())
  })
}

/** A project left (not locked): the page keeps its one renderer and GL context for the next project's canvas. */
export function parkRenderer(chosen: Chosen) {
  chosen.renderer.canvas.remove()
  early = Promise.resolve(chosen)
}

/** The page's early renderer the first time, sized to the canvas; a new one after that, or if the early one failed. */
export async function takeRenderer(width: number, height: number): Promise<Chosen> {
  const taken = early
  early = null
  if (!taken) return start(width, height)
  const chosen = await taken.catch(() => null)
  if (!chosen) return start(width, height)
  chosen.renderer.resize(Math.max(1, width), Math.max(1, height), devicePixelRatio || 1)
  return chosen
}
