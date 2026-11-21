import { chromium } from 'playwright';
import { writeFileSync } from 'node:fs';

const executablePath = process.env.BENCH_CHROME || '/home/fidika/cozy_v2/outputs/canvas-library-selection/browser/chrome-linux64/chrome';
const out = process.env.BENCH_HARDWARE_OUT || '/home/fidika/cozy_v2/outputs/canvas-library-selection/hardware-probes.json';
const nv = {
  __NV_PRIME_RENDER_OFFLOAD: '1',
  __GLX_VENDOR_LIBRARY_NAME: 'nvidia',
  __VK_LAYER_NV_optimus: 'NVIDIA_only',
  VK_DRIVER_FILES: '/usr/share/vulkan/icd.d/nvidia_icd.json',
};
const egl = { ...nv, __EGL_VENDOR_LIBRARY_FILENAMES: '/usr/share/glvnd/egl_vendor.d/10_nvidia.json' };
const common = ['--ignore-gpu-blocklist', '--enable-unsafe-webgpu', '--disable-software-rasterizer'];
const cases = [
  ['nvidia-egl-vulkan-draw', egl, ['--use-angle=gl-egl', '--enable-features=Vulkan']],
  ['nvidia-egl-vulkan-no-surface', egl, ['--use-angle=gl-egl', '--enable-features=Vulkan', '--disable-vulkan-surface']],
  ['nvidia-vulkan-no-surface', nv, ['--use-angle=vulkan', '--enable-features=Vulkan', '--disable-vulkan-surface']],
];
const results = [];
for (const [name, env, flags] of cases) {
  let browser;
  try {
    browser = await chromium.launch({
      headless: true, executablePath, timeout: 15000,
      ignoreDefaultArgs: ['--enable-unsafe-swiftshader'],
      args: ['--no-sandbox', ...common, ...flags], env: { ...process.env, ...env },
    });
    const page = await browser.newPage();
    await page.goto('http://127.0.0.1:5197/probe.html');
    const probe = await page.evaluate(async () => {
      const glProbe = (type) => {
        const canvas = document.createElement('canvas');
        canvas.width = canvas.height = 16;
        document.body.append(canvas);
        const messages = [];
        canvas.addEventListener('webglcontextcreationerror', event => messages.push(event.statusMessage));
        canvas.addEventListener('webglcontextlost', () => messages.push('context lost'));
        const gl = canvas.getContext(type, { failIfMajorPerformanceCaveat: true, preserveDrawingBuffer: true });
        if (!gl) return { context: false, messages };
        const initialError = gl.getError();
        gl.viewport(0, 0, 16, 16);
        gl.bindFramebuffer(gl.FRAMEBUFFER, null);
        gl.clearColor(1, 0, 0, 1);
        gl.clear(gl.COLOR_BUFFER_BIT);
        const clearError = gl.getError();
        const pixels = new Uint8Array(4);
        gl.readPixels(0, 0, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, pixels);
        const debug = gl.getExtension('WEBGL_debug_renderer_info');
        return {
          context: true,
          renderer: debug ? gl.getParameter(debug.UNMASKED_RENDERER_WEBGL) : null,
          pixel: [...pixels], error: gl.getError(),
          initialError, clearError, framebuffer: gl.checkFramebufferStatus(gl.FRAMEBUFFER),
          drawingBuffer: [gl.drawingBufferWidth, gl.drawingBufferHeight],
          contextLost: gl.isContextLost(),
          messages,
        };
      };
      const webgl2Before = glProbe('webgl2');
      const webgl1Before = glProbe('webgl');
      const adapter = await Promise.race([
        navigator.gpu?.requestAdapter({ powerPreference: 'high-performance' }),
        new Promise((_, reject) => setTimeout(() => reject(new Error('requestAdapter timeout')), 10000)),
      ]);
      let webgpu = null;
      if (adapter) {
        const device = await adapter.requestDevice();
        const canvas = document.createElement('canvas');
        const context = canvas.getContext('webgpu');
        let submitted = false;
        let error = null;
        device.pushErrorScope('validation');
        if (context) {
          context.configure({ device, format: navigator.gpu.getPreferredCanvasFormat() });
          const encoder = device.createCommandEncoder();
          const pass = encoder.beginRenderPass({ colorAttachments: [{
            view: context.getCurrentTexture().createView(),
            clearValue: { r: 0.2, g: 0.4, b: 0.6, a: 1 }, loadOp: 'clear', storeOp: 'store',
          }] });
          pass.end();
          device.queue.submit([encoder.finish()]);
          await device.queue.onSubmittedWorkDone();
          submitted = true;
        }
        error = (await device.popErrorScope())?.message || null;
        webgpu = { ...Object.fromEntries(['vendor', 'architecture', 'device', 'description', 'isFallbackAdapter'].map(k => [k, adapter.info[k]])), context: !!context, submitted, error };
        context?.unconfigure();
        device.destroy();
      }
      return { webgl2Before, webgl1Before, webgpu, webgl2After: glProbe('webgl2'), htmlInCanvas: 'drawElementImageToTexture' in (globalThis.GPUQueue?.prototype || {}) };
    });
    const session = await browser.newBrowserCDPSession();
    const info = await session.send('SystemInfo.getInfo');
    const result = { name, headless: true, env, flags: [...common, ...flags], probe, info };
    results.push(result);
    console.log(JSON.stringify({ name, probe, glRenderer: info.gpu.auxAttributes.glRenderer }));
  } catch (error) {
    results.push({ name, env, flags, error: String(error) });
    console.log(JSON.stringify({ name, error: String(error) }));
  } finally {
    await browser?.close();
    writeFileSync(out, JSON.stringify(results, null, 2));
  }
}
