//! What Calcifer's stable parent and its generations say to each other (cl-1113), each message a little-endian length and its JSON on
//! the private stream the parent made for that generation (`platform::generation`). The parent holds the lock, the listeners, the
//! bearer and the holder record; a generation holds everything else, and a swap moves that from one generation to the next.
use std::{
    fs::File,
    io::{self, Read, Write},
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use platform::ipc::LocalStream;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::sync::{mpsc, oneshot};

use crate::project::vault::Carried;

/// Bumped when a message changes meaning: a generation that finds another answers so, and its parent restarts in full.
pub(crate) const PROTOCOL: u32 = 1;
/// The largest message: views with their keys are a few KB each.
const MOST: usize = 16 << 20;

/// The parent to a generation.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "say", rename_all = "snake_case")]
pub(crate) enum ToGeneration {
    /// First, what every generation of this parent shares. `parent` outlives every generation: what a view's supervisor watches.
    Start { protocol: u32, token: String, address: SocketAddr, parent: platform::process::Birth },
    /// Take the home: open its stores, adopt `views` (each supervisor's stdin follows the message, in order), start its machines,
    /// then say `Ready`.
    TakeOver {
        views: Vec<Carried>,
        /// The streams the last generation had: a page's word on one as it handed over is answered, not refused.
        #[serde(default)]
        streams: Vec<String>,
    },
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
    /// Let go of the home: `views` go to the next generation, each supervisor's stdin following the message, in order.
    Released {
        views: Vec<Carried>,
        #[serde(default)]
        streams: Vec<String>,
    },
    /// Stopped for good (`down`, a service stop): the parent ends after it.
    Down,
}

pub(crate) fn send(stream: &LocalStream, message: &impl Serialize) -> io::Result<()> {
    // A view's keys ride in it: no copy is left behind.
    let mut body = zeroize::Zeroizing::new(serde_json::to_vec(message).map_err(io::Error::other)?);
    let mut framed = zeroize::Zeroizing::new(Vec::with_capacity(body.len() + 4));
    framed.extend_from_slice(&(body.len() as u32).to_le_bytes());
    framed.extend_from_slice(&body);
    body.clear();
    let mut writer = stream;
    writer.write_all(&framed)
}

/// Each carried view's pipe, after the message that names them (Unix: as descriptors; Windows: their values ride in the views).
#[cfg(unix)]
pub(crate) fn send_pipes(stream: &LocalStream, pipes: &[impl std::os::fd::AsFd]) -> io::Result<()> {
    for pipe in pipes {
        platform::handles::send_fd(stream, pipe).map_err(io::Error::other)?;
    }
    Ok(())
}

/// The pipes [`send_pipes`] sent for `views`.
#[cfg(unix)]
pub(crate) fn receive_pipes(stream: &LocalStream, views: &[Carried]) -> io::Result<Vec<platform::handles::Owned>> {
    views.iter().map(|_| platform::handles::recv_fd(stream).map_err(io::Error::other)).collect()
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
    let mut body = zeroize::Zeroizing::new(vec![0; size]);
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// What the parent says, each message with the pipes that came with it.
pub(crate) type Heard = mpsc::UnboundedReceiver<(ToGeneration, Vec<File>)>;

/// A generation's end of its stream: what it sends, and the parent's answer to a swap it asked for.
pub(crate) struct Link {
    stream: Mutex<LocalStream>,
    swapping: Mutex<Option<oneshot::Sender<Result<String, String>>>>,
}

impl Link {
    /// The link, and the messages the parent sends that the generation's own course waits on, each with the pipes that came with it.
    pub(crate) fn open(stream: LocalStream) -> io::Result<(Arc<Self>, Heard)> {
        let reader = stream.try_clone()?;
        let link = Arc::new(Self { stream: Mutex::new(stream), swapping: Mutex::default() });
        let (said, heard) = mpsc::unbounded_channel();
        let routing = link.clone();
        std::thread::Builder::new().name("calcifer-parent".into()).spawn(move || {
            while let Ok(message) = receive::<ToGeneration>(&reader) {
                let pipes = match &message {
                    ToGeneration::TakeOver { views, .. } => match pipes(&reader, views) {
                        Ok(pipes) => pipes,
                        Err(e) => {
                            tracing::warn!("the views handed over came without their pipes: {e}");
                            vec![]
                        }
                    },
                    _ => vec![],
                };
                let answer = match &message {
                    ToGeneration::HandOver { to } => Some(Ok(to.clone())),
                    ToGeneration::SwapFailed { why } => Some(Err(why.clone())),
                    _ => None,
                };
                if let Some(answer) = answer
                    && let Some(asked) = routing.swapping.lock().expect("swap").take()
                {
                    let _ = asked.send(answer);
                }
                let _ = said.send((message, pipes));
            }
            // The parent is gone: this generation ends with it (as its death signal or job would end it).
            std::process::exit(0);
        })?;
        Ok((link, heard))
    }

    pub(crate) fn send(&self, message: &ToParent) -> io::Result<()> {
        send(&self.stream.lock().expect("parent stream"), message)
    }

    /// Lets go of the home with `views` and their pipes. Windows: the parent duplicates each pipe out of this process, so the
    /// pipes come back to be kept until the parent says `Exit`.
    pub(crate) fn release(&self, views: Vec<(Carried, File)>, streams: Vec<String>) -> io::Result<Vec<File>> {
        let stream = self.stream.lock().expect("parent stream");
        #[cfg(unix)]
        {
            let (views, pipes): (Vec<Carried>, Vec<File>) = views.into_iter().unzip();
            send(&stream, &ToParent::Released { views, streams })?;
            send_pipes(&stream, &pipes)?;
            Ok(vec![])
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle as _;
            let (views, pipes): (Vec<Carried>, Vec<File>) = views
                .into_iter()
                .map(|(mut view, pipe)| {
                    view.pipe = pipe.as_raw_handle() as usize as u64;
                    (view, pipe)
                })
                .unzip();
            send(&stream, &ToParent::Released { views, streams })?;
            Ok(pipes)
        }
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

/// The pipes of a `TakeOver`'s views, in order.
fn pipes(stream: &LocalStream, views: &[Carried]) -> io::Result<Vec<File>> {
    #[cfg(unix)]
    return Ok(receive_pipes(stream, views)?.into_iter().map(File::from).collect());
    #[cfg(windows)]
    {
        let _ = stream;
        views.iter().map(|view| platform::generation::handed(view.pipe).map_err(io::Error::other)).collect()
    }
}
