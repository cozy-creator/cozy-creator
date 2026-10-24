# Front door

One TLS listener (`api/server.rs`): tokio-rustls offering ALPN `h2, http/1.1`, hyper-util's auto
builder and one axum `Router` (`api/http.rs`) serving the machine API (`MACHINE-API.md`) and
`GET /v1/health` (204, or 401 if an `Authorization` header is sent). HTTP/2 connections are PINGed
every 20 s and dropped after 20 s unanswered; TCP keepalive judges HTTP/1.1 lanes the same way.
The readiness receipt is `GET /v1/status`'s `receipt`, read without a capability.
`machine_api.rs` (`NativeBackend`) implements the backend over the journal, TensorFS store and
installer; the API owns no scheduler, journal or store. Backend calls run in `spawn_blocking`; an
observer's disconnect never cancels a run.

## Identity

`MachineIdentity::machine` (`api/server.rs`): the worker id and owner keys from the machine's grant,
and the boot id and P256 leaf the machine root keeps (`machine/identity.rs`), so a restarted service
is the same machine. The readiness receipt is `{payload, hmac_sha256}`, where the HMAC covers
`cozy.pod-readiness/1\0` + payload. After binding, the service writes `api-ready.json`
(`address`, `worker_id`, `boot_id`, `cert_pem`) to the state root.
