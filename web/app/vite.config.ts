import { readFileSync } from "node:fs"
import { type IncomingMessage, request, type ServerResponse } from "node:http"
import { homedir } from "node:os"
import { join } from "node:path"
import react from "@vitejs/plugin-react"
import { defineConfig, type Plugin } from "vite"

const COZY = join(homedir(), ".cozy")
const HOLDER = join(COZY, "calcifer", "calcifer.lock.holder")

// What Calcifer sends with the page (`crates/calcifer/src/page.rs`), so `vite preview` holds the build to it.
const PAGE_HEADERS = {
  "Content-Security-Policy":
    "default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; img-src 'self' blob: data:; media-src 'self' blob:; font-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
  "Referrer-Policy": "no-referrer",
}

function refuse(res: ServerResponse, status: number, code: string, message: string) {
  res.writeHead(status, { "Content-Type": "application/json", "Cache-Control": "no-store" })
  res.end(JSON.stringify({ error: { kind: status === 403 ? "forbidden" : "unavailable", code, message } }))
}

/** `[calcifer] development_web`: Calcifer serves this code on its own origin, and this server answers no API. */
function servedByCalcifer(): boolean {
  let section = ""
  try {
    for (const line of readFileSync(join(COZY, "config.toml"), "utf8").split("\n")) {
      const text = line.trim()
      if (text.startsWith("[")) section = text.replace(/\s|#.*/g, "")
      else if (section === "[calcifer]" && /^development_web\s*=/.test(text)) return true
    }
  } catch {
    // No config: nothing serves this code but this server.
  }
  return false
}

function holder(): { address: string; token: string } | null {
  try {
    const held = JSON.parse(readFileSync(HOLDER, "utf8"))
    // A record a stopped Calcifer left: its port may be another's by now.
    process.kill(held.pid, 0)
    return held
  } catch {
    return null
  }
}

function ownNames(port: number | undefined): Set<string> {
  return new Set([`localhost:${port}`, `127.0.0.1:${port}`, `[::1]:${port}`])
}

/** A page on any other site must not reach Calcifer through this server: a foreign `Host`, `Origin` or `Sec-Fetch-Site` goes first. */
function foreign(req: IncomingMessage): string | null {
  const own = ownNames(req.socket.localPort)
  const { host, origin } = req.headers
  if (!host || !own.has(host)) return `Host ${host}`
  if (origin && !(origin.startsWith("http://") && own.has(origin.slice("http://".length)))) return `Origin ${origin}`
  const site = req.headers["sec-fetch-site"]
  if (site && site !== "same-origin" && site !== "none") return `Sec-Fetch-Site ${site}`
  return null
}

/** Passes the page's own request to Calcifer: its session (or `?media=`) is its only authority; this server adds none. */
function forward(req: IncomingMessage, res: ServerResponse) {
  const refused = foreign(req)
  if (refused) return refuse(res, 403, "origin_not_allowed", `The dev proxy serves its own page only, not ${refused}`)
  const calcifer = holder()
  if (!calcifer) return refuse(res, 503, "calcifer_down", "Calcifer isn't running on this computer. Run any cozy command to start it.")
  const split = calcifer.address.lastIndexOf(":")
  const headers: Record<string, string | string[] | undefined> = { ...req.headers, host: calcifer.address }
  if (headers.origin) headers.origin = `http://${calcifer.address}`
  delete headers.cookie
  for (const name of Object.keys(headers)) if (name.startsWith("sec-fetch-")) delete headers[name]
  const upstream = request(
    { host: calcifer.address.slice(0, split), port: calcifer.address.slice(split + 1), method: req.method, path: req.url, headers },
    (answer) => {
      res.writeHead(answer.statusCode ?? 502, answer.headers)
      answer.pipe(res)
      // Cut off, as a stopped Calcifer's stream is: the page hears it end, as it would from Calcifer itself.
      answer.on("close", () => {
        if (!answer.complete) res.destroy()
      })
    },
  )
  upstream.on("error", () => {
    if (res.headersSent) res.destroy()
    else refuse(res, 503, "calcifer_down", "Calcifer didn't answer the dev proxy.")
  })
  res.on("close", () => upstream.destroy())
  req.pipe(upstream)
}

/** A one-use sign-in link to this server, as `cozy open` gives one for Calcifer's own origin. */
async function signInLink(port: number): Promise<string> {
  const calcifer = holder()
  if (!calcifer) return "Calcifer isn't running: run any cozy command, then restart this server for a sign-in link"
  const answer = await fetch(`http://${calcifer.address}/v1/session-codes`, { method: "POST", headers: { Authorization: `Bearer ${calcifer.token}`, "Content-Type": "application/json" }, body: "{}" })
  if (!answer.ok) return `no sign-in link: Calcifer answered ${answer.status}`
  const { code } = await answer.json()
  return `sign in (15 min, once): http://localhost:${port}/#code=${encodeURIComponent(code)}`
}

/** `vite dev` against this computer's Calcifer, for any browser signed in by a link. */
function calcifer(): Plugin {
  const route = (server: { middlewares: { use(handler: (req: IncomingMessage, res: ServerResponse, next: () => void) => void): unknown } }) => {
    server.middlewares.use((req, res, next) => {
      if (!req.url?.startsWith("/v1")) return next()
      if (servedByCalcifer()) return refuse(res, 403, "development_api_requires_calcifer", "Calcifer serves this page: open it on Calcifer's origin.")
      forward(req, res)
    })
  }
  return {
    name: "calcifer",
    configureServer(server) {
      route(server)
      if (servedByCalcifer()) return
      server.httpServer?.once("listening", () => {
        void signInLink(server.config.server.port ?? 5173).then((said) => server.config.logger.info(`  ➜  ${said}`), () => {})
      })
    },
    configurePreviewServer: route,
  }
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
