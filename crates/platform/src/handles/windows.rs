//! Windows: a handle crosses by being duplicated into the peer's process, through the process
//! handle the stream keeps; the stream carries a zero marker byte and its value there.
#![allow(unsafe_code)]
use super::Peer;
use crate::{Error, Result, ipc::LocalStream};
use std::{
    io::{self, Read, Write},
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle},
};

/// The peer as the pipe names it (`GetNamedPipe{Client,Server}ProcessId`), checked by its
/// process token: it runs as this user (uid 0 here), or it
/// is refused. The pipes' access list admits only this user; the token check also refuses a
/// squatter's pipe to a dialer, and holds once a pipe admits more.
pub fn peer(stream: &LocalStream) -> io::Result<Peer> {
    let pid = stream.peer_pid()?;
    let (user, me) = (stream.peer_user()?, crate::ipc::this_user()?);
    if user != me {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("process {pid} runs as {user}, not as this user ({me})"),
        ));
    }
    Ok(Peer { pid, uid: 0, gid: 0 })
}

/// One handle, duplicated into the peer and named there.
pub fn send_fd(stream: &LocalStream, handle: &impl AsRawHandle) -> Result<()> {
    let value = crate::ipc::duplicate_into_peer(stream, handle.as_raw_handle())
        .map_err(Error::os("send a handle"))?;
    let mut message = [0u8; 9];
    message[1..].copy_from_slice(&value.to_le_bytes());
    let mut writer = stream;
    writer
        .write_all(&message)
        .map_err(Error::os("send a handle"))?;
    Ok(())
}

/// One handle the peer duplicated into this process.
pub fn recv_fd(stream: &LocalStream) -> Result<OwnedHandle> {
    let mut message = [0u8; 9];
    let mut reader = stream;
    reader
        .read_exact(&mut message)
        .map_err(Error::os("receive a handle"))?;
    if message[0] != 0 {
        return Err(Error::os("receive a handle")(io::Error::new(
            io::ErrorKind::InvalidData,
            "expected a handle marker",
        )));
    }
    let value = u64::from_le_bytes(message[1..].try_into().expect("eight bytes"));
    // SAFETY: the peer duplicated this handle into this process for us to own.
    Ok(unsafe { OwnedHandle::from_raw_handle(value as RawHandle) })
}
