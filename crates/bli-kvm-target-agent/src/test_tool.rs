//! The channel's test tool for the target's Windows
//! (`docs/baselines/work-plan-20261014-target-agent.md`, gate 1): finds the
//! board's agent collection, says hello, and either sends pings and says
//! their round trips, or keeps pinging and says when the channel went and
//! how soon it came back. What it says is also appended to
//! `hardened-blikvm-agent-test.txt` beside the program.
//!
//! usage: hardened-blikvm-agent-test.exe [PINGS]          (1000 by default)
//!        hardened-blikvm-agent-test.exe watch [SECONDS]  (300 by default)
//!        hardened-blikvm-agent-test.exe show             (what the agent reads)

#[cfg(windows)]
#[path = "hid.rs"]
mod hid;
// The tool reads; what the agent's actions need of the module is unused here.
#[cfg(windows)]
#[allow(dead_code)]
#[path = "reads.rs"]
mod reads;

#[cfg(windows)]
fn main() {
    // As the agent: DLLs loaded later come from the system folder only.
    // SAFETY: a process-wide setting with a documented flag.
    unsafe {
        windows_sys::Win32::System::LibraryLoader::SetDefaultDllDirectories(
            windows_sys::Win32::System::LibraryLoader::LOAD_LIBRARY_SEARCH_SYSTEM32,
        );
    }
    std::process::exit(windows::run());
}

#[cfg(not(windows))]
fn main() {
    eprintln!("the agent's test tool runs on Windows");
    std::process::exit(2);
}

#[cfg(windows)]
mod windows {
    use std::io::Write;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use bli_kvm_agent_protocol::{Answer, Call, Hello};
    use bli_kvm_target_agent::client::{Client, ClientError, summary};
    use bli_kvm_target_agent::names::describe;
    use bli_kvm_target_agent::{VERSION, source_commit, title};

    use crate::hid::{self, HidLink};
    use crate::reads;

    const PROGRAM: &str = "hardened-blikvm-agent-test";
    const HELLO_TIMEOUT: Duration = Duration::from_secs(2);
    const PING_TIMEOUT: Duration = Duration::from_secs(1);
    const WATCH_INTERVAL: Duration = Duration::from_millis(200);

    /// Says each line on the console and in the file beside the program.
    struct Log {
        file: Option<std::fs::File>,
        started: Instant,
    }

    impl Log {
        fn open() -> Self {
            let file = std::env::current_exe().ok().and_then(|program| {
                let path = program.with_file_name("hardened-blikvm-agent-test.txt");
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .ok()
            });
            Self {
                file,
                started: Instant::now(),
            }
        }

        fn line(&mut self, text: &str) {
            let line = format!("[{:9.3}] {text}", self.started.elapsed().as_secs_f64());
            println!("{line}");
            if let Some(file) = self.file.as_mut() {
                let _ = writeln!(file, "{line}");
            }
        }
    }

    fn milliseconds(duration: Duration) -> String {
        format!("{:.3} ms", duration.as_secs_f64() * 1000.0)
    }

    fn hello() -> Hello {
        Hello::new(PROGRAM, VERSION, source_commit()).with_system(reads::system())
    }

    /// The agent's reads, for the board's calls.
    fn answer(call: Call) -> Result<Answer, String> {
        match call {
            Call::Ping => Ok(Answer::Pong),
            Call::System => Ok(Answer::System(reads::system())),
            Call::Monitors => Ok(Answer::Monitors(reads::monitors())),
            Call::Keyboard => Ok(Answer::Keyboard(reads::keyboard())),
            _ => Err("the test tool reads only".into()),
        }
    }

    fn show(log: &mut Log) -> i32 {
        for line in describe(&reads::system(), &reads::monitors(), &reads::keyboard()).lines() {
            log.line(line);
        }
        0
    }

    /// Finds, opens and greets the board.
    fn connect(log: &mut Log, quiet: bool) -> Option<Client<HidLink>> {
        let path = match hid::find() {
            Ok(Some(path)) => path,
            Ok(None) => {
                if !quiet {
                    log.line(&format!(
                        "the board's agent collection is not here (VID {:04x}, PID {:04x}, usage page ff00): is the board's \"Target agent\" switch on?",
                        hid::VENDOR_ID,
                        hid::PRODUCT_ID
                    ));
                }
                return None;
            }
            Err(error) => {
                if !quiet {
                    log.line(&format!("the HID collections could not be listed: {error}"));
                }
                return None;
            }
        };
        let link = match HidLink::open(&path) {
            Ok(link) => link,
            Err(error) => {
                if !quiet {
                    log.line(&format!(
                        "the agent collection could not be opened: {error}"
                    ));
                }
                return None;
            }
        };
        let mut client = Client::new(link, hello()).with_answerer(Box::new(answer));
        match client.hello(HELLO_TIMEOUT) {
            Ok(board) => {
                log.line(&format!(
                    "hello: {} {} ({}), protocol {}",
                    board.program, board.version, board.commit, board.protocol
                ));
                Some(client)
            }
            Err(error) => {
                if !quiet {
                    log.line(&format!("no hello: {error}"));
                }
                None
            }
        }
    }

    fn pings(log: &mut Log, count: usize) -> i32 {
        let Some(mut client) = connect(log, false) else {
            return 1;
        };
        let mut round_trips = Vec::with_capacity(count);
        let mut lost = 0_usize;
        for _ in 0..count {
            match client.ping(PING_TIMEOUT) {
                Ok(round_trip) => round_trips.push(round_trip),
                Err(error) => {
                    lost += 1;
                    if lost <= 5 {
                        log.line(&format!("a ping was not answered: {error}"));
                    }
                    if matches!(error, ClientError::Io(_)) {
                        break;
                    }
                }
            }
        }
        match summary(&round_trips) {
            Some(run) => log.line(&format!(
                "pings: {} answered of {count}, {lost} lost, {} dropped; round trip min {}, median {}, 95th percentile {}, max {}",
                run.count,
                client.dropped,
                milliseconds(run.min),
                milliseconds(run.median),
                milliseconds(run.p95),
                milliseconds(run.max)
            )),
            None => log.line(&format!("pings: none of {count} answered")),
        }
        if lost == 0 { 0 } else { 1 }
    }

    fn watch(log: &mut Log, seconds: u64) -> i32 {
        let until = Instant::now() + Duration::from_secs(seconds);
        log.line(&format!(
            "watching for {seconds} s: a ping every {} ms",
            WATCH_INTERVAL.as_millis()
        ));
        let mut client: Option<Client<HidLink>> = None;
        let mut lost_at: Option<Instant> = None;
        let mut gaps: Vec<Duration> = Vec::new();
        let mut answered = 0_u64;
        while Instant::now() < until {
            match client.as_mut() {
                None => {
                    client = connect(log, lost_at.is_some());
                    if client.is_some()
                        && let Some(lost) = lost_at.take()
                    {
                        let gap = lost.elapsed();
                        log.line(&format!("back after {:.2} s", gap.as_secs_f64()));
                        gaps.push(gap);
                    } else if client.is_none() {
                        lost_at.get_or_insert_with(Instant::now);
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
                Some(open) => match open.ping(PING_TIMEOUT) {
                    Ok(_) => {
                        answered += 1;
                        std::thread::sleep(WATCH_INTERVAL);
                    }
                    Err(error) => {
                        log.line(&format!("lost: {error}"));
                        lost_at = Some(Instant::now());
                        client = None;
                    }
                },
            }
        }
        let longest = gaps.iter().max().copied().unwrap_or_default();
        log.line(&format!(
            "watched: {answered} pings answered, the channel went {} times, longest away {:.2} s{}",
            gaps.len(),
            longest.as_secs_f64(),
            if lost_at.is_some() {
                ", and is away at the end"
            } else {
                ""
            }
        ));
        0
    }

    /// Whether this program has the console to itself (it was started by a
    /// double click), so the window stays until the user has read it.
    fn owns_console() -> bool {
        let mut processes = [0_u32; 4];
        // SAFETY: the buffer's length is passed with it.
        let count = unsafe {
            windows_sys::Win32::System::Console::GetConsoleProcessList(
                processes.as_mut_ptr(),
                processes.len() as u32,
            )
        };
        count == 1
    }

    pub fn run() -> i32 {
        reads::aware_of_each_monitors_scale();
        let arguments: Vec<String> = std::env::args().skip(1).collect();
        let mut log = Log::open();
        let started = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|now| now.as_secs())
            .unwrap_or(0);
        log.line(&format!(
            "{} test tool, started at {started} (Unix time)",
            title()
        ));
        let code = match arguments.iter().map(String::as_str).collect::<Vec<_>>()[..] {
            [] => pings(&mut log, 1000),
            ["show"] => show(&mut log),
            ["watch"] => watch(&mut log, 300),
            ["watch", seconds] => match seconds.parse() {
                Ok(seconds) => watch(&mut log, seconds),
                Err(_) => usage(&mut log),
            },
            [count] => match count.parse() {
                Ok(count) if count > 0 => pings(&mut log, count),
                _ => usage(&mut log),
            },
            _ => usage(&mut log),
        };
        if owns_console() {
            println!("Press Enter to close.");
            let _ = std::io::stdin().read_line(&mut String::new());
        }
        code
    }

    fn usage(log: &mut Log) -> i32 {
        log.line("usage: hardened-blikvm-agent-test.exe [PINGS] | watch [SECONDS] | show");
        2
    }
}
