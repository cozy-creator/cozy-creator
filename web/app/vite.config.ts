import type { IncomingMessage, ServerResponse } from "node:http"
import react from "@vitejs/plugin-react"
import { defineConfig, type Plugin } from "vite"

// What Calcifer sends with the page (`crates/calcifer/src/page.rs`), so `vite preview` holds the build to it.
const PAGE_HEADERS = {
  "Content-Security-Policy":
    "default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; img-src 'self' blob: data:; media-src 'self' blob:; font-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
  "Referrer-Policy": "no-referrer",
}


/** Vite serves development code only. Project requests stay on the owned Calcifer origin. */
function noApiBridge(): Plugin {
  const attach = (server: { middlewares: { use(handler: (req: IncomingMessage, res: ServerResponse, next: () => void) => void): unknown } }) => {
    server.middlewares.use((request, response, next) => {
      if (!request.url?.startsWith("/v1")) return next()
      response.writeHead(403, { "Content-Type": "application/json", "Cache-Control": "no-store" })
      response.end(JSON.stringify({ error: { kind: "forbidden", code: "development_api_requires_calcifer", message: "Open Cozy through its owned Calcifer browser to use project data." } }))
    })
  }
  return { name: "no-calcifer-authority-bridge", configureServer: attach, configurePreviewServer: attach }
}

export default defineConfig(({ command, isPreview }) => ({
  base: "/",
  plugins: [react(), noApiBridge()],
  // Calcifer replaces this marker per HTML response. React refresh and injected CSS inherit it.
  html: command === "serve" && !isPreview ? { cspNonce: "__COZY_DEVELOPMENT_NONCE__" } : undefined,
  server: { host: "127.0.0.1", port: 5173, strictPort: true, forwardConsole: false, ws: { path: "/__cozy_hmr" }, fs: { allow: [".", "../player"] } },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    assetsDir: "assets",
    assetsInlineLimit: 0,
    cssCodeSplit: false,
    modulePreload: false,
    chunkSizeWarningLimit: 1024,
  },
  preview: { headers: PAGE_HEADERS },
}))
