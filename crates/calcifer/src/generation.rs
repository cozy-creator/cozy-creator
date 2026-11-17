//! What Calcifer's stable parent and its generations say to each other (cl-1113), each message a little-endian length and its JSON on
//! the private stream the parent made for that generation (`platform::generation`). The parent holds the lock, the listeners, the
//! bearer and the holder record; a generation holds everything else, and a swap moves that from one generation to the next.
use std::{
    io::{self, Read, Write},
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use platform::ipc::LocalStream;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::sync::{mpsc, oneshot};

/// Bumped when a message changes meaning: a generation that finds another answers so, and its parent restarts in full.
pub(crate) const PROTOCOL: u32 = 1;
/// The largest message: views with their keys are a few KB each.
const MOST: usize = 16 << 20;

/// The parent to a generation.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "say", rename_all = "snake_case")]
pub(crate) enum ToGeneration {
    /// First, what every generation of this parent shares.
    Start { protocol: u32, token: String, address: SocketAddr },
    /// Take the home: open its stores, adopt `views`, start its machines, then say `Ready`.
    TakeOver { views: Vec<crate::project::vault::Carried> },
    /// The holder record names this generation: answer requests.
    Accept,
    /// The next generation is prepared: let go of the home and say `Released`.
    HandOver { to: String },
    /// The next generation didn't start: keep serving.
    SwapFailed { why: String },
    /// The next generation serves: exit.
    Exit,
    /// A service stop: stop as `down` does, say `Down`, exit.
    Stop,
}

/// A generation to its parent.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "say", rename_all = "snake_case")]
pub(crate) enum ToParent {
    /// Ready to take the home, owning nothing yet.
    Prepared { protocol: u32, version: String },
    /// About to answer: what the holder record names.
    Ready { pid: u32, version: String, started_at_ms: i64 },
    /// `POST /v1/restart`, once idle: start the installed cozy beside this one and hand over to it.
    Swap,
    /// Let go of the home: `views` go to the next generation.
    Released { views: Vec<crate::project::vault::Carried> },
    /// Stopped for good (`down`, a service stop): the parent ends after it.
    Down,
}

pub(crate) fn send(stream: &LocalStream, message: &impl Serialize) -> io::Result<()> {
    let body = serde_json::to_vec(message).map_err(io::Error::other)?;
    let mut framed = Vec::with_capacity(body.len() + 4);
    framed.extend_from_slice(&(body.len() as u32).to_le_bytes());
    framed.extend_from_slice(&body);
    let mut writer = stream;
    writer.write_all(&framed)
}

/// The next message; `UnexpectedEof` once the other end is gone.
pub(crate) fn receive<T: DeserializeOwned>(stream: &LocalStream) -> io::Result<T> {
    let mut reader = stream;
    let mut size = [0u8; 4];
    reader.read_exact(&mut size)?;
    let size = u32::from_le_bytes(size) as usize;
    if size > MOST {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "a message larger than any Calcifer sends"));
    }
    let mut body = vec![0; size];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// A generation's end of its stream: what it sends, and the parent's answer to a swap it asked for.
pub(crate) struct Link {
    stream: Mutex<LocalStream>,
    swapping: Mutex<Option<oneshot::Sender<Result<String, String>>>>,
}

impl Link {
    /// The link, and the messages the parent sends that the generation's own course waits on (all but a swap's answer).
    pub(crate) fn open(stream: LocalStream) -> io::Result<(Arc<Self>, mpsc::UnboundedReceiver<ToGeneration>)> {
        let reader = stream.try_clone()?;
        let link = Arc::new(Self { stream: Mutex::new(stream), swapping: Mutex::default() });
        let (said, heard) = mpsc::unbounded_channel();
        let routing = link.clone();
        std::thread::Builder::new().name("calcifer-parent".into()).spawn(move || {
            while let Ok(message) = receive::<ToGeneration>(&reader) {
                let answer = match message {
                    ToGeneration::HandOver { to } => Some((Ok(to.clone()), ToGeneration::HandOver { to })),
                    ToGeneration::SwapFailed { why } => Some((Err(why.clone()), ToGeneration::SwapFailed { why })),
                    message => {
                        let _ = said.send(message);
                        None
                    }
                };
                if let Some((answer, message)) = answer {
                    if let Some(asked) = routing.swapping.lock().expect("swap").take() {
                        let _ = asked.send(answer);
                    }
                    let _ = said.send(message);
                }
            }
            // The parent is gone: this generation ends with it (as its death signal or job would end it).
            std::process::exit(0);
        })?;
        Ok((link, heard))
    }

    pub(crate) fn send(&self, message: &ToParent) -> io::Result<()> {
        send(&self.stream.lock().expect("parent stream"), message)
    }

    /// Asks the parent to start the installed cozy beside this one: the version it hands over to, or why it couldn't start.
    pub(crate) async fn swap(&self) -> Result<String, String> {
        let (asked, answer) = oneshot::channel();
        {
            let mut swapping = self.swapping.lock().expect("swap");
            if swapping.is_some() {
                return Err("a restart is already under way".into());
            }
            *swapping = Some(asked);
        }
        if let Err(e) = self.send(&ToParent::Swap) {
            self.swapping.lock().expect("swap").take();
            return Err(format!("Calcifer's parent didn't hear the restart: {e}"));
        }
        answer.await.unwrap_or_else(|_| Err("Calcifer's parent ended".into()))
    }
}
