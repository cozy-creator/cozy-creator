import { createAuthClient } from "@openrails/auth-ui/client"
import type { AuthUiAppearance, AuthUiMessageBundle } from "@openrails/auth-ui"
import { ApiError, type Client } from "../api/client"
import type { Theme } from "../ui/theme"

const MOUNT = "/account"

function reply(status: number, body?: unknown, wait?: number): Response {
  const headers = new Headers({ "Content-Type": "application/json" })
  if (wait !== undefined) headers.set("Retry-After", String(wait))
  return new Response(body === undefined ? null : JSON.stringify(body), { status, headers })
}

/** AuthKit's AuthResult status for a step, and the member carrying it: a Hub refusing a code with one of these and its `metadata` names a step. */
const STEPS: Record<string, string> = {
  second_factor_required: "second_factor",
  enrollment_required: "enrollment",
  reauthentication_required: "reauthentication",
  verification_required: "verification",
  account_recovery_required: "recovery",
  device_verification_required: "device_verification",
}

/** The AuthResult a refusal names as AuthKit's next step, which auth-ui then shows. */
function stepOf(error: unknown): Record<string, unknown> | undefined {
  if (!(error instanceof ApiError)) return undefined
  const { cause, metadata } = error.body
  const member = cause == null ? undefined : STEPS[cause]
  return member && metadata ? { status: cause, [member]: metadata } : undefined
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
  // The code waiting on a step AuthKit named: auth-ui's next call is that step, which Calcifer takes for it.
  let stepping: string | undefined
  const signedIn = () => reply(200, { status: "complete", token_set: { access_token: "", token_type: "bearer" } })
  const fetch: typeof globalThis.fetch = async (input, init) => {
    const path = new URL(String(input), location.origin).pathname.slice(MOUNT.length)
    const ask = init?.body ? JSON.parse(String(init.body)) : {}
    const proving = path === "/passwordless/confirm"
    const id = proving ? sent.get(ask.identifier) : init?.method === "POST" ? stepping : undefined
    try {
      if (path === "/passwordless/start") {
        stepping = undefined
        const out = await client.sendCode("sign_in", ask.identifier)
        if (out.id == null) sent.delete(ask.identifier)
        else sent.set(ask.identifier, out.id)
        // Too soon is an answer here, never a 429 on the wire: auth-ui counts the wait down on Resend.
        if (out.retry_after_s == null) return reply(204)
        const metadata = { retry_after_seconds: out.retry_after_s }
        return reply(429, { error: { type: "", code: "rate_limited", message: "Too soon.", metadata } }, out.retry_after_s)
      }
      if (proving) {
        if (!id) return reply(401, { error: { type: "", code: "code_expired", message: "That code has expired." } })
        await client.proveCode(id, ask.code)
        return signedIn()
      }
      if (id) {
        await client.stepCode(id, path.slice(1), ask)
        stepping = undefined
        return signedIn()
      }
      return reply(404, { error: { type: "", code: "not_found", message: "Not offered here." } })
    } catch (error) {
      const step = stepOf(error)
      if (!step) return refusal(error, proving)
      stepping = id
      return reply(200, step)
    }
  }
  return createAuthClient({ baseUrl: MOUNT, fetch, sessionHint: false })
}

/** The page's tokens, by name, so a change to them carries here. */
const variables: AuthUiAppearance["variables"] = {
  background: "var(--panel)",
  foreground: "var(--ink)",
  card: "var(--card)",
  cardForeground: "var(--ink)",
  popover: "var(--float)",
  popoverForeground: "var(--ink)",
  primary: "var(--ember)",
  primaryForeground: "var(--on-ember)",
  primarySolid: "var(--ember)",
  primarySolidForeground: "var(--on-ember)",
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
}

export const appearance = (theme: Theme): AuthUiAppearance => ({ theme: theme === "dark" ? "dark" : "light", variables })

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
