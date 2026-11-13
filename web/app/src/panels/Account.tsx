import { AuthUiProvider, ContactSignIn } from "@openrails/auth-ui"
import { AuthProvider } from "@openrails/auth-ui/react"
import { useQuery, useQueryClient } from "@tanstack/react-query"
import { useState } from "react"
import { Navigate } from "react-router-dom"
import { useConnection } from "../app/Connection"
import { useTo } from "../app/Shell"
import type { Account as Who, Code, OrgClaim } from "../api/calcifer"
import { Copyable } from "../ui/Copyable"
import { say } from "../ui/notes"
import { useSignal } from "../ui/signal"
import { ShowNsfw, useShowNsfw } from "../ui/Tags"
import { theme } from "../ui/theme"
import { Confirm, Failure, KV, Section, useAsk, useNow } from "./common"
import { DrawerFrame } from "./Drawer"
import { usd } from "./format"
import { appearance, messages, signInClient } from "./signin"

/** What's emailed goes out, and comes back once. Nothing here stays in the browser. Asked too soon with no code out, it counts
 *  the Hub's wait down, and asking again waits for it. */
function CodeEntry({ sent, onDone, onAgain }: { sent: Code; onDone: (code: string) => Promise<void>; onAgain: () => void }) {
  const ask = useAsk()
  const [code, setCode] = useState("")
  const [until] = useState(() => Date.now() + (sent.retry_after_s ?? 0) * 1000)
  const left = sent.id == null ? Math.max(0, Math.ceil((until - Date.now()) / 1000)) : 0
  useNow(left > 0)
  return (
    <form
      className="stack"
      onSubmit={(event) => {
        event.preventDefault()
        void ask.run(() => onDone(code.trim()))
      }}
    >
      <p className="muted">
        {sent.id != null ? `A code is on its way to ${sent.email}.` : left > 0 ? `The Hub sends ${sent.email} its next code in ${left} s.` : `You can ask for a code to ${sent.email} again.`}
      </p>
      <label>
        Code
        <input className="field" autoFocus autoComplete="one-time-code" spellCheck={false} value={code} onChange={(event) => setCode(event.target.value)} />
      </label>
      <div className="row end">
        <button type="button" className="tb" onClick={onAgain} disabled={left > 0}>
          {left > 0 ? `Ask again in ${left} s` : "Start over"}
        </button>
        <button type="submit" className="tb primary" disabled={ask.busy || !code.trim() || sent.id == null}>
          Go on
        </button>
      </div>
      <Failure error={ask.error} lead="That didn't work" plain />
    </form>
  )
}

/** AuthKit's own form, for signing in and signing up alike: a new email makes the account, which then needs a name. */
function SignIn({ onSignedIn, keyRefused }: { onSignedIn: () => void; keyRefused: boolean }) {
  const { client } = useConnection()
  const [auth] = useState(() => signInClient(client))
  const now = useSignal(theme)
  return (
    <>
      {keyRefused && <p className="warn">The Hub no longer accepts this computer's key: it was revoked from another computer.</p>}
      <AuthProvider client={auth} autoStart={false}>
        <AuthUiProvider appearance={appearance(now)} messages={messages}>
          <ContactSignIn hideProviders onSignedIn={onSignedIn} />
        </AuthUiProvider>
      </AuthProvider>
    </>
  )
}

function NameAccount({ onNamed }: { onNamed: () => void }) {
  const { client, queries } = useConnection()
  const naming = useAsk()
  const [name, setName] = useState("")
  return (
    <form
      className="stack"
      onSubmit={(event) => {
        event.preventDefault()
        void naming.run(async () => {
          queries.setQueryData(["account"], await client.nameAccount(name.trim()))
          onNamed()
        })
      }}
    >
      <p className="muted">You're signed in. Your account needs a name, which your packages and models go under.</p>
      <label>
        Name
        <input className="field" autoFocus spellCheck={false} value={name} onChange={(event) => setName(event.target.value)} />
      </label>
      <div className="row end">
        <button type="submit" className="tb primary" disabled={naming.busy || !name.trim()}>
          Name it
        </button>
      </div>
      <Failure error={naming.error} lead="That name won't do" plain />
    </form>
  )
}

function Orgs({ who }: { who: Who }) {
  const { client } = useConnection()
  const ask = useAsk()
  const [org, setOrg] = useState("")
  const [claim, setClaim] = useState<OrgClaim | null>(null)
  return (
    <Section title="Orgs">
      {who.orgs.length ? <p>{who.orgs.join(", ")}</p> : <p className="muted">None yet.</p>}
      <details className="sect">
        <summary>Claim one</summary>
        <form
          className="row"
          onSubmit={(event) => {
            event.preventDefault()
            void ask.run(async () => setClaim(await client.claimOrg(org.trim())))
          }}
        >
          <input className="field" aria-label="An org's name" placeholder="An org's name" spellCheck={false} value={org} onChange={(event) => setOrg(event.target.value)} />
          <button type="submit" className="tb" disabled={ask.busy || !org.trim()}>
            Claim
          </button>
        </form>
        <Failure error={ask.error} lead="Couldn't claim it" />
        {claim?.claimed && <p>{claim.org} is yours{claim.proven_by ? `, proven by ${claim.proven_by}` : ""}.</p>}
        {claim && !claim.claimed && (
          <>
            <p className="muted">Post this code in one of these places, then claim it again:</p>
            {claim.code && <Copyable text={claim.code} />}
            <ul className="rows">
              {claim.proofs.map((proof) => (
                <li className="heldrow" key={proof.handle}>
                  <span className="nm">{proof.handle}</span>
                  <small>
                    {proof.where} ({proof.checked})
                  </small>
                </li>
              ))}
            </ul>
          </>
        )}
      </details>
    </Section>
  )
}

/** Sign out here, or by an emailed code everywhere. */
function SignOut({ who, onDone }: { who: Who; onDone: () => void }) {
  const { client } = useConnection()
  const ask = useAsk()
  const [sent, setSent] = useState<Code | null>(null)
  if (sent) {
    return (
      <CodeEntry
        sent={sent}
        onAgain={() => setSent(null)}
        onDone={async (code) => {
          if (sent.id == null) return
          await client.proveCode(sent.id, code)
          onDone()
        }}
      />
    )
  }
  return (
    <div className="row">
      <Confirm
        label="Sign out"
        disabled={ask.busy}
        says={`Sign this computer out${who.email ? ` of ${who.email}` : ""}? Its key is revoked at the Hub and erased here. Rentals go on billing until they end, and it can't rent or end them until it signs in again.`}
        yes="Sign out"
        onYes={() =>
          void ask.run(async () => {
            const out = await client.signOut()
            out.notes.forEach((note) => say(note))
            onDone()
          })
        }
      />
      <Confirm
        label="Sign out everywhere"
        disabled={ask.busy}
        says="Email a code that signs every computer out of this account, this one too? Rentals go on billing until they end."
        yes="Email the code"
        onYes={() => void ask.run(async () => setSent(await client.sendCode("sign_out_everywhere")))}
      />
      <Failure error={ask.error} lead="Couldn't sign out" />
    </div>
  )
}

function Settings() {
  const [showNsfw, setShowNsfw] = useShowNsfw()
  return (
    <Section title="Settings">
      <ShowNsfw shown={showNsfw} onChange={setShowNsfw} />
      <p className="muted">Applies to model and LoRA lists throughout Creator.</p>
    </Section>
  )
}

/** Who, credit, sign in, sign out, orgs and local settings. Absent entirely on a channel connection: a link can point the page at a fake pod, so no secret is entered there. */
export function Account() {
  const { client } = useConnection()
  const to = useTo()
  const queries = useQueryClient()
  const channel = client.transport.channel
  const { data: who, error } = useQuery({
    queryKey: ["account"],
    queryFn: ({ signal }) => client.account(signal),
    enabled: !channel,
  })

  if (channel) return <Navigate replace to={to()} />

  const changed = () => {
    void queries.invalidateQueries({ queryKey: ["account"] })
    void queries.invalidateQueries({ queryKey: ["status"] })
    void queries.invalidateQueries({ queryKey: ["machines"] })
    void queries.invalidateQueries({ queryKey: ["packages"] })
    void queries.invalidateQueries({ queryKey: ["models"] })
  }

  const signedIn = who?.logged_in && who.account
  return (
    <DrawerFrame title="Account">
      <Settings />
      <Failure error={error} lead="The Hub didn't say who this is" />
      {who && !who.logged_in && <SignIn keyRefused={Boolean(who.key_refused)} onSignedIn={changed} />}
      {who?.logged_in && !who.account && <NameAccount onNamed={changed} />}
      {signedIn && (
        <>
          <KV
            rows={[
              ["Account", who.account],
              ["Email", who.email],
              ["Credit", who.billing === "unmetered" ? "Unmetered" : who.balance_usd_micros !== undefined && who.balance_usd_micros !== null ? usd(who.balance_usd_micros) : ""],
            ]}
          />
          {who.buy_credits && (
            <Section title="Buy credit">
              <Copyable text={who.buy_credits} />
            </Section>
          )}
          <Orgs who={who} />
          <SignOut who={who} onDone={changed} />
        </>
      )}
    </DrawerFrame>
  )
}
