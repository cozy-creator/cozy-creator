#![allow(unsafe_code)]
use super::Peer;
use crate::Result;
use rustix::net::{SendAncillaryBuffer, SendAncillaryMessage, SendFlags, sendmsg};
use std::{
    io::{self, IoSlice},
    mem::MaybeUninit,
    os::{
        fd::{AsFd, AsRawFd, FromRawFd, OwnedFd},
        unix::net::UnixStream,
    },
};

/// The peer as the kernel names it, whoever it runs as.
#[cfg(target_os = "linux")]
pub fn peer(stream: &UnixStream) -> io::Result<Peer> {
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut size = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: correctly sized writable credential structure and size pointer.
    if unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut _ as *mut libc::c_void,
            &mut size,
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(Peer {
        pid: cred.pid as u32,
        uid: cred.uid,
        gid: cred.gid,
    })
}

/// The peer as the kernel names it, whoever it runs as.
#[cfg(target_os = "macos")]
pub fn peer(stream: &UnixStream) -> io::Result<Peer> {
    let (mut uid, mut gid) = (0, 0);
    // SAFETY: a live socket and two writable ids.
    if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut pid: libc::pid_t = 0;
    let mut size = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: a correctly sized writable pid and size pointer.
    if unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            &mut pid as *mut _ as *mut libc::c_void,
            &mut size,
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(Peer {
        pid: pid as u32,
        uid,
        gid,
    })
}

/// One descriptor and a one-byte marker; the receiver refuses anything else.
pub fn send_fd(stream: &UnixStream, fd: &impl AsFd) -> Result<()> {
    let fds = [fd.as_fd()];
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
    let mut control = SendAncillaryBuffer::new(&mut space);
    if !control.push(SendAncillaryMessage::ScmRights(&fds)) {
        return Err(io::Error::other("no room for the descriptor").into());
    }
    // macOS has no MSG_NOSIGNAL; Rust programs ignore SIGPIPE, so a closed peer is EPIPE there too.
    #[cfg(target_os = "linux")]
    let flags = SendFlags::NOSIGNAL;
    #[cfg(target_os = "macos")]
    let flags = SendFlags::empty();
    if sendmsg(stream, &[IoSlice::new(&[0])], &mut control, flags).map_err(io::Error::from)? != 1 {
        return Err(io::Error::other("descriptor marker truncated").into());
    }
    Ok(())
}

/// Reads the control data itself, not through rustix: macOS reports a truncated header's whole
/// length, past the buffer, and rustix's parser trusts it.
pub fn recv_fd(stream: &UnixStream) -> Result<OwnedFd> {
    let mut marker = [255u8];
    // usize storage satisfies cmsghdr alignment; every iteration is bounded by
    // the kernel-returned control length. Parse delivered fds even on CTRUNC,
    // solely to close that prefix before rejecting the message. Room for a thousand
    // descriptors: macOS installs a whole batch even when it reports it truncated, so a
    // batch must fit to be closed.
    let mut control = [0usize; 512];
    let mut iov = libc::iovec {
        iov_base: marker.as_mut_ptr().cast(),
        iov_len: 1,
    };
    let mut msg = libc::msghdr {
        msg_name: std::ptr::null_mut(),
        msg_namelen: 0,
        msg_iov: &mut iov,
        msg_iovlen: 1,
        msg_control: control.as_mut_ptr().cast(),
        msg_controllen: std::mem::size_of_val(&control) as _,
        msg_flags: 0,
    };
    // SAFETY: correctly sized aligned control/data buffers and live socket.
    // macOS has no MSG_CMSG_CLOEXEC: each descriptor is marked close-on-exec as it is taken.
    #[cfg(target_os = "linux")]
    let flags = libc::MSG_CMSG_CLOEXEC;
    #[cfg(target_os = "macos")]
    let flags = 0;
    // SAFETY: correctly sized aligned control/data buffers and a live socket.
    let count = unsafe { libc::recvmsg(stream.as_raw_fd(), &mut msg, flags) };
    if count < 0 {
        return Err(io::Error::last_os_error().into());
    }
    let mut valid =
        count == 1 && marker == [0] && msg.msg_flags & (libc::MSG_CTRUNC | libc::MSG_TRUNC) == 0;
    let mut received = Vec::new();
    // SAFETY: msghdr contains buffers initialized by recvmsg; bounds checked below.
    unsafe {
        let mut header = libc::CMSG_FIRSTHDR(&msg);
        while !header.is_null() {
            let start = header as usize - msg.msg_control as usize;
            let base = libc::CMSG_LEN(0) as usize;
            // macOS reports a truncated header's whole length: only what fits is read.
            let fits = (msg.msg_controllen as usize).saturating_sub(start);
            let length = ((*header).cmsg_len as usize).min(fits);
            if length < base {
                valid = false;
                break;
            }
            valid &= (*header).cmsg_len as usize == length;
            if (*header).cmsg_level == libc::SOL_SOCKET && (*header).cmsg_type == libc::SCM_RIGHTS {
                let bytes = length - base;
                valid &= bytes.is_multiple_of(std::mem::size_of::<i32>());
                for i in 0..bytes / std::mem::size_of::<i32>() {
                    let fd = std::ptr::read_unaligned(libc::CMSG_DATA(header).cast::<i32>().add(i));
                    #[cfg(target_os = "macos")]
                    libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
                    received.push(OwnedFd::from_raw_fd(fd));
                }
            } else {
                valid = false;
            }
            if length < (*header).cmsg_len as usize {
                break;
            }
            header = libc::CMSG_NXTHDR(&msg, header);
        }
    }
    if !valid || received.len() != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "expected exactly one descriptor",
        )
        .into());
    }
    Ok(received.pop().unwrap())
}
