//! Who may talk to Calcifer on this computer (design/calcifer-access.md, cl-985). There are no sessions: each request is judged
//! alone. Every one needs a loopback peer and a `Host` naming the listener it reached; an `Origin`, if sent, must be that Host's,
//! and a WebSocket must send one. `/v1` (media and streams too) and the dev page also need the peer's OS account to be Calcifer's.
//! `/v1` then needs a browser's word that the request is Calcifer's own page's (`Sec-Fetch-Site: same-origin`, or `none` on a
//! navigation) or, from anything that isn't a browser, the bearer in the 0600 lock record. GETs change nothing, so a navigation can
//! only read. The page's own files need only the Host: they hold no secret, and go out with `frame-ancestors 'none'`.
use std::{
    fmt,
    net::SocketAddr,
    sync::{Arc, atomic::{AtomicBool, Ordering}},
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

/// Who's at the other end of a connection: its address, the listener it reached, and its OS account, asked of the kernel as the
/// connection is accepted (off the accept loop) and kept for its life.
#[derive(Clone, Debug)]
pub(crate) struct Peer {
    pub(crate) addr: SocketAddr,
    pub(crate) local: Option<SocketAddr>,
    user: Arc<OnceCell<Result<User, &'static str>>>,
}

impl Connected<IncomingStream<'_, TcpListener>> for Peer {
    fn connect_info(stream: IncomingStream<'_, TcpListener>) -> Self {
        let peer = Peer { addr: *stream.remote_addr(), local: stream.io().local_addr().ok(), user: Arc::default() };
        let asking = peer.clone();
        tokio::spawn(async move {
            let _ = asking.user().await;
        });
        peer
    }
}

impl Peer {
    async fn user(&self) -> Result<&User, &'static str> {
        let (peer, local) = (self.addr, self.local);
        let found = self.user.get_or_init(|| async move {
            let local = local.ok_or("no_listener")?;
            tokio::task::spawn_blocking(move || platform::net::tcp_peer_user(peer, local)).await.map_err(|_| "lookup_failed")?
        });
        found.await.as_ref().map_err(|why| *why)
    }
}

/// Whether `localhost` is Calcifer's: it holds `[::1]` on its port as well as 127.0.0.1, so no other account can take ::1 there
/// and answer `localhost` (browsers try ::1 first). Where ::1 can't be had, only `127.0.0.1` names Calcifer.
pub(crate) static LOCALHOST: AtomicBool = AtomicBool::new(false);

/// Who a request acts as: the OS account Calcifer runs as, verified for its connection. A public key, for remote access (cl-986).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Principal {
    User(User),
}

impl fmt::Display for Principal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Principal::User(user) => write!(f, "user:{user}"),
        }
    }
}

/// What sent a `/v1` request: Calcifer's own page, a URL a person or another app opened (a GET navigation, which only reads), or a
/// program with the lock record's bearer (this computer's cozy).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Client {
    Page,
    Navigation,
    Native,
}

/// The account Calcifer runs as, read once.
fn own() -> Option<&'static User> {
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
pub(crate) async fn own_user(peer: &Peer) -> Result<Principal, Refusal> {
    let said = "Calcifer answers only the account it runs as on this computer";
    match (peer.user().await, own()) {
        (Ok(user), Some(own)) if user == own => Ok(Principal::User(own.clone())),
        // Which it was, as a code: what the kernel couldn't say of this connection, or that it's another account's.
        (Err(why), _) => Err(Refusal(Error::new(Kind::Forbidden, "other_user", said).cause(format!("peer_{why}")))),
        _ => Err(Refusal(Error::new(Kind::Forbidden, "other_user", said).cause("peer_other_account"))),
    }
}

/// What a browser says of where its request came from (`Sec-Fetch-Site`): no other site's page can say `same-origin`. Another port
/// of this computer is `same-site`, refused; `none` counts only on a GET navigation, as extensions say it too. With no
/// `Sec-Fetch-Site`, it's no browser: None. The hot reload socket's exact `Origin` (`refusal`) is a browser's word too, for one that
/// sends no `Sec-Fetch-Site` on an upgrade: a GET of that one path alone.
fn browser(method: &Method, path: &str, headers: &HeaderMap) -> Option<Option<Client>> {
    let hot_reload = *method == Method::GET && path == crate::page::HMR_PATH && websocket(headers);
    let Some(site) = text(headers, "sec-fetch-site") else { return hot_reload.then_some(Some(Client::Page)) };
    Some(match site {
        "same-origin" => Some(Client::Page),
        "none" if *method == Method::GET && text(headers, "sec-fetch-mode") == Some("navigate") => Some(Client::Navigation),
        _ => None,
    })
}

/// `/v1`'s rule: Calcifer's account, then its own page (or a URL opened) or this computer's cozy with the bearer.
pub(crate) async fn admitted(api: &Api, peer: &Peer, method: &Method, path: &str, headers: &HeaderMap) -> Result<(Principal, Client), Refusal> {
    let principal = own_user(peer).await?;
    let (code, said) = match (browser(method, path, headers), bearer(headers)) {
        (Some(None), _) => ("not_own_page", "Calcifer answers its own page only, not another site's, another port's or an extension's"),
        (_, Some(token)) if !api.admits(token) => ("unauthenticated", "this isn't the bearer in Calcifer's lock record"),
        (Some(Some(client)), _) => return Ok((principal, client)),
        (None, Some(_)) => return Ok((principal, Client::Native)),
        (None, None) => ("unauthenticated", "Calcifer answers its own page, and this computer's cozy with the bearer in its lock record"),
    };
    let kind = if code == "unauthenticated" { Kind::Credential } else { Kind::Forbidden };
    Err(Refusal(Error::new(kind, code, said).next("cozy open")))
}
