//! This computer's network interfaces, and who is at the other end of a loopback connection.
#![allow(unsafe_code)]
use std::{net::{Ipv4Addr, SocketAddr}, ptr};

/// Every IPv4 address on an interface. None where the OS will not list them.
pub fn ipv4_addresses() -> Vec<Ipv4Addr> {
    let mut list = ptr::null_mut();
    // SAFETY: fills `list` with a list only `freeifaddrs` below frees.
    if unsafe { libc::getifaddrs(&mut list) } != 0 {
        return vec![];
    }
    let mut found = vec![];
    let mut next = list;
    while !next.is_null() {
        // SAFETY: a node of the list, live until `freeifaddrs`.
        let interface = unsafe { &*next };
        let address = interface.ifa_addr;
        if !address.is_null() {
            // SAFETY: a null-checked address of the list.
            let family = unsafe { (*address).sa_family };
            if i32::from(family) == libc::AF_INET {
                // SAFETY: an AF_INET address is a `sockaddr_in`, perhaps unaligned here.
                let v4 = unsafe { address.cast::<libc::sockaddr_in>().read_unaligned() };
                found.push(Ipv4Addr::from(u32::from_be(v4.sin_addr.s_addr)));
            }
        }
        next = interface.ifa_next;
    }
    // SAFETY: the list `getifaddrs` gave, not read again.
    unsafe { libc::freeifaddrs(list) };
    found
}

/// The user (uid) whose socket is `peer`, connected to `local` on this computer: a loopback TCP connection's other end (cl-980). Read
/// from `/proc/net/tcp` and `tcp6` on Linux, whose rows list each socket's own address, its peer's and its owner. None when no row
/// matches (another network namespace, a connection gone), and on any other OS.
pub fn tcp_peer_uid(peer: SocketAddr, local: SocketAddr) -> Option<u32> {
    #[cfg(target_os = "linux")]
    {
        for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
            let Ok(text) = std::fs::read_to_string(table) else { continue };
            for row in text.lines().skip(1) {
                let fields: Vec<&str> = row.split_whitespace().collect();
                if fields.len() > 7 && proc_address(fields[1]) == Some(peer) && proc_address(fields[2]) == Some(local) {
                    return fields[7].parse().ok();
                }
            }
        }
        None
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (peer, local);
        None
    }
}

/// An address as `/proc/net/tcp*` prints it: each 32-bit word of the address in hex as the host holds it, then `:` and the port.
/// IPv4-mapped IPv6 addresses read as their IPv4, as the listener side sees them.
#[cfg(target_os = "linux")]
fn proc_address(text: &str) -> Option<SocketAddr> {
    let (address, port) = text.split_once(':')?;
    let port = u16::from_str_radix(port, 16).ok()?;
    let word = |at: usize| u32::from_str_radix(address.get(at..at + 8)?, 16).ok().map(u32::to_ne_bytes);
    match address.len() {
        8 => Some(SocketAddr::from((Ipv4Addr::from(word(0)?), port))),
        32 => {
            let mut bytes = [0u8; 16];
            for (at, chunk) in bytes.chunks_mut(4).enumerate() { chunk.copy_from_slice(&word(at * 8)?); }
            let v6 = std::net::Ipv6Addr::from(bytes);
            Some(match v6.to_ipv4_mapped() { Some(v4) => SocketAddr::from((v4, port)), None => SocketAddr::from((v6, port)) })
        }
        _ => None,
    }
}
