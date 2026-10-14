//! The agent's channel to the board, on a thread of its own: it finds the
//! board's collection, says hello with the system, answers the board's
//! calls with what it reads, pings every few seconds so the board knows it
//! is there, and finds the collection again whenever it goes (a virtual
//! media change, the switch, the board's restart). The user can stop it
//! from the icon's menu, which closes the collection, and start it again.
//! What it is connected to is said to the icon's window.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::time::{Duration, Instant};

use bli_kvm_agent_protocol::{Answer, Call, Hello};
use bli_kvm_target_agent::client::Client;
use bli_kvm_target_agent::names::Words;
use bli_kvm_target_agent::{VERSION, source_commit};
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::actions;
use crate::hid::{self, HidLink};
use crate::reads;

/// The program the agent's hello names.
const PROGRAM: &str = "hardened-blikvm-agent";
/// How often the agent pings: well within the board's silence of 10 s.
const PING_EVERY: Duration = Duration::from_secs(3);
/// How long the channel listens between looks at the stop switch.
const LISTEN: Duration = Duration::from_millis(500);
const HELLO_TIMEOUT: Duration = Duration::from_secs(2);
const PING_TIMEOUT: Duration = Duration::from_secs(2);
/// How soon a collection that is not there is looked for again.
const LOOK_AGAIN: Duration = Duration::from_secs(2);

/// What the channel is connected to, for the icon.
static STATUS: Mutex<String> = Mutex::new(String::new());
/// The icon's window, told when the status changes.
static WINDOW: AtomicIsize = AtomicIsize::new(0);
/// The user stopped the channel from the icon's menu.
static STOPPED: AtomicBool = AtomicBool::new(false);

pub fn status() -> String {
    STATUS
        .lock()
        .map(|status| status.clone())
        .unwrap_or_default()
}

pub fn stopped() -> bool {
    STOPPED.load(Ordering::Relaxed)
}

/// Stops the channel (it closes the collection within half a second) or
/// starts it again.
pub fn set_stopped(stop: bool) {
    STOPPED.store(stop, Ordering::Relaxed);
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

/// The agent's reads and actions, for the board's calls.
fn answer(call: Call) -> Result<Answer, String> {
    match call {
        Call::Ping => Ok(Answer::Pong),
        Call::System => Ok(Answer::System(reads::system())),
        Call::Monitors => Ok(Answer::Monitors(reads::monitors())),
        Call::Keyboard => Ok(Answer::Keyboard(reads::keyboard())),
        Call::SetLayout { id } => actions::set_layout(&id).map(|()| Answer::Done),
        Call::SetDisplays { mode } => actions::set_displays(mode)
            .map(|revert_in_seconds| Answer::DisplaysChanged { revert_in_seconds }),
        Call::KeepDisplays => actions::keep_displays().map(|()| Answer::Done),
        Call::ReadClipboard => actions::read_clipboard().map(Answer::Clipboard),
        Call::WriteClipboard { text } => {
            actions::write_clipboard(WINDOW.load(Ordering::Relaxed) as _, &text)
                .map(|()| Answer::Done)
        }
        Call::Lock => actions::lock().map(|()| Answer::Done),
    }
}

/// Waits `duration`, or less if the user stops the channel meanwhile.
fn pause(duration: Duration) {
    let until = Instant::now() + duration;
    while !stopped() && Instant::now() < until {
        std::thread::sleep(LISTEN.min(until.saturating_duration_since(Instant::now())));
    }
}

/// Starts the channel's thread; `message` is posted to `window` whenever
/// the status changes, which is said in `words`.
pub fn start(window: isize, message: u32, words: Words) {
    WINDOW.store(window, Ordering::Relaxed);
    set_status(words.not_connected.into(), message);
    std::thread::spawn(move || {
        let not_connected = |reason: &dyn std::fmt::Display| {
            set_status(format!("{}: {reason}", words.not_connected), message);
        };
        loop {
            if stopped() {
                set_status(words.stopped.into(), message);
                std::thread::sleep(LISTEN);
                continue;
            }
            let path = match hid::find() {
                Ok(Some(path)) => path,
                Ok(None) => {
                    not_connected(&words.channel_off);
                    pause(LOOK_AGAIN);
                    continue;
                }
                Err(error) => {
                    not_connected(&error);
                    pause(LOOK_AGAIN);
                    continue;
                }
            };
            let link = match HidLink::open(&path) {
                Ok(link) => link,
                Err(error) => {
                    not_connected(&error);
                    pause(LOOK_AGAIN);
                    continue;
                }
            };
            let hello = Hello::new(PROGRAM, VERSION, source_commit()).with_system(reads::system());
            let mut client = Client::new(link, hello).with_answerer(Box::new(answer));
            match client.hello(HELLO_TIMEOUT) {
                Ok(board) => set_status(
                    format!("{} {} {}", words.connected, board.program, board.version),
                    message,
                ),
                Err(error) => {
                    not_connected(&error);
                    pause(LOOK_AGAIN);
                    continue;
                }
            }
            let mut pinged = Instant::now();
            while !stopped() {
                let served = client.serve(LISTEN).and_then(|()| {
                    if pinged.elapsed() >= PING_EVERY {
                        pinged = Instant::now();
                        client.ping(PING_TIMEOUT).map(|_| ())
                    } else {
                        Ok(())
                    }
                });
                if let Err(error) = served {
                    not_connected(&error);
                    break;
                }
            }
            // Dropping the client closes the collection.
            drop(client);
            pause(Duration::from_millis(500));
        }
    });
}
