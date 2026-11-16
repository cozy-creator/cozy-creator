// The canvas's renderer: WebGL on a real GPU, else Pixi's canvas (2D) renderer. WebGL drawn in software (SwiftShader, llvmpipe) costs a
// compositor readback of 130-190 ms a frame (cl-1018); a 2D canvas on the CPU has none.
// CSP-safe shader uniforms (no eval): the WebGL renderer is made as this module loads.
import "pixi.js/unsafe-eval"
import { autoDetectRenderer, Container, Graphics, type Renderer, Sprite, Text, Texture } from "pixi.js"

/** Why this renderer: a hardware GPU; no WebGL at all; WebGL only in software (or with a major performance caveat); WebGL refused to start. */
export type Why = "hardware" | "no-webgl" | "software-webgl" | "webgl-failed"
export interface Chosen { renderer: Renderer; backend: "webgl" | "canvas"; why: Why; gpu: string }

const SOFTWARE = /swiftshader|llvmpipe|lavapipe|softpipe|swrast|software|basic render driver/i

/** The GPU a context names. Firefox's RENDERER is already the device (and it warns on the debug extension); Chrome's says "WebKit WebGL". */
function gpuOf(gl: WebGLRenderingContext): string {
  const plain = String(gl.getParameter(gl.RENDERER))
  if (!/^webkit webgl$/i.test(plain)) return plain
  const info = gl.getExtension("WEBGL_debug_renderer_info")
  return info ? String(gl.getParameter(info.UNMASKED_RENDERER_WEBGL)) : plain
}

function context(attributes: WebGLContextAttributes): WebGLRenderingContext | null {
  try {
    const canvas = document.createElement("canvas")
    return (canvas.getContext("webgl2", attributes) ?? canvas.getContext("webgl", attributes)) as WebGLRenderingContext | null
  } catch { return null }
}

let probed: { why: Why; gpu: string } | undefined
/** What WebGL this browser has, asked once: a context that would be no major performance caveat, and the GPU it names. */
function probe(): { why: Why; gpu: string } {
  if (probed) return probed
  const fast = context({ failIfMajorPerformanceCaveat: true, stencil: true })
  const gl = fast ?? context({ stencil: true })
  const gpu = gl ? gpuOf(gl) : ""
  gl?.getExtension("WEBGL_lose_context")?.loseContext()
  probed = { why: !gl ? "no-webgl" : !fast || SOFTWARE.test(gpu) ? "software-webgl" : "hardware", gpu }
  return probed
}

async function start(width: number, height: number): Promise<Chosen> {
  const options = { width: Math.max(1, width), height: Math.max(1, height), resolution: devicePixelRatio || 1, autoDensity: true, backgroundAlpha: 0 }
  const { why, gpu } = probe()
  if (why === "hardware") {
    try {
      const renderer = await autoDetectRenderer({ ...options, antialias: true, failIfMajorPerformanceCaveat: true, preference: ["webgl"] })
      // Its shaders compiled now, while nothing waits on it, and waited for here: left queued, the first call that waits on the GPU held
      // the page's first frame (cl-1018).
      const warm = new Container()
      warm.addChild(new Graphics().rect(0, 0, 2, 2).fill(0x808080), new Sprite(Texture.WHITE), new Text({ text: "a" }))
      renderer.render(warm)
      warm.destroy({ children: true })
      const { gl } = renderer as unknown as { gl: WebGLRenderingContext }
      gl.getError()
      return { renderer, backend: "webgl", why, gpu }
    } catch (error) { console.error("WebGL wouldn't start; drawing in 2D:", error) }
  }
  const renderer = await autoDetectRenderer({ ...options, preference: ["canvas"] })
  return { renderer, backend: "canvas", why: why === "hardware" ? "webgl-failed" : why, gpu }
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
