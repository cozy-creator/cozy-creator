//! The ONE address predicate, moved here WITH the transport (tfs-049, cr-012's lesson).
//!
//! cozy-runtime's `internal/egress.py` states the hazard this file exists against: *"the
//! failure mode this module exists against is not 'the SSRF check was wrong', it is 'there
//! were two downloaders and only one had the check'."* The byte mover moved into tensorfs,
//! so the predicate moved with it — `blocked()` below is the port of `egress.blocked`, one
//! class list, and cr-090 retargets the `one-address-predicate` fence at this file.
//!
//! **The allowlist is a deployment-wide DECLARATION and resolution is fail-closed.** A host
//! absent from the allowlist refuses. A host that resolves to a private, loopback,
//! link-local, multicast, reserved or unspecified address refuses — EVERY address it
//! resolves to, not the first, because a name with one public and one 169.254.169.254
//! answer is the metadata-service bypass with an extra step. An empty allowlist is no
//! egress at all; nothing widens it at request time.
//!
//! **The connection is made to the addresses that were checked.** `egress.py` named its own
//! DNS-rebinding limit — validate at resolve time, connect by hostname, window of one
//! connect — as work rather than papering over it. Here the checked `SocketAddr`s are what
//! the socket connects to, with SNI/Host still carrying the hostname, so a name that
//! answers publicly during the check cannot answer privately during the connect.
//!
//! **Local origins are the operator's declaration, per origin.** Loopback/private addresses and
//! plaintext http are admitted only for the exact origins (`scheme://host:port`) the process's
//! operator declared ([`declare_local`]: cozy's `local_origins` and its configured Hub; else
//! `TENSORFS_LOCAL_ORIGINS`), at the first URL and at every redirect hop alike. Nothing a run, a
//! package or a Hub answer names widens it: a declared loopback Hub that redirects to
//! 169.254.169.254 is refused at that hop. "I trust this host" and "I accept a private address"
//! stay separate statements.

use crate::err::{refuse, Code, Result};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::sync::{Arc, Mutex, OnceLock};

/// The origins this process's operator declared local: set once, before any fetch.
static LOCAL: OnceLock<Vec<String>> = OnceLock::new();

/// Declares the origins a loopback or private address and plaintext http are admitted for, the
/// operator's configuration and never request data. The first declaration stands; one that would
/// change it is refused. Undeclared, the process reads `TENSORFS_LOCAL_ORIGINS` (comma-separated).
pub fn declare_local<S: AsRef<str>>(origins: impl IntoIterator<Item = S>) -> Result<()> {
    let mut declared = Vec::new();
    for origin in origins {
        let origin = origin.as_ref().trim();
        match origin_of(origin) {
            Some(key) => declared.push(key),
            None => return refuse(Code::SOURCE_NOT_ALLOWED, format!("{origin:?} is not an http(s) origin to declare local")),
        }
    }
    declared.sort();
    declared.dedup();
    if *LOCAL.get_or_init(|| declared.clone()) != declared {
        return refuse(Code::SOURCE_NOT_ALLOWED, "this process declared its local origins already");
    }
    Ok(())
}

/// The declared local origins, as a child process is given them (`TENSORFS_LOCAL_ORIGINS`).
pub fn local_origins() -> &'static [String] {
    LOCAL.get_or_init(|| {
        let text = std::env::var("TENSORFS_LOCAL_ORIGINS").unwrap_or_default();
        let mut declared: Vec<String> = text.split(',').filter_map(|o| origin_of(o.trim())).collect();
        declared.sort();
        declared.dedup();
        declared
    })
}

/// Whether `url`'s origin is one the operator declared local.
pub fn is_local(url: &str) -> bool {
    origin_of(url).is_some_and(|origin| local_origins().contains(&origin))
}

/// Whether any declared local origin is on `host`: what the resolver, which sees no port, admits
/// after [`SourcePolicy::check`] judged the whole URL.
fn local_host(host: &str) -> bool {
    let host = host.trim_matches(['[', ']']);
    local_origins().iter().any(|origin| {
        let parsed = super::net::Url::parse(origin).ok();
        parsed.as_ref().and_then(|u| u.host_str()).is_some_and(|at| at.trim_matches(['[', ']']).eq_ignore_ascii_case(host))
    })
}

/// `scheme://host[:port]` of an http(s) URL, the default port omitted.
fn origin_of(url: &str) -> Option<String> {
    let parsed = super::net::Url::parse(url).ok()?;
    matches!(parsed.scheme(), "http" | "https").then(|| parsed.origin().ascii_serialization())
}

/// The address classes no downloader in this tree connects to — the port of
/// `egress._CLASSES`, same names, same order. Returns why `address` is not fetchable, or
/// `""` for a public routable address.
pub(crate) fn blocked(address: IpAddr) -> String {
    let mut named: Vec<&'static str> = Vec::new();
    match canonical(address) {
        IpAddr::V4(v4) => {
            if v4_private(v4) {
                named.push("private");
            }
            if v4.is_loopback() {
                named.push("loopback");
            }
            if v4.is_link_local() {
                named.push("link-local");
            }
            if v4.is_multicast() {
                named.push("multicast");
            }
            if v4_reserved(v4) {
                named.push("reserved");
            }
            if v4.is_unspecified() {
                named.push("unspecified");
            }
        }
        IpAddr::V6(v6) => {
            if v6_unique_local(v6) {
                named.push("private");
            }
            if v6.is_loopback() {
                named.push("loopback");
            }
            if v6_link_local(v6) {
                named.push("link-local");
            }
            if v6.is_multicast() {
                named.push("multicast");
            }
            // FAIL-CLOSED arm: outside 2000::/3 and not already named is not a space this
            // predicate can vouch for, so it refuses as reserved rather than defaulting
            // open. Stricter than the Python original in places, looser nowhere.
            if named.is_empty() && !v6.is_unspecified() && (v6.segments()[0] & 0xe000) != 0x2000 {
                named.push("reserved");
            }
            if v6.is_unspecified() {
                named.push("unspecified");
            }
        }
    }
    named.join(", ")
}

/// v4-mapped and NAT64 spellings of a v4 address are judged AS that v4 address:
/// `::ffff:169.254.169.254` is the metadata service, whatever family the socket claims.
fn canonical(address: IpAddr) -> IpAddr {
    if let IpAddr::V6(v6) = address {
        if let Some(v4) = v6.to_ipv4_mapped() {
            return IpAddr::V4(v4);
        }
        let s = v6.segments();
        if s[0] == 0x64 && s[1] == 0xff9b && s[2..6] == [0, 0, 0, 0] {
            return IpAddr::V4(Ipv4Addr::new(
                (s[6] >> 8) as u8,
                s[6] as u8,
                (s[7] >> 8) as u8,
                s[7] as u8,
            ));
        }
    }
    address
}

fn v4_private(v4: Ipv4Addr) -> bool {
    let o = v4.octets();
    v4.is_private()                                    // 10/8, 172.16/12, 192.168/16
        || o[0] == 0                                   // 0.0.0.0/8 "this network"
        || (o[0] == 100 && (o[1] & 0xc0) == 64)        // 100.64/10 CGNAT
        || (o[0] == 192 && o[1] == 0 && o[2] == 0)     // 192.0.0/24 protocol assignments
        || (o[0] == 198 && (o[1] & 0xfe) == 18)        // 198.18/15 benchmarking
        || v4.is_documentation()
}

/// 198.18/15, the benchmarking range fake-IP resolvers answer from.
fn fake_ip(address: IpAddr) -> bool {
    matches!(canonical(address), IpAddr::V4(v4) if v4.octets()[0] == 198 && (v4.octets()[1] & 0xfe) == 18)
}

fn v4_reserved(v4: Ipv4Addr) -> bool {
    v4.octets()[0] >= 240 // 240/4, includes 255.255.255.255
}

fn v6_unique_local(v6: Ipv6Addr) -> bool {
    (v6.segments()[0] & 0xfe00) == 0xfc00 // fc00::/7
}

fn v6_link_local(v6: Ipv6Addr) -> bool {
    (v6.segments()[0] & 0xffc0) == 0xfe80 // fe80::/10
}

/// EVERY answer for `host`, through one resolver call. A caller that resolved for itself
/// could check a different answer set than it connects over; here the returned addresses
/// are both the checked set and the connect set.
pub(crate) fn resolve(host: &str, port: u16) -> std::io::Result<Vec<SocketAddr>> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(vec![SocketAddr::new(ip, port)]);
    }
    Ok((host, port).to_socket_addrs()?.collect())
}

/// The deployment's declared egress bound. Carried by every caller that hands this module
/// a URL; enforced on the first URL and on every redirect hop, because the hops are only
/// visible here.
#[derive(Debug, Clone, Default)]
pub struct SourcePolicy {
    /// Exact host names, case-insensitive, trailing dot trimmed. An entry starting with a
    /// dot (`.example.com`) admits any subdomain; a bare entry admits only itself. Empty
    /// means NO egress at all, unless `any_host`.
    pub allowed_hosts: Vec<String>,
    /// Any host, set in code alone for bytes held to a digest the caller already has: no
    /// host vouches for anything there. Addresses are still checked.
    pub any_host: bool,
    /// Plaintext http to a public address, for bytes the caller judges itself. It reaches no
    /// private address: only a declared local origin does ([`declare_local`]).
    pub plaintext: bool,
    /// Redirect hop budget. 0 refuses every redirect — a presigned GET has no business
    /// redirecting; a provider handler (tfs-052) that needs hops declares them.
    pub max_redirects: u32,
    /// Answers this policy has already resolved, shared by its clones. One pull asks the
    /// resolver once per host instead of once per object; every answer is still checked.
    pub resolved: Resolved,
}

/// Answers by host, each behind its own lock so one lookup serves every caller.
type Answers = HashMap<String, Arc<Mutex<Option<Vec<SocketAddr>>>>>;

/// One resolution per host for as long as the policy that made it lives. Callers
/// that ask for a host while it resolves wait for that answer: a pull's 512 requests sent
/// as many lookups to a pod's Docker resolver at once, which answered some after 4-8 s.
#[derive(Clone, Default)]
pub struct Resolved(Arc<Mutex<Answers>>);

impl std::fmt::Debug for Resolved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Resolved")
    }
}

impl Resolved {
    fn lookup(&self, host: &str) -> std::io::Result<Vec<SocketAddr>> {
        let slot = Arc::clone(self.0.lock().unwrap().entry(host.to_string()).or_default());
        let mut answer = slot.lock().unwrap();
        if let Some(addrs) = answer.as_ref() {
            return Ok(addrs.clone());
        }
        let addrs = resolve(host, 0)?;
        if !addrs.is_empty() {
            *answer = Some(addrs.clone());
        }
        Ok(addrs)
    }

    /// Answers no connection could use: the next lookup asks the resolver again.
    pub(crate) fn forget(&self, host: &str) {
        self.0.lock().unwrap().remove(host);
    }
}

impl SourcePolicy {
    pub(crate) fn allows_host(&self, host: &str) -> bool {
        if self.any_host {
            return true;
        }
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        self.allowed_hosts.iter().any(|entry| {
            let entry = entry.trim_end_matches('.').to_ascii_lowercase();
            match entry.strip_prefix('.') {
                Some(suffix) => {
                    !suffix.is_empty()
                        && host
                            .strip_suffix(suffix)
                            .is_some_and(|head| head.ends_with('.'))
                }
                None => !entry.is_empty() && host == entry,
            }
        })
    }

    /// The whole front door: scheme, shape, allowlist, resolution, and the class check on
    /// every answer. Everything this module fetches or pushes passes here first, and every
    /// redirect hop passes here again.
    pub(crate) fn check(&self, url: &str) -> Result<()> {
        let (https, host) = self.admit(url)?;
        self.addresses_for(&host, https, is_local(url)).map(drop)
    }

    /// Everything [`check`](Self::check) refuses before resolving: shape, scheme and allowlist.
    /// Returns whether the URL is https, and its host.
    pub(crate) fn admit(&self, url: &str) -> Result<(bool, String)> {
        let (scheme, host) = split_url(url)?;
        let https = scheme == "https";
        if !https && !self.plaintext && !is_local(url) {
            return refuse(
                Code::SOURCE_NOT_ALLOWED,
                format!(
                    "{host}: plaintext http to an origin this computer's operator did not declare local \
                     (`local_origins`) — artifact bytes travel over https"
                ),
            );
        }
        if !self.allows_host(&host) {
            return refuse(
                Code::SOURCE_NOT_ALLOWED,
                format!(
                    "{host:?} is not in this deployment's declared allowed-hosts \
                     ({:?}). The allowlist is a deployment-wide declaration; nothing \
                     widens it at request time",
                    self.allowed_hosts
                ),
            );
        }
        Ok((https, host))
    }

    /// Every answer for `host`, each checked: what the client dials (its port set by the URL). The
    /// scheme was checked with the URL, so a fake-IP answer passes here as it would over https.
    pub(crate) fn addresses(&self, host: &str) -> Result<Vec<SocketAddr>> {
        self.addresses_for(host, true, local_host(host))
    }

    fn addresses_for(&self, host: &str, https: bool, local: bool) -> Result<Vec<SocketAddr>> {
        let addrs = self.resolved.lookup(host).map_err(|error| crate::err::Refusal {
            code: if crate::descriptors::exhausted(&error) { Code::FD_HEADROOM } else { Code::HUB_UNREACHABLE },
            detail: format!("{host}: name resolution failed ({error}); an address this module cannot check is not an address it will connect to"),
        })?;
        if addrs.is_empty() {
            return refuse(Code::HUB_UNREACHABLE, format!("{host} resolved to nothing"));
        }
        if !local {
            let named = host.parse::<IpAddr>().is_err();
            for addr in &addrs {
                // Fake-IP DNS (some datacenters' transparent proxies) answers every name in
                // 198.18/15 and forwards to the real host. Over https the certificate must
                // verify for `host`, which authenticates the peer, and 198.18/15 is no
                // internal-service range; plaintext and IP literals stay strict.
                if https && named && fake_ip(addr.ip()) {
                    continue;
                }
                let why = blocked(addr.ip());
                if !why.is_empty() {
                    return refuse(
                        Code::SOURCE_NOT_ALLOWED,
                        format!(
                            "{host} resolves to {}, which is {why} and not a public \
                             address. EVERY answer is checked, not the first: a name with \
                             one public answer and one 169.254.169.254 is the \
                             metadata-service bypass with an extra step",
                            addr.ip()
                        ),
                    );
                }
            }
        }
        Ok(addrs)
    }
}

/// The host a base URL names — what a caller scopes a credential to, and what naming a
/// hub base declares.
pub(crate) fn base_host(url: &str) -> Result<String> {
    split_url(url).map(|(_, host)| host)
}

/// The small strict split this module needs. Userinfo and fragments are refused outright —
/// a capability URL carrying either is not one clean origin.
fn split_url(url: &str) -> Result<(String, String)> {
    if url.is_empty() || url.len() > 16 * 1024 || !url.is_ascii() {
        return refuse(
            Code::SOURCE_NOT_ALLOWED,
            "source URL is empty, over 16 KiB, or not ASCII",
        );
    }
    // A fragment is never sent to a server.
    let url = url.split('#').next().unwrap_or(url);
    let Some((scheme, rest)) = url.split_once("://") else {
        return refuse(Code::SOURCE_NOT_ALLOWED, "source URL is not absolute");
    };
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return refuse(
            Code::SOURCE_NOT_ALLOWED,
            format!("{scheme}: not an allowed transport scheme (http/https only)"),
        );
    }
    let authority = &rest[..rest.find(['/', '?']).unwrap_or(rest.len())];
    if authority.is_empty() || authority.contains('@') {
        return refuse(
            Code::SOURCE_NOT_ALLOWED,
            "source URL authority is empty or carries userinfo; credentials belong in no host",
        );
    }
    let (host_raw, port_raw) = match authority.strip_prefix('[') {
        Some(bracketed) => {
            let Some((host, after)) = bracketed.split_once(']') else {
                return refuse(
                    Code::SOURCE_NOT_ALLOWED,
                    "source URL IPv6 authority is unterminated",
                );
            };
            match after.strip_prefix(':') {
                Some(p) => (host.to_string(), Some(p.to_string())),
                None if after.is_empty() => (host.to_string(), None),
                None => {
                    return refuse(
                        Code::SOURCE_NOT_ALLOWED,
                        "source URL authority is malformed after the IPv6 bracket",
                    )
                }
            }
        }
        None => match authority.rsplit_once(':') {
            Some((host, port)) => (host.to_string(), Some(port.to_string())),
            None => (authority.to_string(), None),
        },
    };
    if let Some(text) = port_raw.filter(|text| text.parse::<u16>().is_err()) {
        return refuse(Code::SOURCE_NOT_ALLOWED, format!("source URL port {text:?} is not a 16-bit number"));
    }
    let host = host_raw.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return refuse(Code::SOURCE_NOT_ALLOWED, "source URL has no host");
    }
    Ok((scheme, host))
}
