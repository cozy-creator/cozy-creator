import { readFileSync } from "node:fs"
import { type IncomingMessage, request, type ServerResponse } from "node:http"
import { endianness, homedir, platform } from "node:os"
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

/** An address as `/proc/net/tcp*` prints it: each 32-bit word in hex as the host holds it, then the port. As `platform::net` reads it. */
function procAddress(text: string): string | null {
  const [address, port] = text.split(":")
  if (!address || !port) return null
  const bytes = Buffer.alloc(address.length / 2)
  for (let at = 0; at < address.length; at += 8) {
    const word = Number.parseInt(address.slice(at, at + 8), 16)
    if (endianness() === "LE") bytes.writeUInt32LE(word, at / 2)
    else bytes.writeUInt32BE(word, at / 2)
  }
  const v6 = bytes.length === 16
  const mapped = v6 && bytes.subarray(0, 12).equals(Buffer.from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff]))
  const ip = !v6 || mapped ? [...bytes.subarray(v6 ? 12 : 0)].join(".") : [...Array(8).keys()].map((at) => bytes.readUInt16BE(at * 2).toString(16)).join(":")
  return `${ip}:${Number.parseInt(port, 16)}`
}

/** Whether the other end of this connection is this server's own user (cl-980): Calcifer lets in only the user it runs as, and this
 *  proxy passes requests on as that user's, so it asks first, as Calcifer does, from the kernel's socket table. Linux only for now:
 *  elsewhere it lets in no one. */
function sameUser(socket: { remoteAddress?: string; remotePort?: number; localAddress?: string; localPort?: number }): boolean {
  if (platform() !== "linux" || typeof process.getuid !== "function") return false
  const plain = (address?: string) => (address ?? "").replace(/^::ffff:/, "")
  const peer = `${plain(socket.remoteAddress)}:${socket.remotePort}`
  const local = `${plain(socket.localAddress)}:${socket.localPort}`
  for (const table of ["/proc/net/tcp", "/proc/net/tcp6"]) {
    let text = ""
    try { text = readFileSync(table, "utf8") } catch { continue }
    for (const row of text.split("\n").slice(1)) {
      const fields = row.trim().split(/\s+/)
      if (fields.length > 7 && procAddress(fields[1]) === peer && procAddress(fields[2]) === local) return Number(fields[7]) === process.getuid()
    }
  }
  return false
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

/** Passes the page's own request to Calcifer, which checks it as its own page's: this server adds no authority. */
function forward(req: IncomingMessage, res: ServerResponse) {
  if (!sameUser(req.socket)) return refuse(res, 403, "other_user", "The dev proxy answers only the user it runs as on this computer")
  const refused = foreign(req)
  if (refused) return refuse(res, 403, "origin_not_allowed", `The dev proxy serves its own page only, not ${refused}`)
  const calcifer = holder()
  if (!calcifer) return refuse(res, 503, "calcifer_down", "Calcifer isn't running on this computer. Run any cozy command to start it.")
  const split = calcifer.address.lastIndexOf(":")
  const headers: Record<string, string | string[] | undefined> = { ...req.headers, host: calcifer.address }
  if (headers.origin) headers.origin = `http://${calcifer.address}`
  delete headers.cookie
  // Calcifer reads Sec-Fetch-Site as this proxy checked it, of this server's own page (cl-980); the rest of what it said goes.
  for (const name of Object.keys(headers)) if (name.startsWith("sec-fetch-") && name !== "sec-fetch-site" && name !== "sec-fetch-mode") delete headers[name]
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

/** `vite dev` against this computer's Calcifer: its page's requests go through this proxy, as Calcifer's own page's (cl-980). */
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
      // Hot reload's WebSocket too: another user's upgrade is closed before Vite sees it.
      server.httpServer?.prependListener("upgrade", (req: IncomingMessage) => {
        if (!sameUser(req.socket)) req.socket.destroy()
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
