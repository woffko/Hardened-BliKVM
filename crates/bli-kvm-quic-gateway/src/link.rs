//! The gateway's side of the link to the web server.
//!
//! One connection to the server's Unix socket carries every request of every
//! session. The server is the authority: it redeems tokens, serves lease
//! requests and ends sessions. While the link is down the gateway serves
//! nobody, and a link that drops ends every session it carried.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use bli_kvm_quic_protocol::{
    PROTOCOL,
    link::{self, EndReason, GatewayToServer, RedeemRefusal, ServerToGateway},
    page::Refusal,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    sync::{Notify, mpsc, oneshot},
};

use crate::certificate::CertificateInfo;

/// How long a request waits for the server's answer.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(3);
const RECONNECT_FIRST: Duration = Duration::from_millis(500);
const RECONNECT_MAX: Duration = Duration::from_secs(5);
/// A server that speaks another protocol is asked again only rarely.
const RECONNECT_UNSUPPORTED: Duration = Duration::from_secs(30);
/// Messages waiting to be written to the link.
const OUTBOX: usize = 256;

#[derive(Default)]
struct State {
    /// The writer of the connection that is up, if one is.
    outbox: Option<mpsc::Sender<Vec<u8>>>,
    next_id: u64,
    pending: HashMap<u64, oneshot::Sender<ServerToGateway>>,
    sessions: HashMap<String, mpsc::Sender<EndReason>>,
}

struct Shared {
    path: PathBuf,
    port: u16,
    certificate: Mutex<CertificateInfo>,
    state: Mutex<State>,
    /// The gateway is stopping: the link is closed and not looked for again.
    leaving: Notify,
}

#[derive(Clone)]
pub struct Link(Arc<Shared>);

impl Link {
    /// Starts the task that keeps the link up.
    pub fn spawn(path: PathBuf, port: u16, certificate: CertificateInfo) -> Self {
        let link = Self(Arc::new(Shared {
            path,
            port,
            certificate: Mutex::new(certificate),
            state: Mutex::new(State::default()),
            leaving: Notify::new(),
        }));
        tokio::spawn(link.clone().run());
        link
    }

    pub fn is_up(&self) -> bool {
        self.0.state.lock().unwrap().outbox.is_some()
    }

    /// Closes the link for good. A gateway that stops does this first, so
    /// the server issues no token for an endpoint that no longer answers.
    pub fn leave(&self) {
        self.0.leaving.notify_one();
    }

    fn hello(&self) -> GatewayToServer {
        let certificate = self.0.certificate.lock().unwrap().clone();
        GatewayToServer::Hello {
            protocol: PROTOCOL,
            certificate_sha256: certificate.sha256,
            not_before: certificate.not_before,
            not_after: certificate.not_after,
            port: self.0.port,
        }
    }

    /// The endpoint presents a new certificate from now on.
    pub fn set_certificate(&self, certificate: CertificateInfo) {
        *self.0.certificate.lock().unwrap() = certificate;
        self.send(&self.hello());
    }

    fn send(&self, message: &GatewayToServer) -> bool {
        let Ok(bytes) = link::encode(message) else {
            return false;
        };
        let state = self.0.state.lock().unwrap();
        match &state.outbox {
            Some(outbox) => outbox.try_send(bytes).is_ok(),
            None => false,
        }
    }

    /// Sends a request built from its id and waits for the server's answer.
    async fn ask(&self, build: impl FnOnce(u64) -> GatewayToServer) -> Option<ServerToGateway> {
        let (sender, receiver) = oneshot::channel();
        let id = {
            let mut state = self.0.state.lock().unwrap();
            state.next_id += 1;
            let id = state.next_id;
            state.pending.insert(id, sender);
            id
        };
        if !self.send(&build(id)) {
            self.0.state.lock().unwrap().pending.remove(&id);
            return None;
        }
        match tokio::time::timeout(ANSWER_TIMEOUT, receiver).await {
            Ok(Ok(answer)) => Some(answer),
            _ => {
                self.0.state.lock().unwrap().pending.remove(&id);
                None
            }
        }
    }

    /// Redeems a page's token. The answer is the session's reference and the
    /// receiver on which the server ends the session.
    pub async fn redeem(
        &self,
        token: String,
        peer: String,
    ) -> Result<(String, mpsc::Receiver<EndReason>), Refusal> {
        let answer = self
            .ask(|id| GatewayToServer::Redeem { id, token, peer })
            .await
            .ok_or(Refusal::Unavailable)?;
        match answer {
            ServerToGateway::Redeemed {
                session: Some(session),
                refused: None,
                ..
            } => {
                let (sender, receiver) = mpsc::channel(1);
                let mut state = self.0.state.lock().unwrap();
                // The link may have dropped while the answer was on its way.
                if state.outbox.is_none() {
                    return Err(Refusal::Unavailable);
                }
                state.sessions.insert(session.clone(), sender);
                Ok((session, receiver))
            }
            ServerToGateway::Redeemed {
                refused: Some(RedeemRefusal::Sessions),
                ..
            } => Err(Refusal::Sessions),
            ServerToGateway::Redeemed { .. } => Err(Refusal::Token),
            _ => Err(Refusal::Unavailable),
        }
    }

    /// A request of the HID lease API from a session's input stream. The
    /// answer is the server's, `None` if the link gave none in time.
    pub async fn lease(
        &self,
        session: &str,
        request: serde_json::Value,
    ) -> Option<serde_json::Value> {
        let session = session.to_owned();
        match self
            .ask(|id| GatewayToServer::Lease {
                id,
                session,
                request,
            })
            .await?
        {
            ServerToGateway::LeaseAnswer { response, .. } => Some(response),
            _ => None,
        }
    }

    /// A session's connection is gone.
    pub fn closed(&self, session: &str) {
        let known = self
            .0
            .state
            .lock()
            .unwrap()
            .sessions
            .remove(session)
            .is_some();
        if known {
            self.send(&GatewayToServer::Closed {
                session: session.to_string(),
            });
        }
    }

    async fn run(self) {
        let mut wait = RECONNECT_FIRST;
        loop {
            let outcome = tokio::select! {
                outcome = self.connection() => outcome,
                () = self.0.leaving.notified() => {
                    self.drop_everything();
                    return;
                }
            };
            self.drop_everything();
            let pause = match outcome {
                // It was up: look for the restarted server soon.
                Some(true) => {
                    wait = RECONNECT_FIRST;
                    wait
                }
                Some(false) => RECONNECT_UNSUPPORTED,
                None => {
                    let pause = wait;
                    wait = (wait * 2).min(RECONNECT_MAX);
                    pause
                }
            };
            tokio::select! {
                () = tokio::time::sleep(pause) => {}
                () = self.0.leaving.notified() => return,
            }
        }
    }

    /// One connection, from its hello to its end. `Some(true)` if the server
    /// answered with this protocol, `Some(false)` if with another, `None` if
    /// it could not be reached.
    async fn connection(&self) -> Option<bool> {
        let stream = UnixStream::connect(&self.0.path).await.ok()?;
        let (mut reader, mut writer) = stream.into_split();
        writer
            .write_all(&link::encode(&self.hello()).ok()?)
            .await
            .ok()?;
        match read_message(&mut reader).await? {
            ServerToGateway::Hello { protocol } if protocol == PROTOCOL => {}
            ServerToGateway::Hello { protocol } => {
                eprintln!(
                    "quic gateway: the server speaks link protocol {protocol}, not {PROTOCOL}"
                );
                return Some(false);
            }
            _ => return None,
        }
        let (outbox, mut queued) = mpsc::channel::<Vec<u8>>(OUTBOX);
        self.0.state.lock().unwrap().outbox = Some(outbox);
        eprintln!("quic gateway: the server link is up");
        let write = async {
            while let Some(bytes) = queued.recv().await {
                if writer.write_all(&bytes).await.is_err() {
                    break;
                }
            }
        };
        let read = async {
            while let Some(message) = read_message(&mut reader).await {
                self.dispatch(message);
            }
        };
        tokio::select! {
            () = write => {}
            () = read => {}
        }
        eprintln!("quic gateway: the server link is down");
        Some(true)
    }

    fn dispatch(&self, message: ServerToGateway) {
        let mut state = self.0.state.lock().unwrap();
        match &message {
            ServerToGateway::Redeemed { id, .. }
            | ServerToGateway::LeaseAnswer { id, .. }
            | ServerToGateway::VideoAnswer { id, .. } => {
                if let Some(waiting) = state.pending.remove(id) {
                    let _ = waiting.send(message);
                }
            }
            ServerToGateway::End { session, reason } => {
                if let Some(ended) = state.sessions.remove(session) {
                    let _ = ended.try_send(*reason);
                }
            }
            ServerToGateway::Hello { .. } => {}
        }
    }

    /// The link is gone: nobody waits for an answer any more, and every
    /// session it carried ends.
    fn drop_everything(&self) {
        let mut state = self.0.state.lock().unwrap();
        state.outbox = None;
        state.pending.clear();
        for (_, ended) in state.sessions.drain() {
            let _ = ended.try_send(EndReason::Shutdown);
        }
    }
}

async fn read_message(reader: &mut tokio::net::unix::OwnedReadHalf) -> Option<ServerToGateway> {
    let mut prefix = [0u8; 4];
    reader.read_exact(&mut prefix).await.ok()?;
    let length = link::body_length(prefix).ok()?;
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).await.ok()?;
    link::decode(&body).ok()
}
