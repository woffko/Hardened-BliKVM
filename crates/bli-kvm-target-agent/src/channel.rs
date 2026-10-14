//! The agent's channel to the board, on a thread of its own: it finds the
//! board's collection, says hello with the system, answers the board's
//! calls with what it reads, pings every few seconds so the board knows it
//! is there, and finds the collection again whenever it goes (a virtual
//! media change, the switch, the board's restart). What it is connected to
//! is said to the icon's window.

use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::Duration;

use bli_kvm_agent_protocol::{Answer, Call, Hello};
use bli_kvm_target_agent::client::Client;
use bli_kvm_target_agent::{VERSION, source_commit};
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::hid::{self, HidLink};
use crate::reads;

/// The program the agent's hello names.
const PROGRAM: &str = "hardened-blikvm-agent";
/// How often the agent pings: well within the board's silence of 10 s.
const PING_EVERY: Duration = Duration::from_secs(3);
const HELLO_TIMEOUT: Duration = Duration::from_secs(2);
const PING_TIMEOUT: Duration = Duration::from_secs(2);
/// How soon a collection that is not there is looked for again.
const LOOK_AGAIN: Duration = Duration::from_secs(2);

/// What the channel is connected to, for the icon.
static STATUS: Mutex<String> = Mutex::new(String::new());
/// The icon's window, told when the status changes.
static WINDOW: AtomicIsize = AtomicIsize::new(0);

pub fn status() -> String {
    let status = STATUS
        .lock()
        .map(|status| status.clone())
        .unwrap_or_default();
    if status.is_empty() {
        "Not connected".into()
    } else {
        status
    }
}

fn set_status(status: String, message: u32) {
    let changed = match STATUS.lock() {
        Ok(mut current) if *current != status => {
            *current = status;
            true
        }
        _ => false,
    };
    let window = WINDOW.load(Ordering::Relaxed);
    if changed && window != 0 {
        // SAFETY: the window lives as long as the process's message loop;
        // a message to one that is gone is dropped by Windows.
        unsafe {
            PostMessageW(window as _, message, 0, 0);
        }
    }
}

/// The agent's reads, for the board's calls.
fn answer(call: Call) -> Result<Answer, String> {
    match call {
        Call::Ping => Ok(Answer::Pong),
        Call::System => Ok(Answer::System(reads::system())),
        Call::Monitors => Ok(Answer::Monitors(reads::monitors())),
        Call::Keyboard => Ok(Answer::Keyboard(reads::keyboard())),
    }
}

/// Starts the channel's thread; `message` is posted to `window` whenever
/// the status changes.
pub fn start(window: isize, message: u32) {
    WINDOW.store(window, Ordering::Relaxed);
    std::thread::spawn(move || {
        loop {
            let path = match hid::find() {
                Ok(Some(path)) => path,
                Ok(None) => {
                    set_status("Not connected: the board's channel is off".into(), message);
                    std::thread::sleep(LOOK_AGAIN);
                    continue;
                }
                Err(error) => {
                    set_status(format!("Not connected: {error}"), message);
                    std::thread::sleep(LOOK_AGAIN);
                    continue;
                }
            };
            let link = match HidLink::open(&path) {
                Ok(link) => link,
                Err(error) => {
                    set_status(format!("Not connected: {error}"), message);
                    std::thread::sleep(LOOK_AGAIN);
                    continue;
                }
            };
            let hello = Hello::new(PROGRAM, VERSION, source_commit()).with_system(reads::system());
            let mut client = Client::new(link, hello).with_answerer(Box::new(answer));
            match client.hello(HELLO_TIMEOUT) {
                Ok(board) => set_status(
                    format!("Connected to {} {}", board.program, board.version),
                    message,
                ),
                Err(error) => {
                    set_status(format!("Not connected: {error}"), message);
                    std::thread::sleep(LOOK_AGAIN);
                    continue;
                }
            }
            loop {
                if let Err(error) = client
                    .serve(PING_EVERY)
                    .and_then(|()| client.ping(PING_TIMEOUT).map(|_| ()))
                {
                    set_status(format!("Not connected: {error}"), message);
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    });
}
