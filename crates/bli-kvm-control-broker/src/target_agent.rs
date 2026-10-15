//! The target agent's channel, kept by the board owner
//! (`docs/baselines/work-plan-20261014-target-agent.md`).
//!
//! While the operator's switch is on, the absolute mouse's interface carries
//! the agent's collection (report 2, `bli_kvm_usb_gadget::AGENT_REPORT_ID`)
//! and its device, `/dev/hidg1`, carries the agent's reports both ways. One
//! task owns that device: it reads every report the target sends (an OUT
//! endpoint that nobody reads stops taking reports, and the agent's writes
//! would hang), answers the agent's hello and pings, and carries the
//! owner's calls to the agent with a deadline.
//!
//! The kernel's HID function (Linux 5.19) removes its device when the gadget
//! is unbound and makes it again when it is bound, and resets the device's
//! wait queues as it does: a descriptor kept open, or a wait on it, across a
//! virtual media swap or the agent's own switch is not safe. Every change of
//! the gadget therefore pauses the channel first, which closes the device,
//! and resumes it after, which opens it again; the agent on the target sees
//! the device go and come back, and says hello again.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bli_kvm_agent_protocol::{
    Answer, Call, Hello, Message, REPORT_BYTES, ReadError, Reader, Writer,
};
use tokio::io::unix::AsyncFd;
use tokio::sync::{mpsc, oneshot};

/// The operator's "Target agent" switch, kept by the owner in its state
/// directory: off unless the file says `enabled=yes`. The HID bind before
/// the owner starts reads it too, so a boot builds the gadget as it says.
pub const TARGET_AGENT_STATE: &str = "/var/lib/hardened-blikvm-gadget/target-agent.env";

/// The target agent's drive in the slot this owner runs from: the slot's
/// web tree carries it (`scripts/build-agent-drive.sh`), as a signed slot
/// carries nothing else beside its programs. None when this slot has none.
pub fn agent_drive_in_slot() -> Option<PathBuf> {
    let program = std::env::current_exe().ok()?;
    let drive = program
        .parent()?
        .parent()?
        .join("share/web/agent/agent-drive.img");
    drive.is_file().then_some(drive)
}

/// The drive to present: while the agent's switch is on and the gadget is
/// not in HID-only mode, which presents no storage.
pub fn wanted_drive(switch: bool, hid_only: bool, drive: Option<PathBuf>) -> Option<PathBuf> {
    (switch && !hid_only).then_some(drive).flatten()
}

/// The drive to present when the owner starts, or the HID bind before it:
/// the switch's and HID-only mode's files as they are.
pub fn drive_at_start(switch: bool) -> Option<PathBuf> {
    let hid_only = std::fs::read_to_string(crate::device_settings::HID_ONLY_STATE)
        .map(|text| crate::device_settings::parse_flag(&text))
        .unwrap_or(false);
    wanted_drive(switch, hid_only, agent_drive_in_slot())
}

/// Whether the switch file says the agent is on.
pub fn switch_enabled(path: &std::path::Path) -> bool {
    std::fs::read_to_string(path)
        .map(|text| crate::device_settings::parse_flag(&text))
        .unwrap_or(false)
}

/// The switch as the owner keeps it.
pub struct AgentSwitch {
    path: PathBuf,
    enabled: bool,
}

impl AgentSwitch {
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let enabled = switch_enabled(&path);
        Self { path, enabled }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Writes the switch; it stays as it was when the file cannot be
    /// written.
    pub fn set(&mut self, enabled: bool) -> Result<(), String> {
        crate::device_settings::write_flag(&self.path, enabled)?;
        self.enabled = enabled;
        Ok(())
    }
}

/// The program the board's hello names.
pub const BOARD_PROGRAM: &str = "hardened-blikvm-board";
/// How long the agent may be silent and still count as connected; the agent
/// pings more often than this.
pub const SILENCE: Duration = Duration::from_secs(10);
/// The longest a call may wait for its answer.
pub const MAX_CALL_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a report may wait for the target to take it before the
/// messages waiting are dropped (a target asleep takes none).
const WRITE_TIMEOUT: Duration = Duration::from_secs(1);
/// How soon a device that could not be opened is tried again.
const REOPEN_AFTER: Duration = Duration::from_millis(200);
/// The most messages waiting to be written.
const MAX_QUEUED_MESSAGES: usize = 64;
/// The longest problem the state reports.
const MAX_PROBLEM: usize = 160;

/// The board's hello: its program, version and, when the build says it, its
/// commit.
pub fn board_hello() -> Hello {
    let commit = match option_env!("BLI_KVM_SOURCE_COMMIT") {
        Some(commit) if !commit.is_empty() => commit,
        _ => "unknown",
    };
    Hello::new(BOARD_PROGRAM, env!("CARGO_PKG_VERSION"), commit)
}

/// What the owner tells the server about the channel.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentSnapshot {
    /// The channel should be open: the switch is on and the gadget carries
    /// the agent's collection.
    pub wanted: bool,
    /// The device is open now.
    pub open: bool,
    /// Why the device is not open while it should be.
    pub problem: Option<String>,
    /// The connected agent's hello.
    pub agent: Option<Hello>,
    /// Why the agent's last hello was refused.
    pub refused: Option<String>,
    pub messages_in: u64,
    pub messages_out: u64,
    /// Reports and messages dropped: not frames, out of order, not the
    /// protocol's, or not taken by the target in time.
    pub dropped: u64,
}

/// Why a call has no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallError {
    /// No agent has said hello.
    NotConnected,
    /// The agent did not answer in time.
    TimedOut,
    /// The agent said why it did not do it.
    Failed(String),
    /// The channel is gone (the owner is stopping).
    Closed,
}

/// The agent's side of a session: its hello, when it was last heard, and the
/// owner's calls waiting for answers. It writes nothing itself; what it
/// returns is what to send.
pub struct Session {
    board: Hello,
    agent: Option<Hello>,
    refused: Option<String>,
    last_heard: Option<Instant>,
    next_id: u32,
    pending: HashMap<u32, (Call, oneshot::Sender<Result<Answer, CallError>>)>,
}

impl Session {
    pub fn new(board: Hello) -> Self {
        Self {
            board,
            agent: None,
            refused: None,
            last_heard: None,
            next_id: 1,
            pending: HashMap::new(),
        }
    }

    /// The connected agent: one whose hello was taken and that was heard
    /// within [`SILENCE`].
    pub fn agent(&self, now: Instant) -> Option<&Hello> {
        let heard = self.last_heard?;
        (now.saturating_duration_since(heard) <= SILENCE)
            .then_some(self.agent.as_ref())
            .flatten()
    }

    pub fn refused(&self) -> Option<&str> {
        self.refused.as_deref()
    }

    /// Takes one message from the agent; returns the replies.
    pub fn receive(&mut self, message: Message, now: Instant) -> Vec<Message> {
        self.last_heard = Some(now);
        match message {
            Message::Hello(hello) => match hello.refusal() {
                Some(reason) => {
                    self.agent = None;
                    self.refused = Some(reason.clone());
                    self.fail_pending(CallError::NotConnected);
                    vec![Message::Refused { reason }]
                }
                None => {
                    // A new hello is a new session: the old one's calls end.
                    self.fail_pending(CallError::NotConnected);
                    self.agent = Some(hello);
                    self.refused = None;
                    vec![Message::Hello(self.board.clone())]
                }
            },
            Message::Request { id, call } if self.agent.is_some() => match call {
                Call::Ping => vec![Message::Answer {
                    id,
                    answer: Answer::Pong,
                }],
                // The board reads and does nothing of its own for the agent.
                _ => vec![Message::Failed {
                    id,
                    reason: "the board answers pings only".into(),
                }],
            },
            Message::Request { id, .. } => vec![Message::Failed {
                id,
                reason: "hello first".into(),
            }],
            Message::Answer { id, answer } => {
                if let Some((call, reply)) = self.pending.remove(&id) {
                    // An answer to another call, or out of its bounds, is
                    // not carried further.
                    let _ = reply.send(if answer.answers(&call) {
                        Ok(answer)
                    } else {
                        Err(CallError::Failed(
                            "the agent's answer is not the call's or is out of its bounds".into(),
                        ))
                    });
                }
                Vec::new()
            }
            Message::Failed { id, reason } => {
                if let Some((_, reply)) = self.pending.remove(&id) {
                    let reason: String = reason
                        .chars()
                        .filter(|c| !c.is_control())
                        .take(160)
                        .collect();
                    let _ = reply.send(Err(CallError::Failed(reason)));
                }
                Vec::new()
            }
            Message::Refused { reason } => {
                self.agent = None;
                self.refused = Some(reason);
                self.fail_pending(CallError::NotConnected);
                Vec::new()
            }
        }
    }

    /// A call to the connected agent: the request to send, or why there is
    /// none (the reply is then already answered).
    pub fn call(
        &mut self,
        call: Call,
        reply: oneshot::Sender<Result<Answer, CallError>>,
        now: Instant,
    ) -> Option<Message> {
        self.pending.retain(|_, (_, waiting)| !waiting.is_closed());
        if self.agent(now).is_none() {
            let _ = reply.send(Err(CallError::NotConnected));
            return None;
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.pending.insert(id, (call.clone(), reply));
        Some(Message::Request { id, call })
    }

    /// The device went away: the agent with it.
    pub fn reset(&mut self) {
        self.agent = None;
        self.last_heard = None;
        self.fail_pending(CallError::NotConnected);
    }

    fn fail_pending(&mut self, error: CallError) {
        for (_, (_, reply)) in self.pending.drain() {
            let _ = reply.send(Err(error.clone()));
        }
    }
}

enum Command {
    /// Whether the channel should be open (the switch and the gadget); says
    /// when the channel has acted on it.
    Want(bool, oneshot::Sender<()>),
    /// Close the device until resumed, and say when it is closed.
    Pause(oneshot::Sender<()>),
    Resume,
    Call {
        call: Call,
        reply: oneshot::Sender<Result<Answer, CallError>>,
    },
}

/// Opens the channel's device: the agent's `/dev/hidg1` on the board.
type Opener = Box<dyn FnMut() -> Result<std::fs::File, String> + Send>;

/// The owner's handle on the channel's task.
#[derive(Clone)]
pub struct AgentChannel {
    commands: mpsc::Sender<Command>,
    snapshot: Arc<Mutex<AgentSnapshot>>,
}

impl AgentChannel {
    /// Starts the task on the current runtime; it opens `device` once the
    /// channel is wanted, and ends when every handle is gone.
    pub fn start(device: PathBuf, board: Hello) -> Self {
        Self::start_with(Box::new(move || open_device(&device)), board)
    }

    fn start_with(opener: Opener, board: Hello) -> Self {
        let (commands, receiver) = mpsc::channel(32);
        let snapshot = Arc::new(Mutex::new(AgentSnapshot::default()));
        tokio::spawn(run(opener, board, receiver, snapshot.clone()));
        Self { commands, snapshot }
    }

    pub fn snapshot(&self) -> AgentSnapshot {
        self.snapshot
            .lock()
            .map(|snapshot| snapshot.clone())
            .unwrap_or_default()
    }

    /// Whether the channel should be open; returns once the channel has
    /// opened or closed its device as wanted (or the task is gone).
    pub async fn want(&self, wanted: bool) {
        let (done, acted) = oneshot::channel();
        if self
            .commands
            .send(Command::Want(wanted, done))
            .await
            .is_ok()
        {
            let _ = acted.await;
        }
    }

    /// Closes the device before the gadget changes; returns once it is
    /// closed (or the task is gone).
    pub async fn pause(&self) {
        let (done, closed) = oneshot::channel();
        if self.commands.send(Command::Pause(done)).await.is_ok() {
            let _ = closed.await;
        }
    }

    /// Opens the device again after the gadget changed.
    pub async fn resume(&self) {
        let _ = self.commands.send(Command::Resume).await;
    }

    /// Calls the connected agent and waits at most `timeout` (bounded by
    /// [`MAX_CALL_TIMEOUT`]) for its answer.
    pub async fn call(&self, call: Call, timeout: Duration) -> Result<Answer, CallError> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(Command::Call { call, reply })
            .await
            .map_err(|_| CallError::Closed)?;
        match tokio::time::timeout(timeout.min(MAX_CALL_TIMEOUT), answer).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(CallError::Closed),
            Err(_) => Err(CallError::TimedOut),
        }
    }
}

/// What the task keeps between its turns.
struct Channel {
    opener: Opener,
    device: Option<AsyncFd<std::fs::File>>,
    wanted: bool,
    paused: usize,
    retry_at: Option<tokio::time::Instant>,
    session: Session,
    reader: Reader,
    writer: Writer,
    /// Reports waiting for the target, and when the first of them was due.
    queue: VecDeque<[u8; REPORT_BYTES]>,
    queued_since: Option<Instant>,
    snapshot: Arc<Mutex<AgentSnapshot>>,
    counters: (u64, u64, u64),
    problem: Option<String>,
}

async fn run(
    opener: Opener,
    board: Hello,
    mut commands: mpsc::Receiver<Command>,
    snapshot: Arc<Mutex<AgentSnapshot>>,
) {
    let mut channel = Channel {
        opener,
        device: None,
        wanted: false,
        paused: 0,
        retry_at: None,
        session: Session::new(board),
        reader: Reader::new(),
        writer: Writer::new(),
        queue: VecDeque::new(),
        queued_since: None,
        snapshot,
        counters: (0, 0, 0),
        problem: None,
    };
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        channel.settle();
        channel.publish();
        let retry_at = channel.retry_at;
        let reading = channel.device.as_ref();
        let writing = channel
            .device
            .as_ref()
            .filter(|_| !channel.queue.is_empty());
        tokio::select! {
            command = commands.recv() => match command {
                None => break,
                Some(command) => channel.command(command),
            },
            ready = async { reading.unwrap().readable().await }, if reading.is_some() => {
                match ready {
                    Ok(mut guard) => {
                        let mut buffer = [0_u8; REPORT_BYTES + 1];
                        match guard.try_io(|device| device.get_ref().read(&mut buffer)) {
                            Ok(Ok(length)) => channel.report(&buffer[..length]),
                            Ok(Err(error)) => channel.fail(format!("the agent's device could not be read: {error}")),
                            Err(_would_block) => {}
                        }
                    }
                    Err(error) => channel.fail(format!("the agent's device could not be watched: {error}")),
                }
            },
            ready = async { writing.unwrap().writable().await }, if writing.is_some() => {
                match ready {
                    Ok(mut guard) => {
                        let report = channel.queue[0];
                        match guard.try_io(|device| device.get_ref().write(&report)) {
                            Ok(Ok(_)) => channel.written(),
                            Ok(Err(error)) => channel.fail(format!("the agent's device could not be written: {error}")),
                            Err(_would_block) => {}
                        }
                    }
                    Err(error) => channel.fail(format!("the agent's device could not be watched: {error}")),
                }
            },
            _ = async { tokio::time::sleep_until(retry_at.unwrap()).await }, if retry_at.is_some() => {
                channel.retry_at = None;
            },
            _ = tick.tick() => channel.expire(),
        }
    }
}

impl Channel {
    fn command(&mut self, command: Command) {
        match command {
            Command::Want(wanted, done) => {
                self.wanted = wanted;
                self.retry_at = None;
                if !wanted {
                    self.problem = None;
                }
                self.settle();
                self.publish();
                let _ = done.send(());
            }
            Command::Pause(done) => {
                self.paused += 1;
                // The gadget's change may leave the device where it was:
                // the agent is kept, and a new hello replaces it if not.
                self.close(false);
                let _ = done.send(());
            }
            Command::Resume => {
                self.paused = self.paused.saturating_sub(1);
                self.retry_at = None;
            }
            Command::Call { call, reply } => {
                if let Some(request) = self.session.call(call, reply, Instant::now()) {
                    self.send(&request);
                }
            }
        }
    }

    /// Opens or closes the device as wanted.
    fn settle(&mut self) {
        let open = self.wanted && self.paused == 0;
        if !open {
            self.close(!self.wanted);
            return;
        }
        if self.device.is_some() || self.retry_at.is_some() {
            return;
        }
        match (self.opener)().and_then(|file| {
            AsyncFd::new(file)
                .map_err(|error| format!("the agent's device could not be watched: {error}"))
        }) {
            Ok(device) => {
                self.device = Some(device);
                self.problem = None;
            }
            Err(problem) => {
                self.problem = Some(problem);
                self.retry_at = Some(tokio::time::Instant::now() + REOPEN_AFTER);
            }
        }
    }

    /// Closes the device; `forget` ends the agent's session too (the
    /// switch is off, or the device failed).
    fn close(&mut self, forget: bool) {
        self.device.take();
        if forget {
            self.session.reset();
        }
        self.reader = Reader::new();
        self.drop_queue();
    }

    fn fail(&mut self, problem: String) {
        self.close(true);
        self.problem = Some(problem.chars().take(MAX_PROBLEM).collect());
        self.retry_at = Some(tokio::time::Instant::now() + REOPEN_AFTER);
    }

    fn report(&mut self, report: &[u8]) {
        match self.reader.push(report) {
            Ok(None) => {}
            Ok(Some(message)) => {
                self.counters.0 += 1;
                for reply in self.session.receive(message, Instant::now()) {
                    self.send(&reply);
                }
            }
            Err(ReadError::Dropped(_) | ReadError::Undecodable(_)) => self.counters.2 += 1,
        }
    }

    fn send(&mut self, message: &Message) {
        if self.device.is_none() || self.queue.len() >= MAX_QUEUED_MESSAGES * 4 {
            self.counters.2 += 1;
            return;
        }
        match self.writer.reports(message) {
            Ok(reports) => {
                if self.queue.is_empty() {
                    self.queued_since = Some(Instant::now());
                }
                self.queue.extend(reports);
                self.counters.1 += 1;
            }
            Err(_) => self.counters.2 += 1,
        }
    }

    fn written(&mut self) {
        self.queue.pop_front();
        self.queued_since = (!self.queue.is_empty()).then(Instant::now);
    }

    /// A target that takes no report loses what waits for it.
    fn expire(&mut self) {
        if self
            .queued_since
            .is_some_and(|since| since.elapsed() > WRITE_TIMEOUT)
        {
            self.drop_queue();
        }
    }

    fn drop_queue(&mut self) {
        if !self.queue.is_empty() {
            self.counters.2 += 1;
        }
        self.queue.clear();
        self.queued_since = None;
    }

    fn publish(&self) {
        let now = Instant::now();
        if let Ok(mut snapshot) = self.snapshot.lock() {
            *snapshot = AgentSnapshot {
                wanted: self.wanted,
                open: self.device.is_some(),
                problem: self.problem.clone().filter(|_| self.wanted),
                agent: self.session.agent(now).cloned(),
                refused: self.session.refused().map(str::to_owned),
                messages_in: self.counters.0,
                messages_out: self.counters.1,
                dropped: self.counters.2,
            };
        }
    }
}

fn open_device(path: &std::path::Path) -> Result<std::fs::File, String> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOCTTY)
        .open(path)
        .map_err(|error| match error.raw_os_error() {
            Some(libc::EPERM) | Some(libc::EACCES) => format!(
                "the board's system does not let the owner open {}",
                path.display()
            ),
            _ => format!("{} could not be opened: {error}", path.display()),
        })?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("{} could not be read: {error}", path.display()))?;
    use std::os::unix::fs::FileTypeExt;
    if !metadata.file_type().is_char_device() {
        return Err(format!("{} is not a device", path.display()));
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent_hello() -> Hello {
        Hello::new("hardened-blikvm-agent", "0.1.0", "3c940a7e681c")
    }

    fn ping(id: u32) -> Message {
        Message::Request {
            id,
            call: Call::Ping,
        }
    }

    #[test]
    fn a_hello_is_answered_with_the_boards_and_pings_with_pongs() {
        let mut session = Session::new(board_hello());
        let now = Instant::now();
        // Before the hello a request is refused, and asks for one.
        assert_eq!(
            session.receive(ping(1), now),
            vec![Message::Failed {
                id: 1,
                reason: "hello first".into()
            }]
        );
        assert_eq!(
            session.receive(Message::Hello(agent_hello()), now),
            vec![Message::Hello(board_hello())]
        );
        assert_eq!(session.agent(now), Some(&agent_hello()));
        assert_eq!(
            session.receive(ping(2), now),
            vec![Message::Answer {
                id: 2,
                answer: Answer::Pong
            }]
        );
        // Silent for longer than allowed: not connected any more.
        assert_eq!(session.agent(now + SILENCE + Duration::from_secs(1)), None);
    }

    #[tokio::test]
    async fn the_board_answers_only_pings_and_takes_only_answers_that_fit_the_call() {
        let mut session = Session::new(board_hello());
        let now = Instant::now();
        session.receive(Message::Hello(agent_hello()), now);
        assert_eq!(
            session.receive(
                Message::Request {
                    id: 4,
                    call: Call::Monitors
                },
                now
            ),
            vec![Message::Failed {
                id: 4,
                reason: "the board answers pings only".into()
            }]
        );
        let (reply, answer) = oneshot::channel();
        let Some(Message::Request { id, .. }) = session.call(Call::Keyboard, reply, now) else {
            panic!("no request");
        };
        // A pong to a keyboard call is not the call's answer.
        session.receive(
            Message::Answer {
                id,
                answer: Answer::Pong,
            },
            now,
        );
        assert!(matches!(answer.await.unwrap(), Err(CallError::Failed(_))));
        let (reply, answer) = oneshot::channel();
        let Some(Message::Request { id, .. }) = session.call(Call::Keyboard, reply, now) else {
            panic!("no request");
        };
        let keyboard = bli_kvm_agent_protocol::Keyboard {
            current: None,
            installed: Vec::new(),
        };
        session.receive(
            Message::Answer {
                id,
                answer: Answer::Keyboard(keyboard.clone()),
            },
            now,
        );
        assert_eq!(answer.await.unwrap(), Ok(Answer::Keyboard(keyboard)));
    }

    #[test]
    fn a_hello_of_another_protocol_is_refused_and_kept_as_the_reason() {
        let mut session = Session::new(board_hello());
        let now = Instant::now();
        let mut newer = agent_hello();
        newer.protocol = 2;
        let replies = session.receive(Message::Hello(newer), now);
        assert!(
            matches!(&replies[..], [Message::Refused { reason }] if reason.contains("protocol 2"))
        );
        assert_eq!(session.agent(now), None);
        assert!(session.refused().unwrap().contains("protocol 2"));
        // A good hello afterwards is taken.
        session.receive(Message::Hello(agent_hello()), now);
        assert!(session.agent(now).is_some());
        assert_eq!(session.refused(), None);
    }

    #[tokio::test]
    async fn calls_are_answered_failed_or_ended_with_the_session() {
        let mut session = Session::new(board_hello());
        let now = Instant::now();
        let (reply, answer) = oneshot::channel();
        assert_eq!(session.call(Call::Ping, reply, now), None);
        assert_eq!(answer.await.unwrap(), Err(CallError::NotConnected));

        session.receive(Message::Hello(agent_hello()), now);
        let (reply, answer) = oneshot::channel();
        let Some(Message::Request { id, call }) = session.call(Call::Ping, reply, now) else {
            panic!("no request");
        };
        assert_eq!(call, Call::Ping);
        session.receive(
            Message::Answer {
                id,
                answer: Answer::Pong,
            },
            now,
        );
        assert_eq!(answer.await.unwrap(), Ok(Answer::Pong));

        let (reply, answer) = oneshot::channel();
        let Some(Message::Request { id, .. }) = session.call(Call::Ping, reply, now) else {
            panic!("no request");
        };
        session.receive(
            Message::Failed {
                id,
                reason: "busy".into(),
            },
            now,
        );
        assert_eq!(answer.await.unwrap(), Err(CallError::Failed("busy".into())));

        // A call waiting when the device goes, or a new hello comes, ends.
        let (reply, answer) = oneshot::channel();
        assert!(session.call(Call::Ping, reply, now).is_some());
        session.reset();
        assert_eq!(answer.await.unwrap(), Err(CallError::NotConnected));
        session.receive(Message::Hello(agent_hello()), now);
        let (reply, answer) = oneshot::channel();
        assert!(session.call(Call::Ping, reply, now).is_some());
        session.receive(Message::Hello(agent_hello()), now);
        assert_eq!(answer.await.unwrap(), Err(CallError::NotConnected));
    }

    /// The target's end of a device that keeps each report whole: one end
    /// of a datagram socket pair, the other given to the channel once.
    fn device() -> (Opener, tokio::net::UnixDatagram) {
        let (board, target) = std::os::unix::net::UnixDatagram::pair().unwrap();
        target.set_nonblocking(true).unwrap();
        // As the owner opens the real device: without blocking.
        board.set_nonblocking(true).unwrap();
        let mut board = Some(std::fs::File::from(std::os::fd::OwnedFd::from(board)));
        let opener: Opener =
            Box::new(move || board.take().ok_or_else(|| "the device is gone".to_string()));
        (opener, tokio::net::UnixDatagram::from_std(target).unwrap())
    }

    async fn send(target: &tokio::net::UnixDatagram, writer: &mut Writer, message: &Message) {
        for report in writer.reports(message).unwrap() {
            target.send(&report).await.unwrap();
        }
    }

    async fn receive(target: &tokio::net::UnixDatagram, reader: &mut Reader) -> Message {
        let mut buffer = [0_u8; 128];
        loop {
            let length = tokio::time::timeout(Duration::from_secs(5), target.recv(&mut buffer))
                .await
                .expect("the board answers")
                .unwrap();
            assert_eq!(length, REPORT_BYTES);
            if let Some(message) = reader.push(&buffer[..length]).unwrap() {
                return message;
            }
        }
    }

    #[tokio::test]
    async fn the_channel_answers_the_agent_and_carries_the_owners_calls() {
        let (opener, target) = device();
        let channel = AgentChannel::start_with(opener, board_hello());
        channel.want(true).await;
        let (mut writer, mut reader) = (Writer::new(), Reader::new());

        send(&target, &mut writer, &Message::Hello(agent_hello())).await;
        assert_eq!(
            receive(&target, &mut reader).await,
            Message::Hello(board_hello())
        );
        for id in 1..=20 {
            send(&target, &mut writer, &ping(id)).await;
            assert_eq!(
                receive(&target, &mut reader).await,
                Message::Answer {
                    id,
                    answer: Answer::Pong
                }
            );
        }
        // A report of another ID and a frame that is no frame are dropped,
        // and the channel goes on.
        target.send(&[1, 0, 0, 0, 0, 0, 0, 0]).await.unwrap();
        target.send(&[2; REPORT_BYTES]).await.unwrap();
        send(&target, &mut writer, &ping(21)).await;
        assert!(matches!(
            receive(&target, &mut reader).await,
            Message::Answer { id: 21, .. }
        ));
        let snapshot = channel.snapshot();
        assert!(snapshot.open);
        assert_eq!(snapshot.agent, Some(agent_hello()));
        assert_eq!((snapshot.messages_in, snapshot.messages_out), (22, 22));
        assert_eq!(snapshot.dropped, 2);

        // The owner calls the agent; the agent answers.
        let call = tokio::spawn({
            let channel = channel.clone();
            async move { channel.call(Call::Ping, Duration::from_secs(5)).await }
        });
        let Message::Request {
            id,
            call: Call::Ping,
        } = receive(&target, &mut reader).await
        else {
            panic!("no request");
        };
        send(
            &target,
            &mut writer,
            &Message::Answer {
                id,
                answer: Answer::Pong,
            },
        )
        .await;
        assert_eq!(call.await.unwrap(), Ok(Answer::Pong));
        // An agent that does not answer: the call times out.
        assert_eq!(
            channel.call(Call::Ping, Duration::from_millis(100)).await,
            Err(CallError::TimedOut)
        );

        // A pause closes the device and keeps the agent: a gadget's change
        // may leave the device where it was. The device that cannot be
        // opened again is reported.
        channel.pause().await;
        let snapshot = channel.snapshot();
        assert!(!snapshot.open);
        assert_eq!(snapshot.agent, Some(agent_hello()));
        channel.resume().await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            channel.snapshot().problem.as_deref(),
            Some("the device is gone")
        );
    }

    #[test]
    fn the_switch_is_off_unless_its_file_says_on_and_keeps_what_it_is_set_to() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("target-agent.env");
        assert!(!switch_enabled(&path));
        let mut switch = AgentSwitch::open(&path);
        assert!(!switch.enabled());
        switch.set(true).unwrap();
        assert!(switch_enabled(&path));
        assert!(AgentSwitch::open(&path).enabled());
        std::fs::write(&path, "enabled=maybe\n").unwrap();
        assert!(!AgentSwitch::open(&path).enabled());
        // A switch that cannot be written stays as it was.
        let mut broken = AgentSwitch::open(directory.path().join("missing/target-agent.env"));
        assert!(broken.set(true).is_err());
        assert!(!broken.enabled());
    }

    #[test]
    fn the_drive_is_presented_only_with_the_switch_on_and_storage_allowed() {
        let drive = Some(PathBuf::from("/slot/share/web/agent/agent-drive.img"));
        assert_eq!(wanted_drive(true, false, drive.clone()), drive);
        assert_eq!(wanted_drive(false, false, drive.clone()), None);
        assert_eq!(wanted_drive(true, true, drive.clone()), None);
        assert_eq!(wanted_drive(true, false, None), None);
    }

    #[tokio::test]
    async fn the_channel_reports_why_its_device_cannot_be_opened() {
        let directory = tempfile::tempdir().unwrap();
        let channel = AgentChannel::start(directory.path().join("hidg1"), board_hello());
        assert!(!channel.snapshot().wanted);
        channel.want(true).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        let snapshot = channel.snapshot();
        assert!(snapshot.wanted && !snapshot.open);
        assert!(snapshot.problem.unwrap().contains("could not be opened"));
        // Paused, it stays closed; calls without an agent are not connected.
        channel.pause().await;
        assert_eq!(
            channel.call(Call::Ping, Duration::from_millis(100)).await,
            Err(CallError::NotConnected)
        );
        channel.want(false).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(channel.snapshot().problem, None);
    }
}
