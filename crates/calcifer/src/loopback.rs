//! Who may talk to Calcifer on this computer (design/calcifer-access.md, cl-985). There are no sessions: each request is judged
//! alone. Every one needs a loopback peer and a `Host` naming the listener it reached; an `Origin`, if sent, must be that Host's,
//! and a WebSocket must send one. `/v1` (media and streams too) and the dev page also need the peer's OS account to be Calcifer's.
//! `/v1` then needs a browser's word that the request is Calcifer's own page's (`Sec-Fetch-Site: same-origin`) or, from anything that
//! isn't a browser, the bearer in the 0600 lock record. A URL typed or opened by another program (`none`) opens the page's own files,
//! never `/v1`: an agent's browser told to go there reads nothing. The page's own files need only the Host: they hold no secret,
//! and go out with `frame-ancestors 'none'`.
use std::{
    net::SocketAddr,
    sync::{Arc, atomic::{AtomicBool, AtomicU64, Ordering}},
    time::{Duration, Instant, SystemTime},
};

use axum::{
    extract::connect_info::Connected,
    http::{HeaderMap, Method, header},
    serve::IncomingStream,
};
use api::error::{Error, Kind};
use platform::net::User;
use tokio::{net::TcpListener, sync::OnceCell};

use crate::api::{Api, Refusal, bearer, refuse};

/// Who's at the other end of a connection: its address, the listener it reached, and its OS account, asked of the kernel when a
/// request first needs it and kept for the connection's life.
#[derive(Clone, Debug)]
pub(crate) struct Peer {
    pub(crate) addr: SocketAddr,
    pub(crate) local: Option<SocketAddr>,
    /// When it was accepted: no process started after it made it (Windows names a connection by its maker's pid).
    accepted: SystemTime,
    user: Arc<OnceCell<(Result<User, &'static str>, Duration)>>,
}

impl Connected<IncomingStream<'_, TcpListener>> for Peer {
    fn connect_info(stream: IncomingStream<'_, TcpListener>) -> Self {
        Peer { addr: *stream.remote_addr(), local: stream.io().local_addr().ok(), accepted: SystemTime::now(), user: Arc::default() }
    }
}

impl Connected<IncomingStream<'_, Gate>> for Peer {
    fn connect_info(stream: IncomingStream<'_, Gate>) -> Self {
        Peer { addr: *stream.remote_addr(), local: stream.io().local_addr().ok(), accepted: SystemTime::now(), user: Arc::default() }
    }
}

/// The parent's listener as a generation accepts from it (cl-1113). Before it hands over it stops accepting: a connect then waits in
/// the backlog for the next generation, never taken here and closed unread as a server's graceful shutdown closes a connection with
/// no request begun.
pub(crate) struct Gate {
    listener: TcpListener,
    accepting: tokio::sync::watch::Receiver<bool>,
}

impl Gate {
    pub(crate) fn new(listener: TcpListener, accepting: tokio::sync::watch::Receiver<bool>) -> Self {
        Self { listener, accepting }
    }
}

impl axum::serve::Listener for Gate {
    type Io = tokio::net::TcpStream;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            if !*self.accepting.borrow_and_update() {
                // Handed over: what's left is answered, nothing more taken.
                std::future::pending::<()>().await;
            }
            tokio::select! {
                accepted = self.listener.accept() => match accepted {
                    Ok(accepted) => return accepted,
                    // As axum's own: out of descriptors, or a connection that ended before it was taken.
                    Err(e) => {
                        tracing::warn!("accepting a connection: {e}");
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                },
                _ = self.accepting.changed() => {}
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.listener.local_addr()
    }
}

impl Peer {
    async fn user(&self) -> Result<&User, &'static str> {
        let (peer, local, accepted) = (self.addr, self.local, self.accepted);
        let found = self.user.get_or_init(|| async move {
            let began = Instant::now();
            (lookup(peer, local, accepted).await, began.elapsed())
        });
        found.await.0.as_ref().map_err(|why| *why)
    }
}

async fn lookup(peer: SocketAddr, local: Option<SocketAddr>, accepted: SystemTime) -> Result<User, &'static str> {
    let local = local.ok_or("no_listener")?;
    // The cheap answer first, which waits for no turn: another account filling the turns can't hold back the owner's browser.
    let quick = tokio::task::spawn_blocking(move || platform::net::tcp_peer_user_quick(peer, local, accepted)).await.map_err(|_| "lookup_failed")?;
    if let Some(found) = quick {
        return found;
    }
    // A few at a time: macOS's full lookup reads every socket of this account's processes, Windows' every connection, Linux's
    // /proc fallback (netlink closed) every socket.
    let _turn = LOOKUPS.acquire().await.map_err(|_| "lookup_failed")?;
    tokio::task::spawn_blocking(move || platform::net::tcp_peer_user(peer, local, accepted)).await.map_err(|_| "lookup_failed")?
}

/// Tests only (`[calcifer] diagnostics`): each answer says what its checks took, as `Server-Timing` (guard, admit, peer: the
/// connection's one lookup).
pub(crate) static TIMING: AtomicBool = AtomicBool::new(false);

/// What `admit` took, for the guard to say.
#[derive(Clone, Default)]
pub(crate) struct Timing(Arc<AtomicU64>);

impl Timing {
    pub(crate) fn admitted(&self, took: Duration) {
        self.0.store(u64::try_from(took.as_nanos()).unwrap_or(u64::MAX), Ordering::Relaxed);
    }

    /// `guard;dur=…, admit;dur=…, peer;dur=…`, in milliseconds.
    pub(crate) fn header(&self, guard: Duration, peer: &Peer) -> Option<axum::http::HeaderValue> {
        let ms = |nanos: u128| nanos as f64 / 1e6;
        let mut said = format!("guard;dur={:.4}", ms(guard.as_nanos()));
        let admit = self.0.load(Ordering::Relaxed);
        if admit > 0 {
            said.push_str(&format!(", admit;dur={:.4}", ms(u128::from(admit))));
        }
        if let Some((_, took)) = peer.user.get() {
            said.push_str(&format!(", peer;dur={:.4}", ms(took.as_nanos())));
        }
        axum::http::HeaderValue::from_str(&said).ok()
    }
}

/// How many connections' peers are looked up at once.
static LOOKUPS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

/// Whether `localhost` is Calcifer's: it holds `[::1]` on its port as well as 127.0.0.1, so no other account can take ::1 there
/// and answer `localhost` (browsers try ::1 first). Where ::1 can't be had, only `127.0.0.1` names Calcifer.
pub(crate) static LOCALHOST: AtomicBool = AtomicBool::new(false);

/// What sent a `/v1` request: Calcifer's own page, or a program with the lock record's bearer (this computer's cozy).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Client {
    Page,
    Native,
}

/// The account Calcifer runs as, read once.
pub(crate) fn own() -> Option<&'static User> {
    static OWN: std::sync::OnceLock<Option<User>> = std::sync::OnceLock::new();
    OWN.get_or_init(User::current).as_ref()
}

fn text<'a>(headers: &'a HeaderMap, name: impl header::AsHeaderName) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

fn websocket(headers: &HeaderMap) -> bool {
    text(headers, header::UPGRADE).is_some_and(|value| value.eq_ignore_ascii_case("websocket"))
}

/// The listener's own names, `127.0.0.1:<port>` (`[::1]:<port>` on IPv6) and, where Calcifer holds both, `localhost:<port>`: the only
/// `Host` a request may name. A rebinding page's `Host` is its own name, and one that names another port isn't this Calcifer.
fn own_host(host: &str, local: Option<SocketAddr>) -> bool {
    let Some(local) = local else { return false };
    let address = if local.is_ipv6() { "[::1]" } else { "127.0.0.1" };
    let port = local.port();
    host == format!("{address}:{port}") || (LOCALHOST.load(Ordering::Relaxed) && host == format!("localhost:{port}"))
}

/// Every request's checks: a loopback peer, this listener's `Host`, and an `Origin` that is that Host's (required on a WebSocket).
/// A rebinding page is turned away before anything asks who it is.
pub(crate) fn refusal(peer: &Peer, headers: &HeaderMap) -> Option<Refusal> {
    let host = text(headers, header::HOST);
    let origin = text(headers, header::ORIGIN);
    if !peer.addr.ip().is_loopback() {
        Some(refuse(Kind::Forbidden, "not_loopback", "Calcifer serves the loopback interface only"))
    } else if !host.is_some_and(|host| own_host(host, peer.local)) {
        let said = format!("Host {host:?} is not this Calcifer");
        Some(refuse(Kind::Forbidden, "host_not_allowed", said).remedy("reach it at its own address, as `cozy open` gives it"))
    } else if let Some(origin) = origin
        && origin.strip_prefix("http://") != host
    {
        Some(refuse(Kind::Forbidden, "origin_not_allowed", format!("Origin {origin:?} may not reach Calcifer")))
    } else if origin.is_none() && websocket(headers) {
        Some(refuse(Kind::Forbidden, "origin_not_allowed", "a WebSocket to Calcifer must come from its own page's Origin"))
    } else {
        None
    }
}

/// The peer is Calcifer's own OS account, by the kernel's word for its socket: another account on this computer reads nothing
/// through Calcifer, an unlocked encrypted project included. Where the kernel can't say, nothing is let in.
pub(crate) async fn own_user(peer: &Peer) -> Result<(), Refusal> {
    let said = "Calcifer answers only the account it runs as on this computer";
    match (peer.user().await, own()) {
        (Ok(user), Some(own)) if user == own => Ok(()),
        // Which it was, as a code: what the kernel couldn't say of this connection, or that it's another account's.
        (Err(why), _) => Err(Refusal(Error::new(Kind::Forbidden, "other_user", said).cause(format!("peer_{why}")))),
        _ => Err(Refusal(Error::new(Kind::Forbidden, "other_user", said).cause("peer_other_account"))),
    }
}

/// What a browser says of where its request came from (`Sec-Fetch-Site`): no other site's page can say `same-origin`. Another port
/// of this computer is `same-site`, and `none` (a URL typed or opened by another program, or an extension) isn't the page: both
/// refused. With no `Sec-Fetch-Site`, it's no browser: None. The hot reload socket's exact `Origin` (`refusal`) is a browser's word too, for one that
/// sends no `Sec-Fetch-Site` on an upgrade: a GET of that one path alone.
fn browser(method: &Method, path: &str, headers: &HeaderMap) -> Option<Option<Client>> {
    let hot_reload = *method == Method::GET && path == crate::page::HMR_PATH && websocket(headers);
    let Some(site) = text(headers, "sec-fetch-site") else { return hot_reload.then_some(Some(Client::Page)) };
    Some(match site {
        "same-origin" => Some(Client::Page),
        _ => None,
    })
}

/// `/v1`'s rule: Calcifer's account, then its own page or this computer's cozy with the bearer.
pub(crate) async fn admitted(api: &Api, peer: &Peer, method: &Method, path: &str, headers: &HeaderMap) -> Result<Client, Refusal> {
    own_user(peer).await?;
    let (code, said) = match (browser(method, path, headers), bearer(headers)) {
        (Some(None), _) => ("not_own_page", "Calcifer's API answers its own page only: not another site, another port, an extension, or a URL opened directly"),
        (_, Some(token)) if !api.admits(token) => ("unauthenticated", "this isn't the bearer in Calcifer's lock record"),
        (Some(Some(client)), _) => return Ok(client),
        (None, Some(_)) => return Ok(Client::Native),
        (None, None) => ("unauthenticated", "Calcifer answers its own page, and this computer's cozy with the bearer in its lock record"),
    };
    let kind = if code == "unauthenticated" { Kind::Credential } else { Kind::Forbidden };
    Err(Refusal(Error::new(kind, code, said).next("cozy open")))
}
