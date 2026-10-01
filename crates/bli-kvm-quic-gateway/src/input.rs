//! A session's input stream: the page's requests of the HID lease API, passed
//! to the web server over the link in the order they came, each answered
//! with the server's answer.
//!
//! The gateway understands none of it beyond what it needs to stay bounded:
//! one request is with the server at a time, at most [`QUEUE`] wait behind
//! it, and a pointer movement that a newer one supersedes before it was sent
//! is merged into the newer. Keyboard reports, buttons and the wheel are
//! never merged and never dropped.

use std::{collections::VecDeque, future::Future, pin::Pin};

use bli_kvm_quic_protocol::page::{self, InputRequest, InputResponse, MessageReader};
use serde_json::{Value, json};
use wtransport::{RecvStream, SendStream};

use crate::link::Link;

/// Requests of one session read from its stream and not yet answered. When
/// this many wait, the stream is not read: the page is held by its window.
pub const QUEUE: usize = 32;
/// Requests one waiting entry may stand for.
const MERGED: usize = 64;
const READ_BUFFER: usize = 4096;

/// Why the stream is no longer served.
#[derive(Debug, PartialEq, Eq)]
pub enum Ended {
    /// The page closed it, or its connection is gone.
    Page,
    /// What came was not a request.
    Protocol,
}

/// What makes two reports of the pointer mergeable: the same lease, the same
/// buttons, no wheel.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Pointer {
    /// The newest position stands for the ones before it.
    Absolute { lease: String, buttons: u8 },
    /// Movements add up.
    Relative {
        lease: String,
        buttons: u8,
        x: i8,
        y: i8,
    },
}

impl Pointer {
    /// Reads a request as a pointer movement, if it is one: a `report` of the
    /// absolute or the relative mouse without the wheel.
    fn of(request: &Value) -> Option<Self> {
        if request.get("action")?.as_str()? != "report" {
            return None;
        }
        let lease = request.get("leaseId")?.as_str()?.to_owned();
        let report = request.get("report")?;
        let data: Vec<u8> = report
            .get("data")?
            .as_array()?
            .iter()
            .map(|byte| byte.as_u64().and_then(|byte| u8::try_from(byte).ok()))
            .collect::<Option<_>>()?;
        match (report.get("mode")?.as_str()?, data.as_slice()) {
            ("absolute_mouse", [buttons, _, _, _, _, 0]) => Some(Self::Absolute {
                lease,
                buttons: *buttons,
            }),
            ("relative_mouse", [buttons, x, y, 0]) => Some(Self::Relative {
                lease,
                buttons: *buttons,
                x: *x as i8,
                y: *y as i8,
            }),
            _ => None,
        }
    }
}

struct Entry {
    /// The requests this one stands for, oldest first.
    seqs: Vec<u32>,
    request: Value,
    pointer: Option<Pointer>,
}

/// The requests of a session that wait for the server, oldest first. The
/// first one may be with the server already.
#[derive(Default)]
struct Queue {
    entries: VecDeque<Entry>,
    asked: bool,
}

impl Queue {
    fn is_full(&self) -> bool {
        self.entries.len() >= QUEUE
    }

    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Takes a request in. A pointer movement joins the one waiting last, if
    /// that is one of the same kind and has not gone to the server yet.
    fn push(&mut self, seq: u32, mut request: Value) {
        let pointer = Pointer::of(&request);
        let last_waits = self.entries.len() > usize::from(self.asked);
        if let (true, Some(new), Some(last)) = (last_waits, &pointer, self.entries.back_mut())
            && last.seqs.len() < MERGED
        {
            let merged = match (&last.pointer, new) {
                (
                    Some(Pointer::Absolute { lease, buttons }),
                    Pointer::Absolute {
                        lease: new_lease,
                        buttons: new_buttons,
                    },
                ) if lease == new_lease && buttons == new_buttons => Some(new.clone()),
                (
                    Some(Pointer::Relative {
                        lease,
                        buttons,
                        x,
                        y,
                    }),
                    Pointer::Relative {
                        lease: new_lease,
                        buttons: new_buttons,
                        x: new_x,
                        y: new_y,
                    },
                ) if lease == new_lease && buttons == new_buttons => {
                    let sum = |a: i8, b: i8| {
                        let sum = i16::from(a) + i16::from(b);
                        (-127..=127).contains(&sum).then_some(sum as i8)
                    };
                    match (sum(*x, *new_x), sum(*y, *new_y)) {
                        (Some(x), Some(y)) => {
                            request["report"]["data"][1] = json!(x as u8);
                            request["report"]["data"][2] = json!(y as u8);
                            Some(Pointer::Relative {
                                lease: lease.clone(),
                                buttons: *buttons,
                                x,
                                y,
                            })
                        }
                        _ => None,
                    }
                }
                _ => None,
            };
            if let Some(merged) = merged {
                last.seqs.push(seq);
                last.request = request;
                last.pointer = Some(merged);
                return;
            }
        }
        self.entries.push_back(Entry {
            seqs: vec![seq],
            request,
            pointer,
        });
    }

    /// The request to send to the server next, if none is there already.
    fn next(&mut self) -> Option<Value> {
        if self.asked {
            return None;
        }
        let request = self.entries.front()?.request.clone();
        self.asked = true;
        Some(request)
    }

    /// The server answered the first request: the requests it stood for.
    fn answered(&mut self) -> Vec<u32> {
        self.asked = false;
        self.entries
            .pop_front()
            .map(|entry| entry.seqs)
            .unwrap_or_default()
    }
}

/// What the page is told when the server gave no answer.
fn unavailable() -> Value {
    json!({ "status": 503, "body": { "code": -503, "msg": "service unavailable" } })
}

type Asking<'a> = Pin<Box<dyn Future<Output = Option<Value>> + Send + 'a>>;

/// Serves the stream until the page closes it. Requests the page sent before
/// it closed the stream are still passed on and answered.
pub async fn serve(
    mut send: SendStream,
    mut receive: RecvStream,
    link: &Link,
    session: &str,
) -> Ended {
    let mut reader = MessageReader::new();
    let mut buffer = vec![0u8; READ_BUFFER];
    let mut queue = Queue::default();
    let mut asking: Option<Asking<'_>> = None;
    let mut open = true;
    loop {
        while !queue.is_full() {
            match reader.take::<InputRequest>() {
                Ok(Some(message)) => queue.push(message.seq, message.request),
                Ok(None) => break,
                Err(_) => return Ended::Protocol,
            }
        }
        if asking.is_none()
            && let Some(request) = queue.next()
        {
            asking = Some(Box::pin(link.lease(session, request)));
        }
        if !open && queue.is_empty() {
            return Ended::Page;
        }
        tokio::select! {
            answer = async {
                match asking.as_mut() {
                    Some(asking) => asking.await,
                    None => std::future::pending().await,
                }
            } => {
                asking = None;
                let response = answer.unwrap_or_else(unavailable);
                for seq in queue.answered() {
                    let message = InputResponse { seq, response: response.clone() };
                    let Ok(bytes) = page::encode(&message) else {
                        return Ended::Protocol;
                    };
                    if send.write_all(&bytes).await.is_err() {
                        return Ended::Page;
                    }
                }
            }
            read = receive.read(&mut buffer), if open && !queue.is_full() => {
                match read {
                    Ok(Some(count)) => reader.push(&buffer[..count]),
                    // The page closed the stream: what it sent is still served.
                    Ok(None) => open = false,
                    Err(_) => return Ended::Page,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: u8) -> Value {
        json!({
            "action": "report",
            "operationId": format!("{code:032x}"),
            "leaseId": "aa",
            "report": { "mode": "keyboard", "data": [0, 0, code, 0, 0, 0, 0, 0] },
        })
    }

    fn absolute(buttons: u8, x: u16, wheel: u8) -> Value {
        json!({
            "action": "report",
            "operationId": format!("{x:032x}"),
            "leaseId": "aa",
            "report": {
                "mode": "absolute_mouse",
                "data": [buttons, x & 0xff, x >> 8, 5, 0, wheel],
            },
        })
    }

    fn relative(buttons: u8, x: i8, y: i8, wheel: u8) -> Value {
        json!({
            "action": "report",
            "operationId": "0".repeat(32),
            "leaseId": "aa",
            "report": { "mode": "relative_mouse", "data": [buttons, x as u8, y as u8, wheel] },
        })
    }

    /// Drains the queue the way the stream does: the answers' sequence
    /// numbers, grouped as they are answered, with the request that went to
    /// the server for each group.
    fn drain(queue: &mut Queue) -> Vec<(Vec<u32>, Value)> {
        let mut out = Vec::new();
        while let Some(request) = queue.next() {
            out.push((queue.answered(), request));
        }
        out
    }

    #[test]
    fn requests_leave_in_the_order_they_came_and_keys_are_never_merged() {
        let mut queue = Queue::default();
        for (seq, code) in [4u8, 4, 5, 4].into_iter().enumerate() {
            queue.push(seq as u32, key(code));
        }
        queue.push(
            9,
            json!({ "action": "renew", "leaseId": "aa", "ttlSeconds": 30 }),
        );
        let sent = drain(&mut queue);
        assert_eq!(
            sent.iter()
                .map(|(seqs, _)| seqs.clone())
                .collect::<Vec<_>>(),
            vec![vec![0], vec![1], vec![2], vec![3], vec![9]]
        );
        assert_eq!(sent[2].1, key(5));
        assert!(queue.is_empty());
    }

    #[test]
    fn a_waiting_position_gives_way_to_a_newer_one() {
        let mut queue = Queue::default();
        queue.push(1, absolute(0, 10, 0));
        // The first is with the server: it is sent as it is.
        assert_eq!(queue.next(), Some(absolute(0, 10, 0)));
        queue.push(2, absolute(0, 20, 0));
        queue.push(3, absolute(0, 30, 0));
        queue.push(4, absolute(0, 40, 0));
        assert_eq!(queue.entries.len(), 2);
        assert_eq!(queue.answered(), vec![1]);
        // The newest position, for all three requests.
        assert_eq!(drain(&mut queue), vec![(vec![2, 3, 4], absolute(0, 40, 0))]);
    }

    #[test]
    fn buttons_the_wheel_and_another_lease_are_never_merged_away() {
        let mut queue = Queue::default();
        // Something is with the server, so everything below waits.
        queue.push(0, key(4));
        assert!(queue.next().is_some());
        queue.push(1, absolute(0, 10, 0));
        queue.push(2, absolute(1, 10, 0)); // a button goes down
        queue.push(3, absolute(1, 20, 0));
        queue.push(4, absolute(0, 20, 0)); // and up
        queue.push(5, absolute(0, 30, 1)); // the wheel
        queue.push(6, absolute(0, 40, 0));
        let mut other = absolute(0, 50, 0);
        other["leaseId"] = json!("bb");
        queue.push(7, other.clone());
        queue.push(8, key(5)); // a key between two positions
        queue.push(9, absolute(0, 60, 0));
        queue.push(10, absolute(0, 70, 0));
        assert_eq!(queue.answered(), vec![0]);
        let sent = drain(&mut queue);
        assert_eq!(
            sent.iter()
                .map(|(seqs, _)| seqs.clone())
                .collect::<Vec<_>>(),
            vec![
                vec![1],
                vec![2, 3],
                vec![4],
                vec![5],
                vec![6],
                vec![7],
                vec![8],
                vec![9, 10]
            ]
        );
        assert_eq!(sent[1].1, absolute(1, 20, 0));
        assert_eq!(sent[5].1, other);
        assert_eq!(sent[7].1, absolute(0, 70, 0));
    }

    #[test]
    fn relative_movements_add_up_within_a_report() {
        let mut queue = Queue::default();
        queue.push(0, key(4));
        assert!(queue.next().is_some());
        queue.push(1, relative(0, 10, -5, 0));
        queue.push(2, relative(0, 20, -6, 0));
        queue.push(3, relative(0, 90, 0, 0));
        // 120 + 10 does not fit a report: it stays a request of its own.
        queue.push(4, relative(0, 10, 0, 0));
        queue.push(5, relative(0, -127, -100, 0));
        queue.push(6, relative(0, 0, -28, 0)); // -128 does not fit either
        queue.push(7, relative(0, 3, 3, 1)); // the wheel
        assert_eq!(queue.answered(), vec![0]);
        let sent = drain(&mut queue);
        assert_eq!(
            sent.iter()
                .map(|(seqs, _)| seqs.clone())
                .collect::<Vec<_>>(),
            vec![vec![1, 2, 3], vec![4, 5], vec![6], vec![7]]
        );
        assert_eq!(sent[0].1, relative(0, 120, -11, 0));
        assert_eq!(sent[1].1, relative(0, -117, -100, 0));
        assert_eq!(sent[2].1, relative(0, 0, -28, 0));
    }

    #[test]
    fn the_queue_is_bounded_and_so_is_what_an_entry_stands_for() {
        let mut queue = Queue::default();
        for seq in 0..QUEUE as u32 {
            assert!(!queue.is_full());
            queue.push(seq, key(4));
        }
        assert!(queue.is_full());
        assert!(queue.next().is_some());
        assert_eq!(queue.answered(), vec![0]);
        assert!(!queue.is_full());

        let mut queue = Queue::default();
        queue.push(0, key(4));
        assert!(queue.next().is_some());
        for seq in 1..=(2 * MERGED as u32) {
            queue.push(seq, absolute(0, seq as u16, 0));
        }
        assert_eq!(queue.entries.len(), 3);
        assert_eq!(queue.answered(), vec![0]);
        let sent = drain(&mut queue);
        assert_eq!(sent[0].0.len(), MERGED);
        assert_eq!(sent[1].0.len(), MERGED);
    }

    #[test]
    fn what_is_no_pointer_report_is_left_alone() {
        for request in [
            json!({ "action": "acquire", "operationId": "x", "ttlSeconds": 30 }),
            json!({ "action": "report", "leaseId": "aa", "report": { "mode": "absolute_mouse", "data": [0, 1, 2] } }),
            json!({ "action": "report", "leaseId": "aa", "report": { "mode": "absolute_mouse", "data": [0, 1, 2, 3, 4, 300] } }),
            json!({ "action": "report", "leaseId": 5, "report": { "mode": "absolute_mouse", "data": [0, 1, 2, 3, 4, 0] } }),
            json!({ "action": "report", "leaseId": "aa", "report": { "mode": "tablet", "data": [0, 1, 2, 3, 4, 0] } }),
            json!([1, 2, 3]),
            json!(null),
        ] {
            assert_eq!(Pointer::of(&request), None, "{request}");
        }
        let mut queue = Queue::default();
        queue.push(0, key(4));
        assert!(queue.next().is_some());
        queue.push(1, json!(null));
        queue.push(2, json!(null));
        assert_eq!(queue.entries.len(), 3);
    }
}
