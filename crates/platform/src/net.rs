//! This computer's network interfaces.
#![allow(unsafe_code)]
use std::{net::Ipv4Addr, ptr};

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
