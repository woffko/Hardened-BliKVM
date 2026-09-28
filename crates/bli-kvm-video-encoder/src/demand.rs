//! Whether anyone watches: the encoder works only while a viewer is connected.
//!
//! The web server holds one connection to the demand socket per WebRTC
//! viewer, for as long as the viewer's signaling lasts, and names the codec
//! on its first line. Without viewers the encoder leaves the capture stream,
//! so it takes no CPU from the MJPEG path, TLS or HID. Every new viewer
//! bumps a generation, which asks the encoder for a key frame.

use std::io::{self, BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

/// A codec name on the first line is at most this long.
const MAX_REQUEST_BYTES: u64 = 16;

#[derive(Debug)]
pub struct Demand {
    always: bool,
    viewers: Mutex<usize>,
    changed: Condvar,
    generation: AtomicU64,
}

impl Demand {
    /// No demand socket: the encoder always runs, as before.
    pub fn always() -> Arc<Self> {
        Arc::new(Self {
            always: true,
            viewers: Mutex::new(0),
            changed: Condvar::new(),
            generation: AtomicU64::new(0),
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
        if let Some(group) = group {
            std::os::unix::fs::chown(path, None, Some(group))?;
        }
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o660))?;
        let demand = Arc::new(Self {
            always: false,
            viewers: Mutex::new(0),
            changed: Condvar::new(),
            generation: AtomicU64::new(0),
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
        let mut reader = BufReader::new(io::Read::take(&connection, MAX_REQUEST_BYTES));
        let mut codec = String::new();
        let _ = reader.read_line(&mut codec);
        // Anything after the first line, or its end, is only waited for.
        let mut rest = [0_u8; 64];
        while matches!(io::Read::read(&mut &connection, &mut rest), Ok(read) if read > 0) {}
        self.add(-1);
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
