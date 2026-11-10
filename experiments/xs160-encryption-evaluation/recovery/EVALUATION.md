# xs160 recovery evaluation

Checked 2026-11-09 America/Denver (public API receipts: November 10 UTC). This evaluates personal keybag recovery, not project authorization. Source inspection and a synthetic Rust primitive smoke are complete; no authenticator ceremony, account creation, paid call, real key recovery, or security audit was performed.

## Decision and loss model

**V1:** retain local passphrase unlock and an off-device recovery artifact; prefer optional, pre-enrolled off-device PRF credentials for convenient client-only recovery after qualification. Keep the accepted transient-code email option as an explicitly different custody mode. Optional guardian recovery is a later product feature. Prototype Sui/Seal and hosted SDKs before selecting either; none is presently qualified as Cozy's turnkey recovery service.

| Candidate | Durable recovery dependency | Who can defeat normal secrecy/release policy? | Selection |
|---|---|---|---|
| PRF | Original recoverable credential and compatible client | Credential/provider compromise; executing client | Preferred convenience prototype |
| 2-of-3 | Any two independent holdings | Any colluding pair | Optional later feature |
| zkLogin + Seal | Identity/salt continuity, chain policy and live threshold | Issuer/account impersonation; threshold collusion; policy administrator | Hosted research finalist |
| Wallet SDK service | Vendor/verifier continuity and selected recovery factors | Configuration-specific threshold/TEE authority | Conditional research finalist |

Let `U` protect the personal keybag. Each unlock method wraps `U`; these slots form **OR**, so adding a threshold slot does not strengthen standalone passphrase/code/PRF slots. Ordinary Hub login authorizes ciphertext retrieval, but cannot regenerate an independently random lost secret. OIDC supplies identity assertions, not that secret. [OIDC](https://openid.net/specs/openid-connect-core-1_0.html#IDToken)

| Remaining factors after failure | Recovery result under the proposed model |
|---|---|
| Phrase forgotten; an authorized device can unlock `U` | Enroll and verify a new phrase/code; retire old slots. |
| Laptop lost; remembered phrase + off-device encrypted keybag | Local phrase slot recovers. |
| Laptop and phrase lost; original off-device PRF credential survives | PRF slot recovers **only if that credential's PRF works on replacement client**. |
| Laptop, phrase and all independent secrets lost; ordinary Hub login survives | Cannot recover without an enrolled custodian/service route. |
| Hub unavailable | Any surviving factor also needs independently accessible ciphertext/envelopes. |
| Attacker saved an old sufficient factor plus old envelope | Rewrapping the same `U` does not revoke it; rotate affected roots/content keys for future protection. |

These are access-structure consequences, not tested product guarantees. Transient plaintext code submission to TensorHub's mail endpoint gives Hub access during processing, regardless of application retention. Mail delivery/storage adds another secret holder; this mode cannot promise Hub never sees a sufficient factor.

## Four candidates

### 1. Off-device WebAuthn PRF: preferred client-only convenience

WebAuthn `prf.eval` returns repeatable credential-bound bytes. Derive a purpose-separated wrapping key locally; send only ciphertext/public metadata and the authentication response. Registration support alone is insufficient: verify an actual subsequent assertion and unwrap. Standard credential JSON serialization can include PRF results, so explicitly exclude those outputs from server submissions. [WebAuthn PRF](https://www.w3.org/TR/webauthn-3/#prf-extension)

| Provider/path | Current primary-source evidence | Qualification still required |
|---|---|---|
| Apple Passwords/iCloud | Native `ASAuthorizationPublicKeyCredentialPRFAssertionInput`, iOS 18/macOS 15 onward; same passkey/input yields same symmetric key. | Fresh-device restore, cross-OS retrieval, native/browser input mapping. [Apple API](https://developer.apple.com/documentation/authenticationservices/asauthorizationpublickeycredentialprfassertioninput-swift.struct) |
| Google Password Manager | Google documents passkey synchronization across Android, Windows, macOS, Linux, ChromeOS, iOS. | Sync documentation does **not** prove PRF restoration/transport for every combination; test provider PIN/account recovery and QR/hybrid separately. [Google](https://developer.chrome.com/blog/passkeys-gpm-ios) |
| Spare YubiKey 5 | Yubico documents PRF and native `libfido2` HMAC-secret encryption examples. | Enroll each spare separately; losing the sole physical key loses its slot. [Yubico](https://developers.yubico.com/WebAuthn/Concepts/PRF_Extension/CTAP2_HMAC_Secret_Deep_Dive.html) |
| Windows platform/provider | Win32 assertion options explicitly accept PRF inputs. | API availability does not establish every Windows Hello/plugin credential's PRF capability or backup behavior. [Microsoft API](https://learn.microsoft.com/en-us/windows/win32/api/webauthn/ns-webauthn-webauthn_authenticator_get_assertion_options) |

Rust integration surfaces exist: Microsoft's [`windows` bindings](https://microsoft.github.io/windows-docs-rs/doc/windows/Win32/Networking/WindowsWebServices/fn.WebAuthNAuthenticatorGetAssertion.html) (MIT/Apache-2.0), [`objc2-authentication-services`](https://docs.rs/crate/objc2-authentication-services/0.3.2) (Zlib/Apache-2.0/MIT), and C [`libfido2`](https://github.com/Yubico/libfido2/blob/main/LICENSE) via FFI (BSD-2-Clause). These are bindings/components, not a qualified portable recovery client. Native versus browser PRF preprocessing/UV must produce identical bytes. Choose RP domain/native association before enrollment; localhost cannot assume TensorHub's domain identity. Trust remains in authenticators, provider restore rules, and executing client code. [WebAuthn scope](https://www.w3.org/TR/webauthn-3/#rp-id)

### 2. Optional 2-of-3: transparent control, application-owned protocol

Split a random recovery wrapping key into A=user/off-device credential, B=independent guardian/offline artifact, C=Hub. Any pair recovers: lost A → B+C; unavailable B → A+C; unavailable Hub → A+B plus independent ciphertext. A+B lost → no recovery. Hub alone cannot reconstruct; Hub+guardian can. Guardian delivery must authenticate the replacement client's recipient key independently of Hub to prevent substitution. Multiple Hub services do not supply independent custodians.

[`vsss-rs` 6.0.1](https://docs.rs/crate/vsss-rs/6.0.1/source/README.md) is a concrete MIT/Apache-2.0 Rust component: byte-oriented Shamir plus Feldman/Pedersen variants. The pinned [local smoke](threshold-smoke/src/main.rs) recovered a synthetic 32-byte fixture using all three pairs; one share and duplicate identifiers were rejected. This proves API compatibility only. Guardian enrollment, authenticated share transport, persistence, rollback, revocation and availability remain Cozy work.

[`opaque-ke`](https://github.com/meta/opaque-ke) is also useful Rust (MIT/Apache-2.0): OPAQUE's client-only export key can wrap a keybag while hiding a remembered password from the server. It **does not recover a forgotten password with no other factor**. Current upstream recommends a prerelease dependency migration; its documented 2021 audit covered old versions, not today's dependency graph. [RFC 9807 export key](https://www.rfc-editor.org/rfc/rfc9807.html#section-10.4)

### 3. Sui zkLogin + Seal: genuine hosted threshold recovery, research stretch

zkLogin binds fresh client session keys to stable OAuth-derived authority; recovering identity also requires consistent salt/audience. Seal supplies policy-authorized decryption material. Dependencies include OAuth issuer, salt service/prover, Sui/fullnodes, Move policy and upgrade authority, and enough key servers. An account-recovery path that authorizes an attacker also authorizes its fresh session; a colluding decryption threshold can bypass normal release policy. [zkLogin integration](https://docs.sui.io/sui-stack/zklogin-integration/integration-guide), [Seal design](https://docs.sui.io/sui-stack/seal/design)

**Deployment documentation conflicts:** the [overview](https://docs.sui.io/sui-stack/seal) still says committee mode is Testnet-only, but [pricing/operator documentation](https://docs.sui.io/sui-stack/seal/pricing) lists Mainnet **5-of-8**: Mysten, Natsai, NodeInfra, Ruby, Overclock, H2O, Triton One, Unconfirmed; Mysten aggregator, Enoki account/access approval and `X-API-Key`. Testnet lists 3-of-5 and explicitly disclaims persistence/availability guarantees. Listed Mainnet availability is not live decryption qualification.

Read-only [API receipts](public-api-receipts.json): Mysten and independently operated NodeInfra Testnet metadata returned 200, version 0.6.16; Ruby returned Cloudflare 403; Mainnet aggregator returned 401 without API key. No decryption tested. Current [official Rust helpers](https://github.com/MystenLabs/seal/blob/54eff6a7621b9b45905876a4ffd3e675e03a9bfc/crates/seal-sdk/src/lib.rs) provide encryption, signed requests, response verification/decryption, alongside a Rust CLI; not a complete native recovery integration. [Community Rust SDK](https://github.com/gfusee/seal-sdk-rs) offers fuller adapters at git tag 0.0.5; committee/zkLogin interoperability and audit scope remain unverified. Official and community code are Apache-2.0. Crucially, Seal explicitly excludes wallet keys and similarly sensitive data; obtain a concrete suitability answer for a root-unlocking keybag before selection. [Non-goals](https://docs.sui.io/sui-stack/seal#non-goals)

### 4. Existing wallet SDK services: real candidates, substantial fit questions

MetaMask Embedded Wallets/Web3Auth has concrete JS SDKs, OAuth/device/backup factor recovery, and client key access through `request({method:'private_key'})`; a purpose-separated derived key could wrap `U`. This does not require Cozy to send blockchain transactions, but Torus infrastructure itself uses smart-contract node discovery. Independent nodes validate tokens and encrypt returned shares to a temporary client public key; provider impersonation and threshold collusion remain relevant. MFA needs a surviving backup after device loss. [Architecture](https://docs.metamask.io/embedded-wallets/infrastructure/sss-architecture/), [node protocol](https://docs.metamask.io/embedded-wallets/infrastructure/nodes-and-dkg/), [API](https://docs.metamask.io/embedded-wallets/connect-blockchain/other/sui/)

Current `@web3auth/modal` is 11.4.3; [source license](https://github.com/Web3Auth/web3auth-web/blob/c82cb8c2c81c1277c4042421deff88022d6ab920/LICENSE) restricts use, including a ≤1,000-MAU category; larger commercial use requires agreement. Dashboard client ID, verifier continuity and [paid feature tiers](https://docs.metamask.io/embedded-wallets/sdk/js/advanced/mfa/) matter. No Rust SDK or current independent operator roster was verified. Current [Privy architecture](https://docs.privy.io/security/wallet-infrastructure/architecture) instead reconstructs inside AWS Nitro TEEs using 2-of-2 enclave/auth shares, with vendor-controlled deployment approvals; older client-side 2-of-3 descriptions are stale. Its [React SDK metadata](https://registry.npmjs.org/@privy-io%2freact-auth/3.51.3) declares Apache-2.0, but managed-service access/terms and enclave policy governance remain dependencies. Neither is yet verified here as a turnkey independently operated, client-only keybag recovery service.

## Required preselection prototypes

1. PRF: signed native client + downloaded keybag, original laptop absent; test Apple/GPM restore, spare key, Windows/macOS/Linux, browser/native parity, UV/PIN, unsupported PRF and secret-free serialization.
2. Guardian: authenticated fresh-recipient ceremony, every two-factor loss case, Hub outage, malicious share, old-envelope replay and root rotation.
3. Hosted finalist: synthetic keybag recovered after device deletion; provider-account takeover policy, operator/custodian independence, continuity/export, policy updates, billing/license terms and current audit scope. For Seal resolve contradictory availability docs, sensitive-key suitability and Rust/committee interoperability first.
