//! The frame socket: encoded frames for the QUIC gateway
//! (`docs/design/quic-transport-v1.md`).
//!
//! A client receives records, each a header and a whole frame, from the next
//! key frame on. Every client has a queue of its own: one that does not read
//! loses its queue and starts again from a key frame, and never holds the
//! encoder up. A client asks for a key frame by writing one byte; a key
//! frame on request is made at most once per cooldown, and a key frame made
//! for any other reason answers the request as well.

use std::{
    collections::VecDeque,
    ffi::c_void,
    io::{self, Read, Write},
    os::{
        fd::AsRawFd,
        unix::{
            fs::PermissionsExt,
            net::{UnixListener, UnixStream},
        },
    },
    path::Path,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use bli_kvm_quic_protocol::frame::{Kind, RecordHeader};

/// Records a client may have waiting: three seconds of frames.
const MAX_QUEUED_RECORDS: usize = 90;
/// Bytes those records may have together.
const MAX_QUEUED_BYTES: usize = 8 * 1024 * 1024;
/// Clients at once; the gateway is the only one there is.
const MAX_CLIENTS: usize = 4;
/// A key frame on request is made at most this often.
pub const KEY_COOLDOWN: Duration = Duration::from_millis(250);
/// The byte a client writes to ask for a key frame.
pub const KEY_REQUEST: u8 = b'K';
/// A client that takes no bytes for this long is gone.
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Queue {
    records: VecDeque<Arc<[u8]>>,
    bytes: usize,
    /// Nothing is queued until a key frame comes.
    awaiting_key: bool,
    gone: bool,
}

#[derive(Default)]
struct Client {
    queue: Mutex<Queue>,
    ready: Condvar,
}

impl Client {
    fn leave(&self) {
        self.queue.lock().expect("frame queue lock").gone = true;
        self.ready.notify_all();
    }
}

/// The encoder's side of the frame socket.
#[derive(Default)]
pub struct FrameHub {
    clients: Mutex<Vec<Arc<Client>>>,
    key_wanted: AtomicBool,
    last_key_on_request: Mutex<Option<Instant>>,
}

impl FrameHub {
    /// A hub without a socket; `listen` gives it one.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Listens on `path`. Any local process may connect; one that does not
    /// run as `admitted` is turned away, and without `admitted` only this
    /// process's own user is served.
    pub fn listen(path: &Path, admitted: Option<u32>) -> io::Result<Arc<Self>> {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let listener = UnixListener::bind(path)?;
        // The gateway is another user, in no group this unit shares: the
        // socket is open to connect to, and who connects is checked.
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o666))?;
        let admitted = match admitted {
            Some(uid) => uid,
            None => std::os::unix::fs::MetadataExt::uid(&std::fs::metadata(path)?),
        };
        let hub = Self::new();
        let accepting = Arc::clone(&hub);
        thread::spawn(move || {
            for connection in listener.incoming().flatten() {
                match peer_uid(&connection) {
                    Ok(uid) if uid == admitted => {}
                    Ok(uid) => {
                        eprintln!("frame socket: a connection of user {uid} was refused");
                        continue;
                    }
                    Err(_) => continue,
                }
                let hub = Arc::clone(&accepting);
                thread::spawn(move || hub.serve(connection));
            }
        });
        Ok(hub)
    }

    /// Clients that are connected.
    pub fn clients(&self) -> usize {
        self.clients.lock().expect("frame clients lock").len()
    }

    /// Hands a frame to every client that can take it.
    pub fn publish(&self, header: &RecordHeader, frame: &[u8]) {
        let Ok(encoded) = header.encode() else {
            return;
        };
        if header.size as usize != frame.len() {
            return;
        }
        let key = header.kind == Kind::Key;
        if key {
            // Whoever asked has its key frame now.
            self.key_wanted.store(false, Ordering::Relaxed);
        }
        let clients = self.clients.lock().expect("frame clients lock");
        if clients.is_empty() {
            return;
        }
        let mut record = Vec::with_capacity(encoded.len() + frame.len());
        record.extend_from_slice(&encoded);
        record.extend_from_slice(frame);
        let record: Arc<[u8]> = record.into();
        let mut lagging = false;
        for client in clients.iter() {
            let mut queue = client.queue.lock().expect("frame queue lock");
            if queue.gone || (queue.awaiting_key && !key) {
                continue;
            }
            if queue.records.len() >= MAX_QUEUED_RECORDS
                || queue.bytes + record.len() > MAX_QUEUED_BYTES
            {
                // It does not read: what waited is of no use to it any more.
                queue.records.clear();
                queue.bytes = 0;
                if !key {
                    queue.awaiting_key = true;
                    lagging = true;
                    continue;
                }
            }
            queue.awaiting_key = false;
            queue.bytes += record.len();
            queue.records.push_back(Arc::clone(&record));
            client.ready.notify_one();
        }
        drop(clients);
        if lagging {
            self.want_key();
        }
    }

    /// Whether to make a key frame now because a client asked for one.
    pub fn take_key_request(&self, now: Instant) -> bool {
        if !self.key_wanted.load(Ordering::Relaxed) {
            return false;
        }
        let mut last = self.last_key_on_request.lock().expect("key request lock");
        if last.is_some_and(|at| now.saturating_duration_since(at) < KEY_COOLDOWN) {
            // The request stays; it is answered when the cooldown is over.
            return false;
        }
        *last = Some(now);
        self.key_wanted.store(false, Ordering::Relaxed);
        true
    }

    /// Whether a key frame that a client asked for can be made now. The
    /// capture loop then lets its next frame through, changed or not: on a
    /// still screen the answer would otherwise wait for the periodic frame.
    pub fn key_due(&self, now: Instant) -> bool {
        self.key_wanted.load(Ordering::Relaxed)
            && !self
                .last_key_on_request
                .lock()
                .expect("key request lock")
                .is_some_and(|at| now.saturating_duration_since(at) < KEY_COOLDOWN)
    }

    fn want_key(&self) {
        self.key_wanted.store(true, Ordering::Relaxed);
    }

    /// One client, from its connection to its end.
    fn serve(self: Arc<Self>, connection: UnixStream) {
        let client = Arc::new(Client::default());
        {
            let mut clients = self.clients.lock().expect("frame clients lock");
            if clients.len() >= MAX_CLIENTS {
                return;
            }
            client.queue.lock().expect("frame queue lock").awaiting_key = true;
            clients.push(Arc::clone(&client));
        }
        // It starts from a key frame.
        self.want_key();

        // What the client writes: a byte for every key frame it asks for.
        if let Ok(mut requests) = connection.try_clone() {
            let (hub, reading) = (Arc::clone(&self), Arc::clone(&client));
            thread::spawn(move || {
                let mut byte = [0u8; 1];
                loop {
                    match requests.read(&mut byte) {
                        Ok(1) if byte[0] == KEY_REQUEST => hub.want_key(),
                        Ok(1) => {}
                        _ => break,
                    }
                }
                reading.leave();
            });
        }

        let mut connection = connection;
        let _ = connection.set_write_timeout(Some(WRITE_TIMEOUT));
        loop {
            let record = {
                let mut queue = client.queue.lock().expect("frame queue lock");
                loop {
                    if queue.gone {
                        break None;
                    }
                    if let Some(record) = queue.records.pop_front() {
                        queue.bytes -= record.len();
                        break Some(record);
                    }
                    queue = client.ready.wait(queue).expect("frame queue lock");
                }
            };
            let Some(record) = record else {
                break;
            };
            if connection.write_all(&record).is_err() {
                break;
            }
        }
        client.leave();
        let _ = connection.shutdown(std::net::Shutdown::Both);
        self.clients
            .lock()
            .expect("frame clients lock")
            .retain(|other| !Arc::ptr_eq(other, &client));
    }
}

#[repr(C)]
struct PeerCredentials {
    pid: i32,
    uid: u32,
    gid: u32,
}

#[repr(C)]
struct TimeSpec {
    seconds: i64,
    nanoseconds: i64,
}

// The two calls of the C library this module needs; the values below are
// Linux's for the 64-bit targets the encoder is built for.
#[cfg(all(
    target_os = "linux",
    any(target_arch = "aarch64", target_arch = "x86_64")
))]
unsafe extern "C" {
    fn getsockopt(socket: i32, level: i32, name: i32, value: *mut c_void, length: *mut u32) -> i32;
    fn clock_gettime(clock: i32, time: *mut TimeSpec) -> i32;
}

const SOL_SOCKET: i32 = 1;
const SO_PEERCRED: i32 = 17;
const CLOCK_MONOTONIC: i32 = 1;

/// The user of the process at the other end of a Unix socket.
fn peer_uid(connection: &UnixStream) -> io::Result<u32> {
    let mut credentials = PeerCredentials {
        pid: 0,
        uid: u32::MAX,
        gid: u32::MAX,
    };
    let mut length = std::mem::size_of::<PeerCredentials>() as u32;
    // SAFETY: the descriptor is the live socket; the kernel writes at most
    // `length` bytes into the structure, which outlives the call.
    let code = unsafe {
        getsockopt(
            connection.as_raw_fd(),
            SOL_SOCKET,
            SO_PEERCRED,
            (&mut credentials as *mut PeerCredentials).cast(),
            &mut length,
        )
    };
    if code != 0 || length as usize != std::mem::size_of::<PeerCredentials>() {
        return Err(io::Error::last_os_error());
    }
    Ok(credentials.uid)
}

/// The system's monotonic clock in microseconds: the clock every process on
/// the board shares, and the one a frame's capture time is given in.
pub fn monotonic_micros() -> u64 {
    let mut time = TimeSpec {
        seconds: 0,
        nanoseconds: 0,
    };
    // SAFETY: the structure has the layout of the C library's and outlives
    // the call.
    if unsafe { clock_gettime(CLOCK_MONOTONIC, &mut time) } != 0 {
        return 0;
    }
    (time.seconds.max(0) as u64) * 1_000_000 + (time.nanoseconds.max(0) as u64) / 1000
}

/// Turns the instants the encoder keeps into the monotonic clock's
/// microseconds. `Instant` is that clock; it only does not say its value.
#[derive(Debug, Clone, Copy)]
pub struct MonotonicBase {
    instant: Instant,
    micros: u64,
}

impl MonotonicBase {
    pub fn now() -> Self {
        Self {
            instant: Instant::now(),
            micros: monotonic_micros(),
        }
    }

    pub fn micros_at(&self, at: Instant) -> u64 {
        match at.checked_duration_since(self.instant) {
            Some(after) => self.micros.saturating_add(after.as_micros() as u64),
            None => self
                .micros
                .saturating_sub(self.instant.duration_since(at).as_micros() as u64),
        }
    }
}

/// The ids the records of one stream carry: a frame's id rises by one, and a
/// delta frame names the key frame it depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Numbering {
    stream: u32,
    frame: u32,
    reference: u32,
}

impl Numbering {
    /// A new stream: a new encoder, or another size. `stream` is random and
    /// not zero.
    pub fn new(stream: u32) -> Self {
        Self {
            stream: stream.max(1),
            frame: 0,
            reference: 0,
        }
    }

    /// The header of the next encoded frame, or `None` for a delta frame
    /// before the stream's first key frame, which nobody could decode.
    pub fn next(
        &mut self,
        key: bool,
        width: usize,
        height: usize,
        size: usize,
        capture_micros: u64,
    ) -> Option<RecordHeader> {
        self.frame = self.frame.wrapping_add(1).max(1);
        if key {
            self.reference = self.frame;
        }
        if self.reference == 0 {
            return None;
        }
        Some(RecordHeader {
            kind: if key { Kind::Key } else { Kind::Delta },
            width: u16::try_from(width).ok()?,
            height: u16::try_from(height).ok()?,
            stream: self.stream,
            frame: self.frame,
            reference: self.reference,
            size: u32::try_from(size).ok()?,
            capture_micros,
        })
    }
}

/// The numeric id of `name` in an `/etc/passwd` style file.
pub fn user_id(passwd_file: &str, name: &str) -> Option<u32> {
    passwd_file.lines().find_map(|line| {
        let mut fields = line.split(':');
        (fields.next()? == name).then_some(())?;
        fields.nth(1)?.parse().ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bli_kvm_quic_protocol::frame::HEADER_BYTES;
    use std::sync::atomic::AtomicU32;

    fn directory() -> std::path::PathBuf {
        let mut random = [0_u8; 8];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut file| file.read_exact(&mut random))
            .unwrap();
        let name: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let path = std::env::temp_dir().join(format!("bk-frames-{name}"));
        std::fs::create_dir(&path).unwrap();
        path
    }

    fn wait_until(what: &str, check: impl Fn() -> bool) {
        for _ in 0..400 {
            if check() {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("{what}");
    }

    /// Waits until the hub serves `clients` clients and a new one's request
    /// for a key frame is known: `serve` counts a client first and asks for
    /// its key frame after.
    fn wait_for_clients(hub: &FrameHub, clients: usize) {
        wait_until("the clients are not served", || hub.clients() == clients);
        wait_until("a new client did not ask for a key frame", || {
            hub.key_wanted.load(Ordering::Relaxed)
        });
    }

    fn header(kind: Kind, frame: u32, reference: u32, size: usize) -> RecordHeader {
        RecordHeader {
            kind,
            width: 1280,
            height: 720,
            stream: 9,
            frame,
            reference,
            size: size as u32,
            capture_micros: u64::from(frame) * 33_333,
        }
    }

    /// Publishes a frame whose bytes are its id, and returns them.
    fn publish(hub: &FrameHub, kind: Kind, frame: u32, reference: u32, size: usize) -> Vec<u8> {
        let bytes = vec![frame as u8; size];
        hub.publish(&header(kind, frame, reference, size), &bytes);
        bytes
    }

    fn read_record(client: &mut UnixStream) -> (RecordHeader, Vec<u8>) {
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut head = [0u8; HEADER_BYTES];
        client.read_exact(&mut head).unwrap();
        let header = RecordHeader::decode(&head).unwrap();
        let mut frame = vec![0u8; header.size as usize];
        client.read_exact(&mut frame).unwrap();
        (header, frame)
    }

    #[test]
    fn a_client_gets_records_from_the_next_key_frame_on() {
        let directory = directory();
        let path = directory.join("frames.sock");
        let hub = FrameHub::listen(&path, None).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o666);
        // Nobody listens: nothing is kept, and nobody asked for a key frame.
        publish(&hub, Kind::Key, 1, 1, 100);
        assert!(!hub.take_key_request(Instant::now()));

        let mut client = UnixStream::connect(&path).unwrap();
        wait_for_clients(&hub, 1);
        // A new client asks for a key frame by connecting.
        assert!(hub.take_key_request(Instant::now()));
        // Delta frames before it are not for this client.
        publish(&hub, Kind::Delta, 2, 1, 300);
        let key = publish(&hub, Kind::Key, 3, 3, 5000);
        let delta = publish(&hub, Kind::Delta, 4, 3, 700);
        let (first, bytes) = read_record(&mut client);
        assert_eq!(first, header(Kind::Key, 3, 3, 5000));
        assert_eq!(bytes, key);
        let (second, bytes) = read_record(&mut client);
        assert_eq!(second, header(Kind::Delta, 4, 3, 700));
        assert_eq!(bytes, delta);

        // A frame that is not what its header says is not sent.
        hub.publish(&header(Kind::Delta, 5, 3, 10), &[0u8; 11]);
        hub.publish(&header(Kind::Delta, 6, 6, 10), &[0u8; 10]);
        publish(&hub, Kind::Delta, 7, 3, 20);
        assert_eq!(read_record(&mut client).0.frame, 7);

        drop(client);
        wait_until("the client stayed", || hub.clients() == 0);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_key_frame_on_request_is_made_once_per_cooldown() {
        let directory = directory();
        let path = directory.join("frames.sock");
        let hub = FrameHub::listen(&path, None).unwrap();
        let mut client = UnixStream::connect(&path).unwrap();
        wait_for_clients(&hub, 1);
        let start = Instant::now();
        assert!(hub.key_due(start), "for the new client");
        assert!(hub.take_key_request(start), "for the new client");
        assert!(!hub.key_due(start));
        assert!(!hub.take_key_request(start), "one request, one key frame");

        // It asks again at once: the request waits for the cooldown.
        client.write_all(&[KEY_REQUEST]).unwrap();
        wait_until("the request did not arrive", || {
            hub.key_wanted.load(Ordering::Relaxed)
        });
        assert!(!hub.key_due(start + KEY_COOLDOWN - Duration::from_millis(1)));
        assert!(!hub.take_key_request(start + KEY_COOLDOWN - Duration::from_millis(1)));
        assert!(hub.key_due(start + KEY_COOLDOWN));
        assert!(hub.take_key_request(start + KEY_COOLDOWN));
        assert!(!hub.take_key_request(start + KEY_COOLDOWN * 3));

        // Many requests are one; other bytes are none. The client is served
        // to its end once it has stopped writing, and by then every byte it
        // wrote has been read: waiting for the first request only would leave
        // the others to come after the key frame below, and ask for another.
        client.write_all(&[KEY_REQUEST; 50_000]).unwrap();
        client.write_all(b"xyz").unwrap();
        client.shutdown(std::net::Shutdown::Write).unwrap();
        wait_until("the client was not read to its end", || hub.clients() == 0);
        assert!(
            hub.key_wanted.load(Ordering::Relaxed),
            "the requests did not arrive"
        );
        // A key frame made for another reason answers them.
        publish(&hub, Kind::Key, 10, 10, 50);
        assert!(!hub.take_key_request(start + KEY_COOLDOWN * 10));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_client_that_does_not_read_loses_its_queue_and_holds_nobody_up() {
        let directory = directory();
        let path = directory.join("frames.sock");
        let hub = FrameHub::listen(&path, None).unwrap();
        let mut slow = UnixStream::connect(&path).unwrap();
        let mut reading = UnixStream::connect(&path).unwrap();
        wait_for_clients(&hub, 2);
        assert!(hub.take_key_request(Instant::now()));

        // More than a queue holds. The client that reads gets every frame; the
        // other one is not waited for. The publisher is never more than
        // `AHEAD` frames ahead of the last one the reader took, well inside
        // what a queue holds: how fast a loaded host lets the reader run is
        // not what is tested, and a reader that fell a queue behind would be
        // a slow client like the other one.
        const AHEAD: u32 = 16;
        let frame = vec![7u8; 256 * 1024];
        let taken = Arc::new(AtomicU32::new(1));
        hub.publish(&header(Kind::Key, 1, 1, frame.len()), &frame);
        let reader = {
            let taken = Arc::clone(&taken);
            thread::spawn(move || {
                let mut ids = Vec::new();
                while ids.last() != Some(&200) {
                    let id = read_record(&mut reading).0.frame;
                    ids.push(id);
                    taken.store(id, Ordering::Release);
                }
                ids
            })
        };
        // The time spent in `publish`, which is what must not wait for a
        // client; the waits for the reader are not.
        let mut publishing = Duration::ZERO;
        for id in 2..=200u32 {
            let waiting = Instant::now();
            while id > taken.load(Ordering::Acquire) + AHEAD {
                assert!(
                    waiting.elapsed() < Duration::from_secs(5),
                    "the reading client took no frame in five seconds"
                );
                thread::sleep(Duration::from_micros(100));
            }
            let started = Instant::now();
            hub.publish(&header(Kind::Delta, id, 1, frame.len()), &frame);
            publishing += started.elapsed();
        }
        assert!(
            publishing < Duration::from_secs(3),
            "publishing waited for a client: {publishing:?}"
        );
        assert_eq!(reader.join().unwrap(), (1..=200).collect::<Vec<u32>>());

        // The slow client lost its queue; a key frame was asked for it, and
        // from that key frame on it is served again.
        let cooled = Instant::now() + KEY_COOLDOWN;
        assert!(hub.take_key_request(cooled));
        hub.publish(&header(Kind::Delta, 201, 1, 10), &[1u8; 10]);
        hub.publish(&header(Kind::Key, 202, 202, 10), &[2u8; 10]);
        hub.publish(&header(Kind::Delta, 203, 202, 10), &[3u8; 10]);
        // What it reads first is what was in flight when it fell behind;
        // after that comes the key frame, never the frames in between.
        let mut seen = Vec::new();
        loop {
            let (header, _) = read_record(&mut slow);
            seen.push(header.frame);
            if header.frame == 203 {
                break;
            }
        }
        let key_at = seen.iter().position(|id| *id == 202).expect("no key frame");
        assert_eq!(&seen[key_at..], &[202, 203]);
        assert!(!seen.contains(&201));
        assert!(seen[..key_at].windows(2).all(|pair| pair[1] == pair[0] + 1));
        assert!(seen.len() < 150, "{} records reached it", seen.len());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn only_the_admitted_user_is_served() {
        let directory = directory();
        let path = directory.join("frames.sock");
        let own = std::os::unix::fs::MetadataExt::uid(&std::fs::metadata(&directory).unwrap());
        // This test's own user is somebody else's client.
        let hub = FrameHub::listen(&path, Some(own.wrapping_add(1))).unwrap();
        let mut stranger = UnixStream::connect(&path).unwrap();
        stranger
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut byte = [0u8; 1];
        assert_eq!(stranger.read(&mut byte).unwrap(), 0, "it was not closed");
        assert_eq!(hub.clients(), 0);

        let admitted = directory.join("own.sock");
        let hub = FrameHub::listen(&admitted, Some(own)).unwrap();
        let _client = UnixStream::connect(&admitted).unwrap();
        wait_until("the client is not served", || hub.clients() == 1);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn at_most_a_few_clients_are_served() {
        let directory = directory();
        let path = directory.join("frames.sock");
        let hub = FrameHub::listen(&path, None).unwrap();
        let clients: Vec<UnixStream> = (0..MAX_CLIENTS + 2)
            .map(|_| UnixStream::connect(&path).unwrap())
            .collect();
        wait_until("the clients are not served", || {
            hub.clients() == MAX_CLIENTS
        });
        thread::sleep(Duration::from_millis(50));
        assert_eq!(hub.clients(), MAX_CLIENTS);
        drop(clients);
        wait_until("the clients stayed", || hub.clients() == 0);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn the_monotonic_clock_moves_and_instants_map_onto_it() {
        let base = MonotonicBase::now();
        let first = monotonic_micros();
        assert!(first > 0);
        thread::sleep(Duration::from_millis(20));
        let later = Instant::now();
        let second = monotonic_micros();
        assert!(second >= first + 19_000, "{first} then {second}");
        // An instant taken now is the clock's value now, within a millisecond.
        let mapped = base.micros_at(later);
        assert!(mapped.abs_diff(second) < 1000, "{mapped} against {second}");
        // An instant before the base maps before it.
        let earlier = base.micros_at(base.instant - Duration::from_millis(5));
        assert_eq!(base.micros - earlier, 5000);
    }

    #[test]
    fn frames_are_numbered_from_the_streams_first_key_frame() {
        let mut numbering = Numbering::new(0);
        // A delta frame before any key frame is not for the socket, but it
        // has its number.
        assert_eq!(numbering.next(false, 1280, 720, 10, 1), None);
        let key = numbering.next(true, 1280, 720, 5000, 2).unwrap();
        assert_eq!(
            (key.kind, key.stream, key.frame, key.reference, key.size),
            (Kind::Key, 1, 2, 2, 5000)
        );
        assert_eq!((key.width, key.height, key.capture_micros), (1280, 720, 2));
        let delta = numbering.next(false, 1280, 720, 300, 3).unwrap();
        assert_eq!(
            (delta.kind, delta.frame, delta.reference),
            (Kind::Delta, 3, 2)
        );
        let next = numbering.next(true, 1280, 720, 4000, 4).unwrap();
        assert_eq!((next.frame, next.reference), (4, 4));
        assert_eq!(numbering.next(false, 1280, 720, 9, 5).unwrap().reference, 4);
        // Every header is one the socket's reader accepts.
        assert!(delta.encode().is_ok() && key.encode().is_ok());
        // A picture too large for the header's fields is not sent.
        assert_eq!(numbering.next(false, 70_000, 720, 9, 6), None);
    }

    #[test]
    fn the_user_id_comes_from_the_passwd_file() {
        let passwd = "root:x:0:0:root:/root:/bin/bash\nhardened-blikvm-quic:x:983:983:gateway:/nonexistent:/usr/sbin/nologin\n";
        assert_eq!(user_id(passwd, "hardened-blikvm-quic"), Some(983));
        assert_eq!(user_id(passwd, "root"), Some(0));
        assert_eq!(user_id(passwd, "hardened-blikvm"), None);
    }
}
