// The canvas's renderer: WebGL on a real GPU, else Pixi's canvas (2D) renderer. WebGL drawn in software (SwiftShader, llvmpipe) costs a
// compositor readback of 130-190 ms a frame (cl-1018); a 2D canvas on the CPU has none.
// CSP-safe shader uniforms (no eval): the WebGL renderer is made as this module loads.
import "pixi.js/unsafe-eval"
import { CanvasGraphicsPipe, CanvasRenderer, Container, ExtensionType, extensions, Graphics, type InstructionSet, type Renderer, Sprite, Text, Texture, WebGLRenderer } from "pixi.js"

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
early.catch(() => {})

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
