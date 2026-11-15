import type { IncomingMessage, ServerResponse } from "node:http"
import react from "@vitejs/plugin-react"
import { defineConfig, type Plugin } from "vite"

// What Calcifer sends with the page (`crates/calcifer/src/page.rs`), so `vite preview` holds the build to it.
const PAGE_HEADERS = {
  "Content-Security-Policy":
    "default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; img-src 'self' blob: data:; media-src 'self' blob:; font-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
  "Referrer-Policy": "no-referrer",
}

function refuse(res: ServerResponse, status: number, code: string, message: string) {
  res.writeHead(status, { "Content-Type": "application/json", "Cache-Control": "no-store" })
  res.end(JSON.stringify({ error: { kind: status === 404 ? "not_found" : "unavailable", code, message } }))
}

/** Vite serves the page's code only (cl-980): in development Calcifer fetches it from here and serves it as its own page
 *  (`[calcifer] development_web`, crates/calcifer/src/page/development.rs), HMR included, so a browser only ever talks to Calcifer, and
 *  this server answers no API. */
function calcifer(): Plugin {
  const route = (server: { middlewares: { use(handler: (req: IncomingMessage, res: ServerResponse, next: () => void) => void): unknown } }) => {
    server.middlewares.use((req, res, next) => {
      if (!req.url?.startsWith("/v1")) return next()
      refuse(res, 404, "development_api_in_calcifer", "Vite serves Cozy's page code only: open Cozy at Calcifer's address.")
    })
  }
  return { name: "calcifer", configureServer: route, configurePreviewServer: route }
}

export default defineConfig(({ command, isPreview }) => ({
  base: "/",
  plugins: [react(), calcifer()],
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
