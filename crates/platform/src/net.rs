//! This computer's network interfaces, and who is at the other end of a loopback connection.
#![allow(unsafe_code)]
#[cfg(unix)]
use std::{net::Ipv4Addr, ptr};
use std::{fmt, net::{IpAddr, SocketAddr}};

/// Every IPv4 address on an interface. None where the OS will not list them.
#[cfg(unix)]
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

/// An account on this computer, as its OS names it: a uid on Unix, a SID on Windows.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct User(String);

impl User {
    /// The account this process runs as (its effective uid on Unix). None where Windows won't say.
    pub fn current() -> Option<User> {
        #[cfg(unix)]
        {
            Some(User(rustix::process::geteuid().as_raw().to_string()))
        }
        #[cfg(windows)]
        {
            crate::ipc::this_user().ok().map(User)
        }
    }
}

impl fmt::Display for User {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The account whose TCP socket is `peer`, connected to `local` on this computer: a loopback connection's other end, by the
/// kernel's word for it. Linux reads `/proc/net/tcp{,6}`, whose rows name each socket's owner; macOS asks libproc for the sockets
/// of this account's processes, and Windows for the connection's owning process, then its token. None when nothing matches: the
/// connection is gone or closing (only an established socket counts: a closed one's row names root), it's in another network
/// namespace, or (macOS, Windows) its process is one this account may not inspect.
pub fn tcp_peer_user(peer: SocketAddr, local: SocketAddr) -> Option<User> {
    let (peer, local) = (canonical(peer), canonical(local));
    sys::tcp_peer_user(peer, local)
}

/// An IPv4-mapped IPv6 address as its IPv4, as a listener on IPv4 sees it.
fn canonical(address: SocketAddr) -> SocketAddr {
    match address.ip() {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(address, |v4| SocketAddr::new(v4.into(), address.port())),
        IpAddr::V4(_) => address,
    }
}

#[cfg(target_os = "linux")]
mod sys {
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

    use super::{User, canonical};

    pub(super) fn tcp_peer_user(peer: SocketAddr, local: SocketAddr) -> Option<User> {
        for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
            let Ok(text) = std::fs::read_to_string(table) else { continue };
            for row in text.lines().skip(1) {
                // sl local rem st queues timer retransmits uid timeout inode: established (01), with a live socket's inode.
                let fields: Vec<&str> = row.split_whitespace().collect();
                if fields.len() > 9 && fields[3] == "01" && fields[9] != "0" && address(fields[1]) == Some(peer) && address(fields[2]) == Some(local) {
                    return fields[7].parse::<u32>().ok().map(|uid| User(uid.to_string()));
                }
            }
        }
        None
    }

    /// An address as `/proc/net/tcp*` prints it: each 32-bit word of the address in hex as the host holds it, then `:` and the port.
    fn address(text: &str) -> Option<SocketAddr> {
        let (address, port) = text.split_once(':')?;
        let port = u16::from_str_radix(port, 16).ok()?;
        let word = |at: usize| u32::from_str_radix(address.get(at..at + 8)?, 16).ok().map(u32::to_ne_bytes);
        match address.len() {
            8 => Some(SocketAddr::from((Ipv4Addr::from(word(0)?), port))),
            32 => {
                let mut bytes = [0u8; 16];
                for (at, chunk) in bytes.chunks_mut(4).enumerate() {
                    chunk.copy_from_slice(&word(at * 8)?);
                }
                Some(canonical(SocketAddr::from((Ipv6Addr::from(bytes), port))))
            }
            _ => None,
        }
    }
}

/// `<sys/proc_info.h>`'s socket records, which libc doesn't carry. Only some fields are read; the rest keep the layout.
#[cfg(target_os = "macos")]
#[allow(dead_code)]
mod sys {
    use std::{
        ffi::{c_int, c_void},
        mem,
        net::{Ipv4Addr, Ipv6Addr, SocketAddr},
    };

    use super::{User, canonical};

    const PROC_UID_ONLY: u32 = 4;
    const PROC_PIDFDSOCKETINFO: c_int = 3;
    const SOCKINFO_TCP: c_int = 2;
    const TSI_S_ESTABLISHED: c_int = 4;
    const INI_IPV4: u8 = 0x1;
    const INI_IPV6: u8 = 0x2;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct ProcFileinfo {
        openflags: u32,
        status: u32,
        offset: i64,
        kind: i32,
        guardflags: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct SockbufInfo {
        cc: u32,
        hiwat: u32,
        mbcnt: u32,
        mbmax: u32,
        lowat: u32,
        flags: i16,
        timeo: i16,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct InV6 {
        hlim: u8,
        cksum: i32,
        ifindex: u16,
        hops: i16,
    }

    /// `in_sockinfo`: an address is a union of `in4in6_addr` (the IPv4 address in its last 4 bytes) and `in6_addr`.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct InSockinfo {
        fport: c_int,
        lport: c_int,
        gencnt: u64,
        flags: u32,
        flow: u32,
        vflag: u8,
        ip_ttl: u8,
        rfu_1: u32,
        faddr: [u32; 4],
        laddr: [u32; 4],
        v4_tos: u8,
        v6: InV6,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct TcpSockinfo {
        ini: InSockinfo,
        state: c_int,
        timer: [c_int; 4],
        mss: c_int,
        flags: u32,
        rfu_1: u32,
        tp: u64,
    }

    /// `soi_proto`: its largest member, `un_sockinfo`, is 528 bytes.
    #[repr(C)]
    #[derive(Clone, Copy)]
    union Proto {
        tcp: TcpSockinfo,
        size: [u64; 66],
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct SocketInfo {
        stat: libc::vinfo_stat,
        so: u64,
        pcb: u64,
        kind_of_socket: c_int,
        protocol: c_int,
        family: c_int,
        options: i16,
        linger: i16,
        state: i16,
        qlen: i16,
        incqlen: i16,
        qlimit: i16,
        timeo: i16,
        error: u16,
        oobmark: u32,
        rcv: SockbufInfo,
        snd: SockbufInfo,
        kind: c_int,
        rfu_1: u32,
        proto: Proto,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct SocketFdinfo {
        pfi: ProcFileinfo,
        psi: SocketInfo,
    }

    const _: () = assert!(mem::size_of::<SocketFdinfo>() == 792);

    /// Fills `buffer` by `call` (bytes in, bytes written out), doubling it while it comes back full.
    fn listed<T: Copy>(empty: T, mut call: impl FnMut(*mut c_void, c_int) -> c_int) -> Option<Vec<T>> {
        let mut buffer = vec![empty; 256];
        loop {
            let bytes = mem::size_of_val(buffer.as_slice());
            let written = call(buffer.as_mut_ptr().cast(), c_int::try_from(bytes).ok()?);
            let count = usize::try_from(written).ok()? / mem::size_of::<T>();
            if count < buffer.len() {
                buffer.truncate(count);
                return Some(buffer);
            }
            if buffer.len() >= 1 << 20 {
                return None;
            }
            buffer.resize(buffer.len() * 2, empty);
        }
    }

    fn address(vflag: u8, words: [u32; 4], port: c_int) -> Option<SocketAddr> {
        let mut bytes = [0u8; 16];
        for (chunk, word) in bytes.chunks_mut(4).zip(words) {
            chunk.copy_from_slice(&word.to_ne_bytes());
        }
        // The port as the kernel holds it, in network order, in an int.
        let port = u16::from_be(port as u16);
        if vflag & INI_IPV4 != 0 {
            Some(SocketAddr::from((Ipv4Addr::new(bytes[12], bytes[13], bytes[14], bytes[15]), port)))
        } else if vflag & INI_IPV6 != 0 {
            Some(canonical(SocketAddr::from((Ipv6Addr::from(bytes), port))))
        } else {
            None
        }
    }

    /// The socket's owner, among this account's processes: another account's are closed to it anyway.
    pub(super) fn tcp_peer_user(peer: SocketAddr, local: SocketAddr) -> Option<User> {
        let uid = rustix::process::geteuid().as_raw();
        // SAFETY: a writable buffer of the size given.
        let pids = listed(libc::pid_t::default(), |buffer, bytes| unsafe { libc::proc_listpids(PROC_UID_ONLY, uid, buffer, bytes) })?;
        for pid in pids.into_iter().filter(|pid| *pid > 0) {
            let empty = libc::proc_fdinfo { proc_fd: 0, proc_fdtype: 0 };
            // SAFETY: a writable buffer of the size given; a pid gone since is an error, skipped.
            let Some(fds) = listed(empty, |buffer, bytes| unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, buffer, bytes) }) else { continue };
            for fd in fds.iter().filter(|fd| fd.proc_fdtype == libc::PROX_FDTYPE_SOCKET as u32) {
                // SAFETY: plain integers, for which all zeroes is a value.
                let mut info: SocketFdinfo = unsafe { mem::zeroed() };
                let size = mem::size_of::<SocketFdinfo>() as c_int;
                // SAFETY: a writable record of the size given.
                let written = unsafe { libc::proc_pidfdinfo(pid, fd.proc_fd, PROC_PIDFDSOCKETINFO, (&raw mut info).cast(), size) };
                if written < size || info.psi.kind != SOCKINFO_TCP {
                    continue;
                }
                // SAFETY: `soi_kind` says the union holds a `tcp_sockinfo`.
                let tcp = unsafe { info.psi.proto.tcp };
                let ini = tcp.ini;
                if tcp.state == TSI_S_ESTABLISHED && address(ini.vflag, ini.laddr, ini.lport) == Some(peer) && address(ini.vflag, ini.faddr, ini.fport) == Some(local) {
                    return Some(User(info.psi.stat.vst_uid.to_string()));
                }
            }
        }
        None
    }
}

#[cfg(windows)]
mod sys {
    use std::{
        net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
        ptr,
    };

    use windows_sys::Win32::{
        Foundation::{ERROR_INSUFFICIENT_BUFFER, NO_ERROR},
        NetworkManagement::IpHelper::{
            GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCP6TABLE_OWNER_PID, MIB_TCPROW_OWNER_PID, MIB_TCPTABLE_OWNER_PID,
            TCP_TABLE_OWNER_PID_CONNECTIONS,
        },
        System::Threading::PROCESS_QUERY_LIMITED_INFORMATION,
    };

    use super::User;

    const AF_INET: u32 = 2;
    const AF_INET6: u32 = 23;
    /// `MIB_TCP_STATE_ESTAB`.
    const ESTABLISHED: u32 = 5;

    /// A port as the table holds it: network order, in a DWORD's low 16 bits.
    fn port(value: u32) -> u16 {
        u16::from_be(value as u16)
    }

    /// This computer's TCP connections of `family`, as `GetExtendedTcpTable` lists them with their owning processes.
    fn table(family: u32) -> Option<Vec<u64>> {
        let mut buffer: Vec<u64> = Vec::new();
        for _ in 0..8 {
            let mut size = u32::try_from(buffer.len() * 8).ok()?;
            let at = if buffer.is_empty() { ptr::null_mut() } else { buffer.as_mut_ptr().cast() };
            // SAFETY: `size` bytes at `at` (none at first, to learn the size), 8-aligned for the table.
            let result = unsafe { GetExtendedTcpTable(at, &mut size, 0, family, TCP_TABLE_OWNER_PID_CONNECTIONS, 0) };
            match result {
                NO_ERROR if !buffer.is_empty() => return Some(buffer),
                // Connections come and go between the calls: room for a few more.
                ERROR_INSUFFICIENT_BUFFER => buffer.resize((size as usize).div_ceil(8) + 64, 0),
                _ => return None,
            }
        }
        None
    }

    fn owning_pid(peer: SocketAddr, local: SocketAddr) -> Option<u32> {
        match (peer.ip(), local.ip()) {
            (IpAddr::V4(_), IpAddr::V4(_)) => {
                let buffer = table(AF_INET)?;
                let table = buffer.as_ptr().cast::<MIB_TCPTABLE_OWNER_PID>();
                // SAFETY: the call filled a table whose count says how many rows follow it.
                let rows = unsafe {
                    std::slice::from_raw_parts(ptr::addr_of!((*table).table).cast::<MIB_TCPROW_OWNER_PID>(), (*table).dwNumEntries as usize)
                };
                let v4 = |address: u32, at: u32| SocketAddr::from((Ipv4Addr::from(address.to_ne_bytes()), port(at)));
                rows.iter().find(|row| row.dwState == ESTABLISHED && v4(row.dwLocalAddr, row.dwLocalPort) == peer && v4(row.dwRemoteAddr, row.dwRemotePort) == local).map(|row| row.dwOwningPid)
            }
            (IpAddr::V6(_), IpAddr::V6(_)) => {
                let buffer = table(AF_INET6)?;
                let table = buffer.as_ptr().cast::<MIB_TCP6TABLE_OWNER_PID>();
                // SAFETY: as above.
                let rows = unsafe {
                    std::slice::from_raw_parts(ptr::addr_of!((*table).table).cast::<MIB_TCP6ROW_OWNER_PID>(), (*table).dwNumEntries as usize)
                };
                let v6 = |address: [u8; 16], at: u32| SocketAddr::from((Ipv6Addr::from(address), port(at)));
                rows.iter().find(|row| row.dwState == ESTABLISHED && v6(row.ucLocalAddr, row.dwLocalPort) == peer && v6(row.ucRemoteAddr, row.dwRemotePort) == local).map(|row| row.dwOwningPid)
            }
            _ => None,
        }
    }

    /// The owning process's token's user. Another account's process or token is closed to this one: none. The row is read again
    /// once the process is open, whose handle holds its pid: a pid freed and reused between the two reads isn't this socket's.
    pub(super) fn tcp_peer_user(peer: SocketAddr, local: SocketAddr) -> Option<User> {
        let pid = owning_pid(peer, local)?;
        let process = crate::ipc::open_process(pid, PROCESS_QUERY_LIMITED_INFORMATION).ok()?;
        if owning_pid(peer, local)? != pid {
            return None;
        }
        crate::ipc::token_user(std::os::windows::io::AsRawHandle::as_raw_handle(&process)).ok().map(User)
    }
}
