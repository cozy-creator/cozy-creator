import { readFileSync } from "node:fs"
import { type IncomingMessage, request, type ServerResponse } from "node:http"
import { homedir } from "node:os"
import { join } from "node:path"
import react from "@vitejs/plugin-react"
import { defineConfig, type Plugin } from "vite"

const HOLDER = join(homedir(), ".cozy", "calcifer", "calcifer.lock.holder")

// What Calcifer sends with the page (`crates/calcifer/src/page.rs`), so `vite preview` holds the build to it.
const PAGE_HEADERS = {
  "Content-Security-Policy":
    "default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; img-src 'self' blob: data:; media-src 'self' blob:; font-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
  "Referrer-Policy": "no-referrer",
}

function refuse(res: ServerResponse, status: number, code: string, message: string) {
  res.writeHead(status, { "Content-Type": "application/json" })
  res.end(JSON.stringify({ error: { kind: status === 403 ? "forbidden" : "unavailable", code, message } }))
}

function ownNames(port: number | undefined): Set<string> {
  return new Set([`localhost:${port}`, `127.0.0.1:${port}`, `[::1]:${port}`])
}

/**
 * This proxy puts the laptop's bearer on what it passes, so a page on any other site must not reach it: a cross-site POST carries a
 * foreign `Origin`, a cross-site `<img>` a `Sec-Fetch-Site`, and a rebinding page a foreign `Host`. Those go first, before the lock is read.
 */
function foreign(req: IncomingMessage): string | null {
  const own = ownNames(req.socket.localPort)
  const { host, origin } = req.headers
  if (!host || !own.has(host)) return `Host ${host}`
  if (origin && !(origin.startsWith("http://") && own.has(origin.slice("http://".length)))) return `Origin ${origin}`
  const site = req.headers["sec-fetch-site"]
  if (site && site !== "same-origin" && site !== "none") return `Sec-Fetch-Site ${site}`
  return null
}

function forward(req: IncomingMessage, res: ServerResponse) {
  const refused = foreign(req)
  if (refused) return refuse(res, 403, "origin_not_allowed", `The dev proxy serves its own page only, not ${refused}`)

  let holder: { address: string; token: string; pid: number }
  try {
    holder = JSON.parse(readFileSync(HOLDER, "utf8"))
    // A record a stopped Calcifer left: its port may be another's by now, which would refuse this one's bearer.
    process.kill(holder.pid, 0)
  } catch {
    return refuse(res, 503, "calcifer_down", "Calcifer isn't running on this computer. Run any cozy command to start it.")
  }

  const split = holder.address.lastIndexOf(":")
  const headers: Record<string, string | string[] | undefined> = {
    ...req.headers,
    host: holder.address,
    authorization: `Bearer ${holder.token}`,
  }
  if (headers.origin) headers.origin = `http://${holder.address}`
  delete headers.cookie
  for (const name of Object.keys(headers)) if (name.startsWith("sec-fetch-")) delete headers[name]

  const upstream = request(
    { host: holder.address.slice(0, split), port: holder.address.slice(split + 1), method: req.method, path: req.url, headers },
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

/** `vite dev` against this computer's real Calcifer, whose lock says where it listens and what it wants. */
function calcifer(): Plugin {
  // Returns nothing: Vite calls a function returned here as a later hook.
  const attach = (server: { middlewares: { use(handler: (req: IncomingMessage, res: ServerResponse, next: () => void) => void): unknown } }) => {
    server.middlewares.use((req, res, next) => {
      if (req.url?.startsWith("/v1/")) forward(req, res)
      else next()
    })
  }
  return { name: "calcifer", configureServer: attach, configurePreviewServer: attach }
}

// Absolute assets keep direct project URLs and their nested routes reloadable.
export default defineConfig({
  base: "/",
  plugins: [react(), calcifer()],
  // The app imports `web/player`, which stays public beside it.
  server: { fs: { allow: [".", "../player"] } },
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
})
