//! [`super`] on Windows.
#![allow(unsafe_code)]
use std::{
    ffi::OsString,
    fs::OpenOptions,
    io::{self, Read, Write},
    mem,
    net::TcpListener,
    os::windows::{
        io::{AsRawHandle, AsRawSocket, FromRawHandle, FromRawSocket, OwnedHandle},
        process::ExitStatusExt,
    },
    path::Path,
    process::ExitStatus,
    sync::OnceLock,
};

use windows_sys::Win32::{
    Foundation::{HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT},
    Networking::WinSock::{
        FROM_PROTOCOL_INFO, INVALID_SOCKET, SOCKET, WSA_FLAG_NO_HANDLE_INHERIT, WSA_FLAG_OVERLAPPED, WSADATA, WSADuplicateSocketW,
        WSAGetLastError, WSAPROTOCOL_INFOW, WSASocketW, WSAStartup,
    },
    System::Threading::{GetExitCodeProcess, INFINITE, TerminateProcess, WaitForSingleObject},
};

use super::Taken;
use crate::ipc::LocalStream;

/// Every generation is in this one job, which ends them with the parent (the job's last handle closing). Their own children are in
/// jobs of their own, which they may hand on.
static JOB: OnceLock<OwnedHandle> = OnceLock::new();

/// A started generation.
#[derive(Debug)]
pub struct Child {
    process: OwnedHandle,
    pid: u32,
}

impl Child {
    pub fn pid(&self) -> u32 {
        self.pid
    }
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.waited(0)
    }
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        self.waited(INFINITE)?.ok_or_else(|| io::Error::other("the generation has not exited"))
    }
    pub fn kill(&mut self) -> io::Result<()> {
        // SAFETY: a live process handle this value owns.
        if unsafe { TerminateProcess(self.process.as_raw_handle() as HANDLE, 1) } == 0 && self.try_wait()?.is_none() {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    fn waited(&mut self, ms: u32) -> io::Result<Option<ExitStatus>> {
        // SAFETY: a live process handle.
        match unsafe { WaitForSingleObject(self.process.as_raw_handle() as HANDLE, ms) } {
            WAIT_OBJECT_0 => {}
            WAIT_TIMEOUT => return Ok(None),
            _ => return Err(io::Error::last_os_error()),
        }
        let mut code = 0;
        // SAFETY: a live process handle and a writable code.
        if unsafe { GetExitCodeProcess(self.process.as_raw_handle() as HANDLE, &mut code) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Some(ExitStatus::from_raw(code)))
    }
}

pub(super) fn start(
    program: &Path,
    args: &[OsString],
    log: Option<&Path>,
    dir: &Path,
    child: LocalStream,
    parent: &LocalStream,
    listeners: &[&TcpListener],
) -> io::Result<Child> {
    if JOB.get().is_none() {
        let _ = JOB.set(crate::process::job()?);
    }
    let job = JOB.get().expect("set above");
    let log = log.map(|path| OpenOptions::new().create(true).append(true).open(path)).transpose()?;
    let control = child.as_raw_handle() as HANDLE;
    let mut args = args.to_vec();
    args.push((control as usize).to_string().into());
    args.push(listeners.len().to_string().into());
    let (process, pid) = crate::process::start_in_job(program, &args, log.as_ref(), dir, &[control], job)?;
    drop(child);
    let child = Child { process, pid };
    // The sockets, as records the child turns back into its own: each a length and a WSAPROTOCOL_INFOW.
    let mut writer = parent;
    for listener in listeners {
        // SAFETY: a zeroed plain-old-data record WSADuplicateSocketW fills for the child's process.
        let mut info: WSAPROTOCOL_INFOW = unsafe { mem::zeroed() };
        if unsafe { WSADuplicateSocketW(listener.as_raw_socket() as SOCKET, pid, &mut info) } != 0 {
            return Err(io::Error::from_raw_os_error(unsafe { WSAGetLastError() }));
        }
        // SAFETY: the record as its bytes, for exactly its size.
        let bytes = unsafe { std::slice::from_raw_parts((&info as *const WSAPROTOCOL_INFOW).cast::<u8>(), mem::size_of::<WSAPROTOCOL_INFOW>()) };
        writer.write_all(&(bytes.len() as u32).to_le_bytes())?;
        writer.write_all(bytes)?;
    }
    Ok(child)
}

pub(super) fn take(words: &[OsString]) -> io::Result<Taken> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidInput, "a generation is given its pipe's handle and its sockets' count");
    let [control, count] = words else { return Err(invalid()) };
    let control = control.to_str().and_then(|word| word.parse::<usize>().ok()).filter(|handle| *handle != 0).ok_or_else(invalid)?;
    let count = count.to_str().and_then(|word| word.parse::<usize>().ok()).filter(|count| *count <= 8).ok_or_else(invalid)?;
    // SAFETY: the pipe handle this process inherited for exactly this, owned from here on; `inherited` checks it is a pipe.
    let control = LocalStream::inherited(unsafe { OwnedHandle::from_raw_handle(control as HANDLE) })?;
    let mut data: WSADATA = unsafe { mem::zeroed() };
    // SAFETY: Winsock 2.2, counted like std's own start, which then finds it started.
    let started = unsafe { WSAStartup(0x0202, &mut data) };
    if started != 0 {
        return Err(io::Error::from_raw_os_error(started));
    }
    let mut reader = &control;
    let mut listeners = Vec::with_capacity(count);
    for _ in 0..count {
        let mut size = [0u8; 4];
        reader.read_exact(&mut size)?;
        if u32::from_le_bytes(size) as usize != mem::size_of::<WSAPROTOCOL_INFOW>() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "a socket record of the wrong size"));
        }
        // SAFETY: plain-old-data filled from the parent's record of exactly this size.
        let mut info: WSAPROTOCOL_INFOW = unsafe { mem::zeroed() };
        let bytes = unsafe { std::slice::from_raw_parts_mut((&mut info as *mut WSAPROTOCOL_INFOW).cast::<u8>(), mem::size_of::<WSAPROTOCOL_INFOW>()) };
        reader.read_exact(bytes)?;
        // SAFETY: a protocol record WSADuplicateSocketW made for this process.
        let socket = unsafe {
            WSASocketW(FROM_PROTOCOL_INFO, FROM_PROTOCOL_INFO, FROM_PROTOCOL_INFO, &info, 0, WSA_FLAG_OVERLAPPED | WSA_FLAG_NO_HANDLE_INHERIT)
        };
        if socket == INVALID_SOCKET {
            return Err(io::Error::from_raw_os_error(unsafe { WSAGetLastError() }));
        }
        // SAFETY: a socket this process now owns.
        listeners.push(unsafe { TcpListener::from_raw_socket(socket as u64) });
    }
    Ok(Taken { control, listeners })
}
