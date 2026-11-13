//! Outbound TCP that never waits on an unroutable address family, and listeners that keep
//! their port.
use std::{
    fs, io,
    net::{IpAddr, SocketAddr, TcpListener},
    path::Path,
};
use tokio::{net::TcpStream, task::JoinSet};

/// A listener on `port`, or with none, on the port `saved` names, else a free one that `saved`
/// then names: this computer's machine keeps its ports across restarts.
pub(crate) fn kept_listener(saved: &Path, host: IpAddr, port: Option<u16>) -> io::Result<TcpListener> {
    if let Some(port) = port {
        return TcpListener::bind((host, port));
    }
    let kept = fs::read_to_string(saved).ok().and_then(|text| text.trim().parse::<u16>().ok());
    let listener = match kept.map(|port| TcpListener::bind((host, port))) {
        Some(Ok(listener)) => listener,
        _ => TcpListener::bind((host, 0))?,
    };
    let bound = listener.local_addr()?.port();
    if kept != Some(bound) {
        super::identity::write_atomic(saved, bound.to_string().as_bytes(), 0o600)?;
    }
    Ok(listener)
}

/// The API's listener and the player's, bound by the stable parent and held there (cl-977): every service it starts serves on
/// them, so one that can't exit takes no port from the next, and connections wait in the backlog through a restart. An exec'd
/// parent takes them from the slots its image handed them on (`hand_on`).
pub(crate) struct Listeners {
    pub api: TcpListener,
    pub webrtc: Option<TcpListener>,
}

const API_FD: i32 = 4;
const WEBRTC_FD: i32 = 3;

impl Listeners {
    /// The API on `host` and `api_port` (none: the port `api-port` names, else a free one); the player's (`player::listen`) where it
    /// has a port granted or is this computer's.
    pub(crate) fn hold(state: &Path, host: IpAddr, api_port: Option<u16>, webrtc_port: Option<u16>, local: bool) -> io::Result<Self> {
        let api = match handed(API_FD, host, api_port) {
            Some(api) => api,
            None => kept_listener(&state.join("api-port"), host, api_port)?,
        };
        let any = std::net::Ipv4Addr::UNSPECIFIED.into();
        let webrtc = match handed(WEBRTC_FD, any, webrtc_port) {
            Some(webrtc) => Some(webrtc),
            None => super::player::listen(state, webrtc_port, local)?,
        };
        Ok(Self { api, webrtc })
    }

    /// Each on its slot for the program this process execs next.
    #[cfg(unix)]
    pub(crate) fn hand_on(&self) -> bool {
        let webrtc = self.webrtc.as_ref().is_none_or(|webrtc| platform::process::hand_on_exec(webrtc, WEBRTC_FD).is_ok());
        webrtc && platform::process::hand_on_exec(&self.api, API_FD).is_ok()
    }
}

/// The listener this image's parent handed on at `fd`, when it listens where it should: on `host`, and on `port` when one is
/// granted. A slot holding anything else (`/dev/null`, when nothing was handed) is none.
#[cfg(unix)]
fn handed(fd: i32, host: IpAddr, port: Option<u16>) -> Option<TcpListener> {
    let held = std::os::fd::OwnedFd::from(platform::process::take_handed(fd)?);
    if rustix::net::sockopt::socket_type(&held).ok()? != rustix::net::SocketType::STREAM {
        return None;
    }
    #[cfg(target_os = "linux")]
    if !rustix::net::sockopt::socket_acceptconn(&held).ok()? {
        return None;
    }
    let listener = TcpListener::from(held);
    let bound = listener.local_addr().ok()?;
    (bound.ip() == host && port.is_none_or(|port| port == bound.port())).then_some(listener)
}

#[cfg(windows)]
fn handed(_fd: i32, _host: IpAddr, _port: Option<u16>) -> Option<TcpListener> {
    None
}

/// Connects to host:port over whichever of its addresses answers first. A host whose IPv6
/// is unroutable otherwise stalls each connect for the kernel's SYN retries (about two
/// minutes) before trying IPv4.
pub(crate) async fn connect(host: &str, port: u16) -> io::Result<TcpStream> {
    connect_any(tokio::net::lookup_host((host, port)).await?).await
}

async fn connect_any(addresses: impl IntoIterator<Item = SocketAddr>) -> io::Result<TcpStream> {
    let mut attempts = JoinSet::new();
    for address in addresses {
        attempts.spawn(TcpStream::connect(address));
    }
    let mut last = io::Error::new(io::ErrorKind::NotFound, "the host has no address");
    while let Some(attempt) = attempts.join_next().await {
        match attempt.map_err(io::Error::other)? {
            Ok(stream) => return Ok(stream),
            Err(error) => last = error,
        }
    }
    Err(last)
}
