//! The agent's side of the channel, over any link that carries the board's
//! reports: the HID collection on Windows, a played board in the tests.

use std::io;
use std::time::{Duration, Instant};

use bli_kvm_agent_protocol::{
    Answer, Call, Hello, Message, REPORT_BYTES, ReadError, Reader, Writer,
};

/// One report each way, the report ID first.
pub type Report = [u8; REPORT_BYTES];

/// What carries the reports.
pub trait Link {
    fn send(&mut self, report: &Report) -> io::Result<()>;
    /// The next report, or none within `timeout`.
    fn receive(&mut self, timeout: Duration) -> io::Result<Option<Report>>;
}

#[derive(Debug)]
pub enum ClientError {
    Io(io::Error),
    /// Nothing came in time.
    TimedOut,
    /// The board refused the hello, and why.
    Refused(String),
    /// The board did not do the call, and why.
    Failed(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::TimedOut => write!(formatter, "the board did not answer in time"),
            Self::Refused(reason) => write!(formatter, "the board refused the hello: {reason}"),
            Self::Failed(reason) => write!(formatter, "the board did not do it: {reason}"),
        }
    }
}

impl From<io::Error> for ClientError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// The agent's session with the board.
pub struct Client<L: Link> {
    link: L,
    hello: Hello,
    writer: Writer,
    reader: Reader,
    next_id: u32,
    /// Reports that were not frames of a message, or messages that were not
    /// the protocol's.
    pub dropped: u64,
}

impl<L: Link> Client<L> {
    pub fn new(link: L, hello: Hello) -> Self {
        Self {
            link,
            hello,
            writer: Writer::new(),
            reader: Reader::new(),
            next_id: 1,
            dropped: 0,
        }
    }

    fn send(&mut self, message: &Message) -> Result<(), ClientError> {
        let reports = self
            .writer
            .reports(message)
            .map_err(|error| ClientError::Io(io::Error::other(error.to_string())))?;
        for report in &reports {
            self.link.send(report)?;
        }
        Ok(())
    }

    /// Waits until `deadline` for a message `wanted` takes, answering the
    /// board's pings meanwhile.
    fn wait_for<T>(
        &mut self,
        deadline: Instant,
        mut wanted: impl FnMut(Message) -> Option<Result<T, ClientError>>,
    ) -> Result<T, ClientError> {
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(ClientError::TimedOut);
            }
            let Some(report) = self.link.receive(left)? else {
                continue;
            };
            let message = match self.reader.push(&report) {
                Ok(Some(message)) => message,
                Ok(None) => continue,
                Err(ReadError::Dropped(_) | ReadError::Undecodable(_)) => {
                    self.dropped += 1;
                    continue;
                }
            };
            if let Message::Request {
                id,
                call: Call::Ping,
            } = message
            {
                self.send(&Message::Answer {
                    id,
                    answer: Answer::Pong,
                })?;
                continue;
            }
            if let Some(result) = wanted(message) {
                return result;
            }
        }
    }

    /// Says hello and returns the board's.
    pub fn hello(&mut self, timeout: Duration) -> Result<Hello, ClientError> {
        let deadline = Instant::now() + timeout;
        self.send(&Message::Hello(self.hello.clone()))?;
        self.wait_for(deadline, |message| match message {
            Message::Hello(board) => Some(match board.refusal() {
                None => Ok(board),
                Some(reason) => Err(ClientError::Refused(reason)),
            }),
            Message::Refused { reason } => Some(Err(ClientError::Refused(reason))),
            _ => None,
        })
    }

    /// One ping and its round trip.
    pub fn ping(&mut self, timeout: Duration) -> Result<Duration, ClientError> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let started = Instant::now();
        self.send(&Message::Request {
            id,
            call: Call::Ping,
        })?;
        self.wait_for(started + timeout, |message| match message {
            Message::Answer {
                id: answered,
                answer: Answer::Pong,
            } if answered == id => Some(Ok(started.elapsed())),
            Message::Failed {
                id: answered,
                reason,
            } if answered == id => Some(Err(ClientError::Failed(reason))),
            _ => None,
        })
    }

    pub fn into_link(self) -> L {
        self.link
    }
}

/// The round trips of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub count: usize,
    pub min: Duration,
    pub median: Duration,
    pub p95: Duration,
    pub max: Duration,
}

/// The run's round trips, by the nearest rank.
pub fn summary(round_trips: &[Duration]) -> Option<Summary> {
    if round_trips.is_empty() {
        return None;
    }
    let mut sorted = round_trips.to_vec();
    sorted.sort();
    let rank = |percent: usize| sorted[(percent * sorted.len()).div_ceil(100).max(1) - 1];
    Some(Summary {
        count: sorted.len(),
        min: sorted[0],
        median: rank(50),
        p95: rank(95),
        max: sorted[sorted.len() - 1],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// A board that answers as the owner's session does, and can stay
    /// silent, refuse, or call the agent first.
    struct PlayedBoard {
        reader: Reader,
        writer: Writer,
        incoming: VecDeque<Report>,
        silent: bool,
        refuse: bool,
        call_first: bool,
        answered_calls: Vec<u32>,
    }

    impl PlayedBoard {
        fn new() -> Self {
            Self {
                reader: Reader::new(),
                writer: Writer::new(),
                incoming: VecDeque::new(),
                silent: false,
                refuse: false,
                call_first: false,
                answered_calls: Vec::new(),
            }
        }

        fn say(&mut self, message: &Message) {
            let reports = self.writer.reports(message).unwrap();
            self.incoming.extend(reports);
        }
    }

    impl Link for PlayedBoard {
        fn send(&mut self, report: &Report) -> io::Result<()> {
            let Some(message) = self.reader.push(report).unwrap() else {
                return Ok(());
            };
            if self.silent {
                return Ok(());
            }
            match message {
                Message::Hello(_) if self.refuse => self.say(&Message::Refused {
                    reason: "protocol 1 is not this side's 2".into(),
                }),
                Message::Hello(_) => {
                    // A report of another ID first: the agent drops it.
                    self.incoming.push_back([1; REPORT_BYTES]);
                    if self.call_first {
                        self.say(&Message::Request {
                            id: 77,
                            call: Call::Ping,
                        });
                    }
                    self.say(&Message::Hello(Hello::new(
                        "hardened-blikvm-board",
                        "0.1.0",
                        "unknown",
                    )));
                }
                Message::Request {
                    id,
                    call: Call::Ping,
                } => self.say(&Message::Answer {
                    id,
                    answer: Answer::Pong,
                }),
                Message::Answer { id, .. } => self.answered_calls.push(id),
                _ => {}
            }
            Ok(())
        }

        fn receive(&mut self, timeout: Duration) -> io::Result<Option<Report>> {
            match self.incoming.pop_front() {
                Some(report) => Ok(Some(report)),
                None => {
                    std::thread::sleep(timeout.min(Duration::from_millis(5)));
                    Ok(None)
                }
            }
        }
    }

    fn agent_hello() -> Hello {
        Hello::new("hardened-blikvm-agent-test", "0.1.0", "unknown")
    }

    #[test]
    fn a_hello_and_pings_come_back_and_the_boards_calls_are_answered() {
        let mut board = PlayedBoard::new();
        board.call_first = true;
        let mut client = Client::new(board, agent_hello());
        let hello = client.hello(Duration::from_secs(1)).unwrap();
        assert_eq!(hello.program, "hardened-blikvm-board");
        assert_eq!(client.dropped, 1);
        let round_trips: Vec<Duration> = (0..20)
            .map(|_| client.ping(Duration::from_secs(1)).unwrap())
            .collect();
        assert_eq!(summary(&round_trips).unwrap().count, 20);
        assert_eq!(client.into_link().answered_calls, vec![77]);
    }

    #[test]
    fn a_refusal_and_silence_are_said_as_they_are() {
        let mut board = PlayedBoard::new();
        board.refuse = true;
        let mut client = Client::new(board, agent_hello());
        assert!(matches!(
            client.hello(Duration::from_secs(1)),
            Err(ClientError::Refused(reason)) if reason.contains("protocol")
        ));
        let mut board = PlayedBoard::new();
        board.silent = true;
        let mut client = Client::new(board, agent_hello());
        assert!(matches!(
            client.ping(Duration::from_millis(30)),
            Err(ClientError::TimedOut)
        ));
    }

    #[test]
    fn the_summary_takes_the_nearest_rank() {
        let millis = |values: &[u64]| -> Vec<Duration> {
            values
                .iter()
                .map(|value| Duration::from_millis(*value))
                .collect()
        };
        assert_eq!(summary(&[]), None);
        let one = summary(&millis(&[7])).unwrap();
        assert_eq!(
            (one.min, one.median, one.p95, one.max),
            (
                millis(&[7])[0],
                millis(&[7])[0],
                millis(&[7])[0],
                millis(&[7])[0]
            )
        );
        let hundred: Vec<u64> = (1..=100).rev().collect();
        let run = summary(&millis(&hundred)).unwrap();
        assert_eq!(run.count, 100);
        assert_eq!(run.min, Duration::from_millis(1));
        assert_eq!(run.median, Duration::from_millis(50));
        assert_eq!(run.p95, Duration::from_millis(95));
        assert_eq!(run.max, Duration::from_millis(100));
    }
}
