//! The gateway's side of the encoder's frame socket: one connection, held
//! while any session watches, whose records are handed to every session that
//! does. Nothing of a frame is kept here beyond what the sessions' own
//! queues hold.
//!
//! An encoder that restarts is found again without the sessions noticing
//! more than a new stream. One that stays away is not waited for: the
//! sessions' queues are closed, and each tells its page.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use bli_kvm_quic_protocol::frame::{HEADER_BYTES, RecordHeader};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    sync::{Notify, mpsc},
};

/// Frames a session may have waiting to be sent. A session that does not
/// keep up loses what waits and starts again from a key frame.
pub const SESSION_QUEUE: usize = 8;
/// How long the socket is given to accept a connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// Between attempts to reach an encoder that is not there.
const RETRY: Duration = Duration::from_millis(500);
/// An encoder that sent nothing for this long, connection after connection,
/// is not waited for any more.
const GIVE_UP: Duration = Duration::from_secs(3);
/// The byte that asks the encoder for a key frame.
const KEY_REQUEST: u8 = b'K';

/// One encoded frame as the encoder made it.
#[derive(Debug)]
pub struct Frame {
    pub header: RecordHeader,
    pub data: Vec<u8>,
}

struct Subscriber {
    frames: mpsc::Sender<Arc<Frame>>,
    /// Set when a frame found the session's queue full.
    lagged: Arc<AtomicBool>,
}

#[derive(Default)]
struct State {
    subscribers: HashMap<u64, Subscriber>,
    next: u64,
    /// Whether the encoder's socket is connected.
    connected: bool,
}

struct Shared {
    path: PathBuf,
    state: Mutex<State>,
    /// A session started or stopped watching.
    changed: Notify,
    /// Somebody needs a key frame.
    key_wanted: Notify,
}

/// The frames of the H.264 encoder, for the sessions that watch.
#[derive(Clone)]
pub struct FrameSource(Arc<Shared>);

/// A session's place among the watchers; dropping it gives the place up.
pub struct Subscription {
    id: u64,
    source: FrameSource,
    pub frames: mpsc::Receiver<Arc<Frame>>,
    lagged: Arc<AtomicBool>,
}

impl Subscription {
    /// Whether a frame was lost to a full queue since this was last asked.
    pub fn take_lagged(&self) -> bool {
        self.lagged.swap(false, Ordering::AcqRel)
    }

    /// Asks the encoder for a key frame.
    pub fn request_key(&self) {
        self.source.0.key_wanted.notify_one();
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let shared = &self.source.0;
        shared.state.lock().unwrap().subscribers.remove(&self.id);
        shared.changed.notify_one();
    }
}

impl FrameSource {
    /// Starts the task that reads the socket while somebody watches.
    pub fn spawn(path: PathBuf) -> Self {
        let source = Self(Arc::new(Shared {
            path,
            state: Mutex::new(State::default()),
            changed: Notify::new(),
            key_wanted: Notify::new(),
        }));
        tokio::spawn(source.clone().run());
        source
    }

    /// A place among the watchers. The encoder is reached first, so a board
    /// without one is known at once; `None` if it could not be reached.
    pub async fn subscribe(&self) -> Option<Subscription> {
        let (frames, receiver) = mpsc::channel(SESSION_QUEUE);
        let lagged = Arc::new(AtomicBool::new(false));
        let id = {
            let mut state = self.0.state.lock().unwrap();
            state.next += 1;
            let id = state.next;
            state.subscribers.insert(
                id,
                Subscriber {
                    frames,
                    lagged: Arc::clone(&lagged),
                },
            );
            id
        };
        let subscription = Subscription {
            id,
            source: self.clone(),
            frames: receiver,
            lagged,
        };
        self.0.changed.notify_one();
        // It starts from a key frame, whoever else is watching already.
        self.0.key_wanted.notify_one();
        let deadline = tokio::time::Instant::now() + CONNECT_TIMEOUT;
        while !self.0.state.lock().unwrap().connected {
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Some(subscription)
    }

    /// Asks the encoder for a key frame, for whoever needs one.
    pub fn request_key(&self) {
        self.0.key_wanted.notify_one();
    }

    fn watched(&self) -> bool {
        !self.0.state.lock().unwrap().subscribers.is_empty()
    }

    async fn run(self) {
        // Since when the encoder has sent nothing, while somebody watches.
        let mut silent_since: Option<Instant> = None;
        loop {
            while !self.watched() {
                silent_since = None;
                self.0.changed.notified().await;
            }
            let connected =
                tokio::time::timeout(CONNECT_TIMEOUT, UnixStream::connect(&self.0.path)).await;
            if let Ok(Ok(stream)) = connected {
                self.0.state.lock().unwrap().connected = true;
                let delivered = self.connection(stream).await;
                self.0.state.lock().unwrap().connected = false;
                if delivered {
                    // It was there, and may be back at once: a restart.
                    silent_since = None;
                    continue;
                }
            }
            if silent_since.get_or_insert_with(Instant::now).elapsed() >= GIVE_UP {
                // The sessions' queues end, and each tells its page.
                self.0.state.lock().unwrap().subscribers.clear();
                silent_since = None;
            } else {
                tokio::time::sleep(RETRY).await;
            }
        }
    }

    /// One connection to the socket, until it fails or nobody watches.
    /// Whether it delivered a frame.
    async fn connection(&self, stream: UnixStream) -> bool {
        let (mut reader, mut writer) = stream.into_split();
        let mut delivered = false;
        let read = async {
            let mut head = [0u8; HEADER_BYTES];
            loop {
                if reader.read_exact(&mut head).await.is_err() {
                    return;
                }
                // The header's own rules bound the size before it is read.
                let Ok(header) = RecordHeader::decode(&head) else {
                    eprintln!("quic gateway: the frame socket sent what is no record");
                    return;
                };
                let mut data = vec![0u8; header.size as usize];
                if reader.read_exact(&mut data).await.is_err() {
                    return;
                }
                delivered = true;
                self.hand_out(Arc::new(Frame { header, data }));
            }
        };
        let ask = async {
            loop {
                self.0.key_wanted.notified().await;
                if writer.write_all(&[KEY_REQUEST]).await.is_err() {
                    return;
                }
            }
        };
        let unwatched = async {
            loop {
                self.0.changed.notified().await;
                if !self.watched() {
                    return;
                }
            }
        };
        tokio::select! {
            () = read => {}
            () = ask => {}
            () = unwatched => {}
        }
        delivered
    }

    fn hand_out(&self, frame: Arc<Frame>) {
        let state = self.0.state.lock().unwrap();
        for subscriber in state.subscribers.values() {
            if subscriber.frames.try_send(Arc::clone(&frame)).is_err() {
                subscriber.lagged.store(true, Ordering::Release);
            }
        }
    }
}
