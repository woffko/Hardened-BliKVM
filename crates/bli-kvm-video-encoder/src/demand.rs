//! Whether anyone watches: the encoder works only while a viewer is connected.
//!
//! The web server holds one connection to the demand socket per WebRTC
//! viewer, for as long as the viewer's signaling lasts, and names the codec
//! on its first line. Without viewers the encoder leaves the capture stream,
//! so it takes no CPU from the MJPEG path, TLS or HID. Every new viewer
//! bumps a generation, which asks the encoder for a key frame.
//!
//! After the codec the server may send `kbps N` lines, at the start and
//! whenever the operator picks another quality: the encoder takes the last one
//! as its target bitrate without a restart. To the H.264 encoder it also sends
//! `engine hardware` or `engine software`, the board's choice of the engine
//! (`docs/design/h264-engine-choice-and-quality-monitor-v1.md`), on every
//! connection and again whenever the choice is made. A connection's first
//! such line that repeats what the encoder has changes nothing; every other
//! one counts as the choice made, the same or another, and lets the next
//! frame through as a key frame, so that a switch shows at once.

use std::io::{self, BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

/// A codec name on the first line is at most this long.
const MAX_REQUEST_BYTES: u64 = 16;
/// A later line (`kbps N`, `engine software`) is at most this long.
const MAX_LINE_BYTES: u64 = 32;
/// Bitrates the encoders accept, in kbit/s.
pub const KBPS_RANGE: std::ops::RangeInclusive<u32> = 64..=20_000;

/// The engine the server asks the H.264 encoder to encode with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineChoice {
    /// The board's video engine, where it can be had.
    Hardware,
    /// x264.
    Software,
}

impl EngineChoice {
    pub fn name(self) -> &'static str {
        match self {
            Self::Hardware => "hardware",
            Self::Software => "software",
        }
    }
}

#[derive(Debug)]
pub struct Demand {
    always: bool,
    viewers: Mutex<usize>,
    changed: Condvar,
    generation: AtomicU64,
    /// The last bitrate the server asked for (0: none yet).
    kbps: std::sync::atomic::AtomicU32,
    /// The last engine the server asked for (0: none yet, 1: hardware,
    /// 2: software), and how many `engine` lines came.
    engine: AtomicU8,
    engine_lines: AtomicU64,
}

impl Demand {
    /// No demand socket: the encoder always runs, as before.
    pub fn always() -> Arc<Self> {
        Arc::new(Self {
            always: true,
            viewers: Mutex::new(0),
            changed: Condvar::new(),
            generation: AtomicU64::new(0),
            kbps: std::sync::atomic::AtomicU32::new(0),
            engine: AtomicU8::new(0),
            engine_lines: AtomicU64::new(0),
        })
    }

    /// Listens on `path`; the socket is given `group` and mode 0660, so only
    /// that group (the web server's) can signal demand.
    pub fn listen(path: &Path, group: Option<u32>) -> io::Result<Arc<Self>> {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let listener = UnixListener::bind(path)?;
        // The unit runs the encoder with that group as its primary group, so
        // the socket normally has it already; the system call filter refuses
        // chown to the service.
        if let Some(group) = group
            && std::os::unix::fs::MetadataExt::gid(&std::fs::metadata(path)?) != group
        {
            std::os::unix::fs::chown(path, None, Some(group))?;
        }
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o660))?;
        let demand = Arc::new(Self {
            always: false,
            viewers: Mutex::new(0),
            changed: Condvar::new(),
            generation: AtomicU64::new(0),
            kbps: std::sync::atomic::AtomicU32::new(0),
            engine: AtomicU8::new(0),
            engine_lines: AtomicU64::new(0),
        });
        let accepting = Arc::clone(&demand);
        thread::spawn(move || {
            for connection in listener.incoming().flatten() {
                let viewer = Arc::clone(&accepting);
                thread::spawn(move || viewer.serve(connection));
            }
        });
        Ok(demand)
    }

    /// One viewer for as long as its connection stays open.
    fn serve(&self, connection: UnixStream) {
        self.add(1);
        let mut reader = BufReader::new(&connection);
        let mut codec = String::new();
        let _ = io::Read::take(&mut reader, MAX_REQUEST_BYTES).read_line(&mut codec);
        // Then `kbps N` and `engine` lines until the connection ends; a line
        // that is too long or not understood is skipped.
        let mut announced = false;
        loop {
            let mut line = Vec::new();
            match io::Read::take(&mut reader, MAX_LINE_BYTES).read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            if line.last() != Some(&b'\n') {
                // Too long: drop the rest of it.
                let mut rest = Vec::new();
                if !matches!(reader.read_until(b'\n', &mut rest), Ok(read) if read > 0) {
                    break;
                }
                continue;
            }
            if let Some(kbps) = parse_kbps(&line) {
                self.kbps.store(kbps, Ordering::Relaxed);
            } else if let Some(choice) = parse_engine(&line) {
                // A new viewer's connection says what the board has chosen:
                // that is no choice made again.
                if announced || self.engine() != Some(choice) {
                    self.choose(choice);
                }
                announced = true;
            }
        }
        self.add(-1);
    }

    /// The engine the server last asked for, if it asked.
    pub fn engine(&self) -> Option<EngineChoice> {
        match self.engine.load(Ordering::Relaxed) {
            1 => Some(EngineChoice::Hardware),
            2 => Some(EngineChoice::Software),
            _ => None,
        }
    }

    /// How many times the engine was chosen, the same or another: a choice
    /// made again lets the encoder try an engine that failed before.
    pub fn engine_lines(&self) -> u64 {
        // Whoever sees a choice counted sees the choice.
        self.engine_lines.load(Ordering::Acquire)
    }

    fn choose(&self, choice: EngineChoice) {
        let code = match choice {
            EngineChoice::Hardware => 1,
            EngineChoice::Software => 2,
        };
        self.engine.store(code, Ordering::Relaxed);
        self.engine_lines.fetch_add(1, Ordering::Release);
        // The next frame comes through as a key frame, from whichever
        // engine encodes it.
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.changed.notify_all();
    }

    /// The bitrate the server last asked for, if it asked.
    pub fn kbps(&self) -> Option<u32> {
        match self.kbps.load(Ordering::Relaxed) {
            0 => None,
            kbps => Some(kbps),
        }
    }

    fn add(&self, delta: isize) {
        let mut viewers = self.viewers.lock().expect("demand lock");
        *viewers = viewers.saturating_add_signed(delta);
        if delta > 0 {
            self.generation.fetch_add(1, Ordering::Relaxed);
        }
        self.changed.notify_all();
    }

    /// Whether the encoder should work now.
    pub fn active(&self) -> bool {
        self.always || *self.viewers.lock().expect("demand lock") > 0
    }

    /// Bumped by every new viewer, who needs a key frame to start.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    /// Blocks until a viewer is connected or `stop` is set.
    pub fn wait_for_viewers(&self, stop: &AtomicBool) {
        let mut viewers = self.viewers.lock().expect("demand lock");
        while !self.always && *viewers == 0 && !stop.load(Ordering::Relaxed) {
            viewers = self
                .changed
                .wait_timeout(viewers, Duration::from_millis(500))
                .expect("demand lock")
                .0;
        }
    }
}

/// `engine hardware\n` or `engine software\n`.
fn parse_engine(line: &[u8]) -> Option<EngineChoice> {
    match std::str::from_utf8(line).ok()?.trim_end() {
        "engine hardware" => Some(EngineChoice::Hardware),
        "engine software" => Some(EngineChoice::Software),
        _ => None,
    }
}

/// `kbps N\n` with N in [`KBPS_RANGE`].
fn parse_kbps(line: &[u8]) -> Option<u32> {
    let text = std::str::from_utf8(line).ok()?.trim_end();
    let kbps = text.strip_prefix("kbps ")?.parse::<u32>().ok()?;
    KBPS_RANGE.contains(&kbps).then_some(kbps)
}

/// The numeric id of `name` in an `/etc/group` style file.
pub fn group_id(group_file: &str, name: &str) -> Option<u32> {
    group_file.lines().find_map(|line| {
        let mut fields = line.split(':');
        (fields.next()? == name).then_some(())?;
        fields.nth(1)?.parse().ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn wait_until(check: impl Fn() -> bool) {
        for _ in 0..200 {
            if check() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("condition not reached");
    }

    #[test]
    fn viewers_start_and_stop_the_work() {
        let directory = tempfile_dir();
        let path = directory.join("demand.sock");
        let demand = Demand::listen(&path, None).unwrap();
        assert!(!demand.active());
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o660);

        let mut first = UnixStream::connect(&path).unwrap();
        first.write_all(b"vp8\n").unwrap();
        wait_until(|| demand.active());
        assert_eq!(demand.generation(), 1);
        let second = UnixStream::connect(&path).unwrap();
        wait_until(|| demand.generation() == 2);
        drop(first);
        thread::sleep(Duration::from_millis(50));
        assert!(demand.active(), "one viewer is still connected");
        drop(second);
        wait_until(|| !demand.active());

        let stop = AtomicBool::new(true);
        demand.wait_for_viewers(&stop);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn the_server_sets_the_bitrate_after_the_codec() {
        let directory = tempfile_dir();
        let path = directory.join("demand.sock");
        let demand = Demand::listen(&path, None).unwrap();
        assert_eq!(demand.kbps(), None);
        let mut viewer = UnixStream::connect(&path).unwrap();
        viewer.write_all(b"h264\nkbps 2500\n").unwrap();
        wait_until(|| demand.kbps() == Some(2500));
        // Not understood, out of range or too long: ignored.
        viewer
            .write_all(b"kbps lots\nkbps 9\nkbps 99999\n")
            .unwrap();
        viewer.write_all(&[b'x'; 100]).unwrap();
        viewer.write_all(b"\nkbps 8000\n").unwrap();
        wait_until(|| demand.kbps() == Some(8000));
        assert!(demand.active());
        drop(viewer);
        wait_until(|| !demand.active());
        // The last bitrate stays for the next viewer's encoder.
        assert_eq!(demand.kbps(), Some(8000));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn the_server_chooses_the_engine_and_a_choice_made_again_counts() {
        let directory = tempfile_dir();
        let path = directory.join("demand.sock");
        let demand = Demand::listen(&path, None).unwrap();
        assert_eq!(demand.engine(), None);
        assert_eq!(demand.engine_lines(), 0);
        let mut viewer = UnixStream::connect(&path).unwrap();
        viewer
            .write_all(b"h264\nkbps 8000\nengine hardware\n")
            .unwrap();
        wait_until(|| demand.engine() == Some(EngineChoice::Hardware));
        assert_eq!(demand.kbps(), Some(8000));
        assert_eq!(demand.engine_lines(), 1);
        // The viewer's arrival and the choice: two key frames asked for.
        wait_until(|| demand.generation() == 2);
        viewer.write_all(b"engine software\n").unwrap();
        wait_until(|| demand.engine() == Some(EngineChoice::Software));
        assert_eq!(demand.engine_lines(), 2);
        assert_eq!(demand.generation(), 3);
        // Not understood or too long: ignored, and no choice counted.
        viewer
            .write_all(b"engine fast\nengine hardware please\nengine\n")
            .unwrap();
        viewer.write_all(&[b'e'; 100]).unwrap();
        viewer.write_all(b"\nkbps 2500\n").unwrap();
        wait_until(|| demand.kbps() == Some(2500));
        assert_eq!(demand.engine(), Some(EngineChoice::Software));
        assert_eq!(demand.engine_lines(), 2);
        // The same choice again still counts: an engine that failed is
        // tried again.
        viewer.write_all(b"engine software\n").unwrap();
        wait_until(|| demand.engine_lines() == 3);
        // Another viewer's connection names the board's choice: no choice
        // made, only the viewer's key frame. Naming another one is one.
        let mut second = UnixStream::connect(&path).unwrap();
        second
            .write_all(b"h264\nkbps 2500\nengine software\nkbps 3000\n")
            .unwrap();
        wait_until(|| demand.kbps() == Some(3000));
        assert_eq!((demand.engine_lines(), demand.generation()), (3, 5));
        second.write_all(b"engine software\n").unwrap();
        wait_until(|| demand.engine_lines() == 4);
        let mut third = UnixStream::connect(&path).unwrap();
        third.write_all(b"h264\nengine hardware\n").unwrap();
        wait_until(|| demand.engine() == Some(EngineChoice::Hardware));
        assert_eq!(demand.engine_lines(), 5);
        drop((viewer, second, third));
        wait_until(|| !demand.active());
        assert_eq!(demand.engine(), Some(EngineChoice::Hardware));
        assert_eq!(EngineChoice::Hardware.name(), "hardware");
        assert_eq!(EngineChoice::Software.name(), "software");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn without_a_socket_the_encoder_always_runs() {
        let demand = Demand::always();
        assert!(demand.active());
        demand.wait_for_viewers(&AtomicBool::new(false));
    }

    #[test]
    fn the_group_id_comes_from_the_group_file() {
        let groups = "root:x:0:\nvideo:x:44:hardened-blikvm-video\nhardened-blikvm:x:998:\n";
        assert_eq!(group_id(groups, "hardened-blikvm"), Some(998));
        assert_eq!(group_id(groups, "video"), Some(44));
        assert_eq!(group_id(groups, "missing"), None);
    }

    fn tempfile_dir() -> std::path::PathBuf {
        let mut random = [0_u8; 8];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut file| io::Read::read_exact(&mut file, &mut random))
            .unwrap();
        let name: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let path = std::env::temp_dir().join(format!("bk-demand-{name}"));
        std::fs::create_dir(&path).unwrap();
        path
    }
}
