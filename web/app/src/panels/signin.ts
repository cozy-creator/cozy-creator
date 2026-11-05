import { createAuthClient } from "@openrails/auth-ui/client"
import type { AuthUiAppearance, AuthUiMessageBundle } from "@openrails/auth-ui"
import { ApiError, type Client } from "../api/client"

const MOUNT = "/account"

function reply(status: number, body?: unknown, wait?: number): Response {
  const headers = new Headers({ "Content-Type": "application/json" })
  if (wait !== undefined) headers.set("Retry-After", String(wait))
  return new Response(body === undefined ? null : JSON.stringify(body), { status, headers })
}

/** What Calcifer refused, in the AuthKit code auth-ui has words for. A Hub's own code is the cause where Calcifer has no name for it. */
function refusal(error: unknown, proving: boolean): Response {
  if (!(error instanceof ApiError)) throw error
  const { body, status } = error
  const said = body.cause ?? body.code
  const code = said === "code_wrong" ? "invalid_code" : said === "code_not_found" ? "code_expired" : said === "rate_limited" && proving ? "code_tries_limited" : said
  const wait = body.retry_after_s ?? undefined
  const metadata = wait === undefined ? undefined : { retry_after_seconds: wait }
  const answer = { error: { type: "", code, message: body.message, metadata } }
  return reply(said === "rate_limited" ? 429 : status || 503, answer, wait)
}

/**
 * auth-ui's client over Calcifer's `/v1/account/codes`, which `cozy login` uses too. The key stays with Calcifer and the empty token below
 * is all this client ever holds, so nothing of the Hub's reaches the browser. Not started: it has no session to restore.
 */
export function signInClient(client: Client) {
  const sent = new Map<string, string>()
  const fetch: typeof globalThis.fetch = async (input, init) => {
    const path = new URL(String(input), location.origin).pathname.slice(MOUNT.length)
    const ask = init?.body ? JSON.parse(String(init.body)) : {}
    const proving = path === "/passwordless/confirm"
    try {
      if (path === "/passwordless/start") {
        const out = await client.sendCode("sign_in", ask.identifier)
        sent.set(ask.identifier, out.id)
        if (out.retry_after_s === undefined) return reply(204)
        const metadata = { retry_after_seconds: out.retry_after_s }
        return reply(429, { error: { type: "", code: "rate_limited", message: "Too soon.", metadata } }, out.retry_after_s)
      }
      if (proving) {
        const id = sent.get(ask.identifier)
        if (!id) return reply(401, { error: { type: "", code: "code_expired", message: "That code has expired." } })
        await client.proveCode(id, ask.code)
        return reply(200, { status: "complete", token_set: { access_token: "", token_type: "bearer" } })
      }
      return reply(404, { error: { type: "", code: "not_found", message: "Not offered here." } })
    } catch (error) {
      return refusal(error, proving)
    }
  }
  return createAuthClient({ baseUrl: MOUNT, fetch, sessionHint: false })
}

/** The page's tokens, by name, so a change to them carries here. */
export const appearance: AuthUiAppearance = {
  theme: "dark",
  variables: {
    background: "var(--panel)",
    foreground: "var(--ink)",
    card: "var(--card)",
    cardForeground: "var(--ink)",
    popover: "var(--float)",
    popoverForeground: "var(--ink)",
    primary: "var(--ember)",
    primaryForeground: "var(--bg)",
    primarySolid: "var(--ember)",
    primarySolidForeground: "var(--bg)",
    secondary: "var(--card2)",
    secondaryForeground: "var(--ink)",
    muted: "var(--card2)",
    mutedForeground: "var(--mute)",
    accent: "var(--card2)",
    accentForeground: "var(--ink)",
    destructive: "var(--bad)",
    success: "var(--cur)",
    border: "var(--line)",
    input: "var(--line2)",
    ring: "var(--ember)",
    radius: "var(--round)",
    fontFamily: "var(--sans)",
  },
}

/** A Hub's codes go by email only, and the waits are what the refusal stated. */
export const messages = {
  contactSignIn: { description: "Enter your email. We'll send you a code.", useDifferent: "Use a different email" },
  fields: { emailOrPhone: "Email" },
  validation: { emailOrPhoneRequired: "Enter your email" },
  errors: {
    rate_limited: "Check your email: the code you have still works. You can ask for another code in {retry_after_seconds} s.",
    code_tries_limited: "Too many tries. You can try again in {retry_after_seconds} s.",
    invalid_code: "That code isn't right. Use the code from your latest email, or ask for another.",
    code_expired: "That code has expired. Ask for another.",
    hub_unreachable: "Couldn't reach the Hub. Check your connection and try again.",
    calcifer_unreachable: "Calcifer isn't answering. Try again in a moment.",
  },
} as AuthUiMessageBundle
