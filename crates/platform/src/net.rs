//! This computer's network interfaces, and who is at the other end of a loopback connection.
#![allow(unsafe_code)]
#[cfg(unix)]
use std::{net::Ipv4Addr, ptr};
use std::{fmt, net::{IpAddr, SocketAddr}, time::SystemTime};

/// What the TCP sockets of a set of processes received, read again and again ([`Received::read`]): an install's own downloads,
/// beside which a machine's weights take a share of the link (cl-1007).
#[derive(Default)]
pub struct Received {
    /// Each socket's bytes at the last read, by inode.
    seen: std::collections::HashMap<u32, u64>,
}

impl Received {
    /// What `pids`' TCP sockets received since the last read; a socket new since then counts all it received. None where the OS
    /// won't say: only Linux's sock_diag does (`tcp_info`'s bytes received, Linux 4.1+).
    pub fn read(&mut self, pids: &[u32]) -> Option<u64> {
        sys::received(pids, &mut self.seen)
    }
}

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
/// namespace, or (macOS, Windows) its process is one this account may not inspect. `accepted` is when the connection was accepted:
/// Windows names a connection by the pid that made it, so a process that started after it (that pid reused) is never its owner. The
/// error is a code saying which step failed.
pub fn tcp_peer_user(peer: SocketAddr, local: SocketAddr, accepted: SystemTime) -> Result<User, &'static str> {
    let (peer, local) = (canonical(peer), canonical(local));
    sys::tcp_peer_user(peer, local, accepted)
}

/// The cheap answer, where there is one, before the full lookup: Linux's is the lookup itself; macOS asks the process that held the
/// last one (a browser's network process); Windows, the connection table it read last, if that's newer than the connection. None:
/// only the full lookup can say.
pub fn tcp_peer_user_quick(peer: SocketAddr, local: SocketAddr, accepted: SystemTime) -> Option<Result<User, &'static str>> {
    let (peer, local) = (canonical(peer), canonical(local));
    sys::quick(peer, local, accepted)
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
    use std::{
        mem,
        net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
        os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd},
        time::SystemTime,
    };

    use super::{User, canonical};

    /// The kernel's sock_diag, asked for the one socket by its four addresses: its uid and inode, with no scan of every socket
    /// (what `ss` asks). `/proc/net/tcp{,6}` where the netlink family is closed (a seccomp sandbox).
    pub(super) fn tcp_peer_user(peer: SocketAddr, local: SocketAddr, _accepted: SystemTime) -> Result<User, &'static str> {
        match diag(peer, local) {
            Err(Closed) => proc_tables(peer, local),
            Ok(found) => found,
        }
    }

    /// The one socket from sock_diag; where netlink is closed, none: `/proc/net/tcp{,6}`, a read of every socket, waits for its
    /// turn among the full lookups (Calcifer's four at a time) as macOS's and Windows' do.
    pub(super) fn quick(peer: SocketAddr, local: SocketAddr, _accepted: SystemTime) -> Option<Result<User, &'static str>> {
        diag(peer, local).ok()
    }

    const NETLINK_SOCK_DIAG: libc::c_int = 4;
    const SOCK_DIAG_BY_FAMILY: u16 = 20;
    const NLMSG_ERROR: u16 = 2;
    const NLM_F_REQUEST: u16 = 1;
    const TCP_ESTABLISHED: u8 = 1;
    const NO_COOKIE: u32 = !0;

    #[repr(C)]
    #[derive(Clone, Copy)]
    #[allow(dead_code)]
    struct Header {
        len: u32,
        kind: u16,
        flags: u16,
        seq: u32,
        pid: u32,
    }

    /// `inet_diag_sockid`: ports and addresses in network order.
    #[repr(C)]
    #[derive(Clone, Copy)]
    #[allow(dead_code)]
    struct SockId {
        sport: [u8; 2],
        dport: [u8; 2],
        src: [u32; 4],
        dst: [u32; 4],
        interface: u32,
        cookie: [u32; 2],
    }

    /// `inet_diag_req_v2`.
    #[repr(C)]
    #[derive(Clone, Copy)]
    #[allow(dead_code)]
    struct Request {
        header: Header,
        family: u8,
        protocol: u8,
        ext: u8,
        pad: u8,
        states: u32,
        id: SockId,
    }

    /// `inet_diag_msg`.
    #[repr(C)]
    #[derive(Clone, Copy)]
    #[allow(dead_code)]
    struct Message {
        family: u8,
        state: u8,
        timer: u8,
        retrans: u8,
        id: SockId,
        expires: u32,
        rqueue: u32,
        wqueue: u32,
        uid: u32,
        inode: u32,
    }

    #[repr(C)]
    #[allow(dead_code)]
    struct Kernel {
        family: libc::sa_family_t,
        pad: u16,
        pid: u32,
        groups: u32,
    }

    /// No answer from netlink either way (closed here, or no tcp_diag): `/proc` is asked.
    struct Closed;

    fn words(ip: IpAddr) -> [u32; 4] {
        match ip {
            IpAddr::V4(v4) => [u32::from_ne_bytes(v4.octets()), 0, 0, 0],
            IpAddr::V6(v6) => {
                let o = v6.octets();
                [0, 4, 8, 12].map(|at| u32::from_ne_bytes([o[at], o[at + 1], o[at + 2], o[at + 3]]))
            }
        }
    }

    fn diag(peer: SocketAddr, local: SocketAddr) -> Result<Result<User, &'static str>, Closed> {
        // SAFETY: a plain call; a negative result is the failure.
        let raw = unsafe { libc::socket(libc::AF_NETLINK, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, NETLINK_SOCK_DIAG) };
        if raw < 0 {
            return Err(Closed);
        }
        // SAFETY: a new descriptor that is now ours alone.
        let socket = unsafe { OwnedFd::from_raw_fd(raw) };
        let family = (if peer.is_ipv4() { libc::AF_INET } else { libc::AF_INET6 }) as u8;
        let request = Request {
            header: Header { len: mem::size_of::<Request>() as u32, kind: SOCK_DIAG_BY_FAMILY, flags: NLM_F_REQUEST, seq: 1, pid: 0 },
            family,
            protocol: libc::IPPROTO_TCP as u8,
            ext: 0,
            pad: 0,
            states: 1 << TCP_ESTABLISHED,
            // The peer's own socket: its local end is `peer`, its remote end Calcifer's listener.
            id: SockId {
                sport: peer.port().to_be_bytes(),
                dport: local.port().to_be_bytes(),
                src: words(peer.ip()),
                dst: words(local.ip()),
                interface: 0,
                cookie: [NO_COOKIE; 2],
            },
        };
        let kernel = Kernel { family: libc::AF_NETLINK as libc::sa_family_t, pad: 0, pid: 0, groups: 0 };
        let timeout = libc::timeval { tv_sec: 1, tv_usec: 0 };
        // SAFETY: our descriptor and a timeval of the size given.
        unsafe {
            libc::setsockopt(socket.as_raw_fd(), libc::SOL_SOCKET, libc::SO_RCVTIMEO, (&raw const timeout).cast(), mem::size_of::<libc::timeval>() as libc::socklen_t)
        };
        // SAFETY: our descriptor, the request and the kernel's address, each of the size given.
        let sent = unsafe {
            libc::sendto(
                socket.as_raw_fd(),
                (&raw const request).cast(),
                mem::size_of::<Request>(),
                0,
                (&raw const kernel).cast(),
                mem::size_of::<Kernel>() as libc::socklen_t,
            )
        };
        if sent < 0 {
            return Err(Closed);
        }
        let mut answer = [0u64; 512];
        // SAFETY: our descriptor and a writable buffer of the size given.
        let got = unsafe { libc::recv(socket.as_raw_fd(), answer.as_mut_ptr().cast(), mem::size_of_val(&answer), 0) };
        let got = usize::try_from(got).map_err(|_| Closed)?;
        if got < mem::size_of::<Header>() {
            return Err(Closed);
        }
        // SAFETY: the kernel wrote at least a header at the buffer's 8-aligned start.
        let header = unsafe { answer.as_ptr().cast::<Header>().read() };
        if header.kind == NLMSG_ERROR {
            // ENOENT is both "no such socket" and "no tcp_diag in this kernel": /proc says which.
            return Err(Closed);
        }
        if header.kind != SOCK_DIAG_BY_FAMILY || got < mem::size_of::<Header>() + mem::size_of::<Message>() {
            return Ok(Err("diag_unreadable"));
        }
        // SAFETY: a full inet_diag_msg follows the 16-byte header, 4-aligned.
        let message = unsafe { answer.as_ptr().cast::<u8>().add(mem::size_of::<Header>()).cast::<Message>().read_unaligned() };
        Ok(if message.state == TCP_ESTABLISHED && message.inode != 0 { Ok(User(message.uid.to_string())) } else { Err("no_socket") })
    }

    /// `Received::read` on Linux: the sockets `pids` hold (`/proc/<pid>/fd`), each one's bytes from one sock_diag dump.
    pub(super) fn received(pids: &[u32], seen: &mut std::collections::HashMap<u32, u64>) -> Option<u64> {
        let mut held = std::collections::HashSet::new();
        for pid in pids {
            let Ok(fds) = std::fs::read_dir(format!("/proc/{pid}/fd")) else { continue };
            for fd in fds.flatten() {
                let target = std::fs::read_link(fd.path()).ok();
                let inode = target.as_ref().and_then(|t| t.to_str()?.strip_prefix("socket:[")?.strip_suffix(']')?.parse::<u32>().ok());
                held.extend(inode);
            }
        }
        let mut moved = 0;
        let mut now = std::collections::HashMap::new();
        for family in [libc::AF_INET, libc::AF_INET6] {
            for (inode, bytes) in dump(family as u8)?.into_iter().filter(|(inode, _)| held.contains(inode)) {
                moved += bytes.saturating_sub(seen.get(&inode).copied().unwrap_or(0));
                now.insert(inode, bytes);
            }
        }
        *seen = now;
        Some(moved)
    }

    const NLM_F_DUMP: u16 = 0x300;
    const NLMSG_DONE: u16 = 3;
    const INET_DIAG_INFO: u16 = 2;
    /// `struct tcp_info`'s `tcpi_bytes_received`: its offset, and the length it ends at.
    const BYTES_RECEIVED: usize = 128;

    /// Every TCP socket of `family` in this network namespace: its inode and the bytes it received. None where netlink won't answer.
    fn dump(family: u8) -> Option<Vec<(u32, u64)>> {
        // SAFETY: a plain call; a negative result is the failure.
        let raw = unsafe { libc::socket(libc::AF_NETLINK, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, NETLINK_SOCK_DIAG) };
        if raw < 0 {
            return None;
        }
        // SAFETY: a new descriptor that is now ours alone.
        let socket = unsafe { OwnedFd::from_raw_fd(raw) };
        let request = Request {
            header: Header { len: mem::size_of::<Request>() as u32, kind: SOCK_DIAG_BY_FAMILY, flags: NLM_F_REQUEST | NLM_F_DUMP, seq: 1, pid: 0 },
            family,
            protocol: libc::IPPROTO_TCP as u8,
            ext: 1 << (INET_DIAG_INFO - 1),
            pad: 0,
            states: !0,
            id: SockId { sport: [0; 2], dport: [0; 2], src: [0; 4], dst: [0; 4], interface: 0, cookie: [NO_COOKIE; 2] },
        };
        let kernel = Kernel { family: libc::AF_NETLINK as libc::sa_family_t, pad: 0, pid: 0, groups: 0 };
        let timeout = libc::timeval { tv_sec: 1, tv_usec: 0 };
        // SAFETY: our descriptor and a timeval of the size given.
        unsafe {
            libc::setsockopt(socket.as_raw_fd(), libc::SOL_SOCKET, libc::SO_RCVTIMEO, (&raw const timeout).cast(), mem::size_of::<libc::timeval>() as libc::socklen_t)
        };
        // SAFETY: our descriptor, the request and the kernel's address, each of the size given.
        let sent = unsafe {
            libc::sendto(socket.as_raw_fd(), (&raw const request).cast(), mem::size_of::<Request>(), 0, (&raw const kernel).cast(), mem::size_of::<Kernel>() as libc::socklen_t)
        };
        if sent < 0 {
            return None;
        }
        let mut found = vec![];
        let mut answer = vec![0u64; 4096];
        loop {
            // SAFETY: our descriptor and a writable buffer of the size given.
            let got = unsafe { libc::recv(socket.as_raw_fd(), answer.as_mut_ptr().cast(), answer.len() * 8, 0) };
            let got = usize::try_from(got).ok()?;
            // SAFETY: the buffer's first `got` bytes, which the kernel wrote.
            let bytes = unsafe { std::slice::from_raw_parts(answer.as_ptr().cast::<u8>(), got) };
            let mut at = 0;
            while at + mem::size_of::<Header>() <= got {
                // SAFETY: a header's bytes within what was received, read unaligned.
                let header = unsafe { bytes.as_ptr().add(at).cast::<Header>().read_unaligned() };
                let len = header.len as usize;
                if header.kind == NLMSG_DONE {
                    return Some(found);
                }
                if header.kind == NLMSG_ERROR || len < mem::size_of::<Header>() || at + len > got {
                    return None;
                }
                let body = &bytes[at + mem::size_of::<Header>()..at + len];
                if header.kind == SOCK_DIAG_BY_FAMILY && body.len() >= mem::size_of::<Message>() {
                    // SAFETY: a full inet_diag_msg at the body's start, read unaligned.
                    let message = unsafe { body.as_ptr().cast::<Message>().read_unaligned() };
                    let mut attribute = mem::size_of::<Message>().next_multiple_of(4);
                    while attribute + 4 <= body.len() {
                        let size = usize::from(u16::from_ne_bytes([body[attribute], body[attribute + 1]]));
                        let kind = u16::from_ne_bytes([body[attribute + 2], body[attribute + 3]]);
                        if size < 4 || attribute + size > body.len() {
                            break;
                        }
                        let info = &body[attribute + 4..attribute + size];
                        if kind == INET_DIAG_INFO && info.len() >= BYTES_RECEIVED + 8 {
                            let mut word = [0; 8];
                            word.copy_from_slice(&info[BYTES_RECEIVED..BYTES_RECEIVED + 8]);
                            found.push((message.inode, u64::from_ne_bytes(word)));
                        }
                        attribute += size.next_multiple_of(4);
                    }
                }
                at += len.next_multiple_of(4);
            }
        }
    }

    fn proc_tables(peer: SocketAddr, local: SocketAddr) -> Result<User, &'static str> {
        for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
            let Ok(text) = std::fs::read_to_string(table) else { continue };
            for row in text.lines().skip(1) {
                // sl local rem st queues timer retransmits uid timeout inode: established (01), with a live socket's inode.
                let fields: Vec<&str> = row.split_whitespace().collect();
                if fields.len() > 9 && fields[3] == "01" && fields[9] != "0" && address(fields[1]) == Some(peer) && address(fields[2]) == Some(local) {
                    return fields[7].parse::<u32>().map(|uid| User(uid.to_string())).map_err(|_| "uid_unreadable");
                }
            }
        }
        Err("no_socket")
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
        sync::atomic::{AtomicI32, Ordering},
        time::SystemTime,
    };

    use super::{User, canonical};

    /// `Received::read`: macOS keeps no count of a socket's bytes that an unprivileged reader can ask for.
    pub(super) fn received(_pids: &[u32], _seen: &mut std::collections::HashMap<u32, u64>) -> Option<u64> {
        None
    }

    const PROC_UID_ONLY: u32 = 4;
    const PROC_PIDFDSOCKETINFO: c_int = 3;
    const SOCKINFO_TCP: c_int = 2;
    const TSI_S_ESTABLISHED: c_int = 4;
    /// The pid whose socket the last lookup found.
    static LAST: AtomicI32 = AtomicI32::new(0);
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

    /// The effective uid of the process holding the socket, asked of it now: the pid listed may have gone and been reused since.
    fn owner(pid: libc::pid_t) -> Result<User, &'static str> {
        // SAFETY: plain integers and byte arrays, for which all zeroes is a value.
        let mut info: libc::proc_bsdinfo = unsafe { mem::zeroed() };
        let size = mem::size_of::<libc::proc_bsdinfo>() as c_int;
        // SAFETY: a writable record of the size given.
        let written = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
        (written == size).then(|| User(info.pbi_uid.to_string())).ok_or("process_unreadable")
    }

    /// Whether `pid` holds the established socket from `peer` to `local`, counting the sockets and TCP sockets it looked at.
    fn holds(pid: libc::pid_t, peer: SocketAddr, local: SocketAddr, seen: &mut (u32, u32)) -> bool {
        let empty = libc::proc_fdinfo { proc_fd: 0, proc_fdtype: 0 };
        // SAFETY: a writable buffer of the size given; a pid gone since is an error, and holds nothing.
        let Some(fds) = listed(empty, |buffer, bytes| unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, buffer, bytes) }) else { return false };
        for fd in fds.iter().filter(|fd| fd.proc_fdtype == libc::PROX_FDTYPE_SOCKET as u32) {
            // SAFETY: plain integers, for which all zeroes is a value.
            let mut info: SocketFdinfo = unsafe { mem::zeroed() };
            let size = mem::size_of::<SocketFdinfo>() as c_int;
            // SAFETY: a writable record of the size given.
            let written = unsafe { libc::proc_pidfdinfo(pid, fd.proc_fd, PROC_PIDFDSOCKETINFO, (&raw mut info).cast(), size) };
            seen.0 += 1;
            if written < size || info.psi.kind != SOCKINFO_TCP {
                continue;
            }
            seen.1 += 1;
            // SAFETY: `soi_kind` says the union holds a `tcp_sockinfo`.
            let socket = unsafe { info.psi.proto.tcp };
            let ini = socket.ini;
            if socket.state == TSI_S_ESTABLISHED && address(ini.vflag, ini.laddr, ini.lport) == Some(peer) && address(ini.vflag, ini.faddr, ini.fport) == Some(local) {
                return true;
            }
        }
        false
    }

    /// The process that held the last one: a browser's connections all come from its one network process.
    pub(super) fn quick(peer: SocketAddr, local: SocketAddr, _accepted: SystemTime) -> Option<Result<User, &'static str>> {
        let last = LAST.load(Ordering::Relaxed);
        (last > 0 && holds(last, peer, local, &mut (0, 0))).then(|| owner(last))
    }

    /// The process holding the socket, among this account's: another account's are closed to it anyway. It's the holder now that
    /// counts, whoever made the socket.
    pub(super) fn tcp_peer_user(peer: SocketAddr, local: SocketAddr, _accepted: SystemTime) -> Result<User, &'static str> {
        let uid = rustix::process::geteuid().as_raw();
        // SAFETY: a writable buffer of the size given.
        let pids = listed(libc::pid_t::default(), |buffer, bytes| unsafe { libc::proc_listpids(PROC_UID_ONLY, uid, buffer, bytes) }).ok_or("no_processes")?;
        let mut seen = (0, 0);
        for pid in pids.into_iter().filter(|pid| *pid > 0) {
            if holds(pid, peer, local, &mut seen) {
                LAST.store(pid, Ordering::Relaxed);
                return owner(pid);
            }
        }
        // Which of the steps found nothing.
        Err(match seen { (0, _) => "no_sockets", (_, 0) => "no_tcp", _ => "no_socket" })
    }
}

#[cfg(windows)]
mod sys {
    use std::{
        net::{Ipv4Addr, Ipv6Addr, SocketAddr},
        ptr,
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::{SystemTime, UNIX_EPOCH},
    };

    use windows_sys::Win32::{
        Foundation::{ERROR_INSUFFICIENT_BUFFER, FILETIME, NO_ERROR},
        NetworkManagement::IpHelper::{
            GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCP6TABLE_OWNER_PID, MIB_TCPROW_OWNER_PID, MIB_TCPTABLE_OWNER_PID,
            TCP_TABLE_OWNER_PID_CONNECTIONS,
        },
        System::Threading::{GetProcessTimes, PROCESS_QUERY_LIMITED_INFORMATION},
    };

    use super::User;

    /// `Received::read`: Windows counts a connection's bytes only once an administrator turns its statistics on.
    pub(super) fn received(_pids: &[u32], _seen: &mut std::collections::HashMap<u32, u64>) -> Option<u64> {
        None
    }

    const AF_INET: u32 = 2;
    const AF_INET6: u32 = 23;
    /// `MIB_TCP_STATE_ESTAB`.
    const ESTABLISHED: u32 = 5;

    /// A port as the table holds it: network order, in a DWORD's low 16 bits.
    fn port(value: u32) -> u16 {
        u16::from_be(value as u16)
    }

    /// The words the last table took, so the next is read once, not first measured: the kernel lists every connection each call.
    static WORDS: AtomicUsize = AtomicUsize::new(0);

    /// This computer's TCP connections of `family`, as `GetExtendedTcpTable` lists them with their owning processes.
    fn table(family: u32) -> Option<Vec<u64>> {
        let last = WORDS.load(Ordering::Relaxed);
        let mut buffer: Vec<u64> = if last == 0 { Vec::new() } else { vec![0; last + last / 8 + 64] };
        for _ in 0..8 {
            let mut size = u32::try_from(buffer.len() * 8).ok()?;
            let at = if buffer.is_empty() { ptr::null_mut() } else { buffer.as_mut_ptr().cast() };
            // SAFETY: `size` bytes at `at` (none at first, to learn the size), 8-aligned for the table.
            let result = unsafe { GetExtendedTcpTable(at, &mut size, 0, family, TCP_TABLE_OWNER_PID_CONNECTIONS, 0) };
            match result {
                NO_ERROR if !buffer.is_empty() => {
                    WORDS.store((size as usize).div_ceil(8), Ordering::Relaxed);
                    return Some(buffer);
                }
                // Connections come and go between the calls: room for a few more.
                ERROR_INSUFFICIENT_BUFFER => buffer.resize((size as usize).div_ceil(8) + 64, 0),
                _ => return None,
            }
        }
        None
    }

    /// An established connection's two ends and the pid that made it.
    type Row = (SocketAddr, SocketAddr, u32);

    /// The connections of the family last read, and when: a quick answer for a connection accepted before then.
    static LAST: Mutex<[Option<(SystemTime, Vec<Row>)>; 2]> = Mutex::new([None, None]);

    /// Every established connection of `peer`'s family, read now, and kept as the last.
    fn rows(peer: SocketAddr) -> Option<Vec<Row>> {
        let read_at = SystemTime::now();
        let found: Vec<Row> = if peer.is_ipv4() {
            let buffer = table(AF_INET)?;
            let table = buffer.as_ptr().cast::<MIB_TCPTABLE_OWNER_PID>();
            // SAFETY: the call filled a table whose count says how many rows follow it.
            let rows = unsafe {
                std::slice::from_raw_parts(ptr::addr_of!((*table).table).cast::<MIB_TCPROW_OWNER_PID>(), (*table).dwNumEntries as usize)
            };
            let v4 = |address: u32, at: u32| SocketAddr::from((Ipv4Addr::from(address.to_ne_bytes()), port(at)));
            rows.iter().filter(|row| row.dwState == ESTABLISHED).map(|row| (v4(row.dwLocalAddr, row.dwLocalPort), v4(row.dwRemoteAddr, row.dwRemotePort), row.dwOwningPid)).collect()
        } else {
            let buffer = table(AF_INET6)?;
            let table = buffer.as_ptr().cast::<MIB_TCP6TABLE_OWNER_PID>();
            // SAFETY: as above.
            let rows = unsafe {
                std::slice::from_raw_parts(ptr::addr_of!((*table).table).cast::<MIB_TCP6ROW_OWNER_PID>(), (*table).dwNumEntries as usize)
            };
            let v6 = |address: [u8; 16], at: u32| SocketAddr::from((Ipv6Addr::from(address), port(at)));
            rows.iter().filter(|row| row.dwState == ESTABLISHED).map(|row| (v6(row.ucLocalAddr, row.dwLocalPort), v6(row.ucRemoteAddr, row.dwRemotePort), row.dwOwningPid)).collect()
        };
        LAST.lock().unwrap_or_else(std::sync::PoisonError::into_inner)[usize::from(!peer.is_ipv4())] = Some((read_at, found.clone()));
        Some(found)
    }

    fn pid_of(rows: &[Row], peer: SocketAddr, local: SocketAddr) -> Option<u32> {
        rows.iter().find(|(near, far, _)| *near == peer && *far == local).map(|(_, _, pid)| *pid)
    }

    fn filetime(time: FILETIME) -> u64 {
        (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
    }

    /// `time` as a FILETIME counts it: 100 ns since 1601.
    fn as_filetime(time: SystemTime) -> u64 {
        let since = time.duration_since(UNIX_EPOCH).unwrap_or_default();
        u64::try_from(since.as_nanos() / 100).unwrap_or(u64::MAX).saturating_add(116_444_736_000_000_000)
    }

    /// The token's user of the process `pid` names, if that process started before the connection was accepted. Windows lists a
    /// connection under the pid that made it, even after that process exits while another holds the socket: a process started since
    /// (that pid reused) never made it. Its handle holds the pid from the open on. Another account's process or token is closed.
    fn owner(pid: u32, accepted: SystemTime) -> Result<User, &'static str> {
        let process = crate::ipc::open_process(pid, PROCESS_QUERY_LIMITED_INFORMATION).map_err(|_| "process_closed")?;
        let handle = std::os::windows::io::AsRawHandle::as_raw_handle(&process);
        let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
        // SAFETY: a live process handle and four writable FILETIMEs.
        if unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) } == 0 {
            return Err("process_closed");
        }
        if filetime(created) > as_filetime(accepted) {
            return Err("process_changed");
        }
        crate::ipc::token_user(handle).map(User).map_err(|_| "token_closed")
    }

    /// The table read last, if it was read after the connection was accepted (so the connection is in it).
    pub(super) fn quick(peer: SocketAddr, local: SocketAddr, accepted: SystemTime) -> Option<Result<User, &'static str>> {
        let pid = {
            let last = LAST.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let (read_at, rows) = last[usize::from(!peer.is_ipv4())].as_ref()?;
            if *read_at < accepted { return None; }
            pid_of(rows, peer, local)?
        };
        Some(owner(pid, accepted))
    }

    pub(super) fn tcp_peer_user(peer: SocketAddr, local: SocketAddr, accepted: SystemTime) -> Result<User, &'static str> {
        let rows = rows(peer).ok_or("no_table")?;
        let pid = pid_of(&rows, peer, local).ok_or("no_socket")?;
        owner(pid, accepted)
    }
}
