//! The picture for one session: frames from the encoder, sent as the
//! transport's design says. A key frame travels whole on a unidirectional
//! stream of its own; if a newer one is ready before it is through, its
//! stream is reset. A delta frame is cut into datagrams that are handed to
//! the endpoint together, or not at all. A session that lost a frame gets no
//! more delta frames until the next key frame, which is asked for.

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use bli_kvm_quic_protocol::{
    frame::{self, FrameHeader, Kind, MAX_DATAGRAM_BYTES},
    page::{GatewayControl, VideoState},
};
use tokio::sync::{Notify, mpsc};
use wtransport::{Connection, VarInt};

use crate::frames::Subscription;

/// What a session's picture has cost and lost, for its counters.
#[derive(Debug, Default)]
pub struct VideoCounters {
    pub frames_sent: AtomicU64,
    pub frames_dropped: AtomicU64,
    pub key_frames: AtomicU64,
}

/// What the session's control side tells its picture.
#[derive(Debug, Default)]
pub struct VideoControl {
    /// The page asked for a key frame: it can decode nothing until one comes.
    pub page_needs_key: AtomicBool,
}

/// A stream that is reset when its key frame is overtaken.
const OVERTAKEN: u32 = 1;
/// Key frames give way to the control and the input stream.
const KEY_FRAME_PRIORITY: i32 = -1;

/// Writes one key frame on a stream of its own, unless `overtaken` comes
/// first.
async fn send_key_frame(connection: Connection, bytes: Vec<u8>, overtaken: Arc<Notify>) {
    let opening = tokio::select! {
        opening = connection.open_uni() => opening,
        () = overtaken.notified() => return,
    };
    let Ok(opening) = opening else {
        return;
    };
    let mut stream = tokio::select! {
        stream = opening => match stream {
            Ok(stream) => stream,
            Err(_) => return,
        },
        () = overtaken.notified() => return,
    };
    stream.set_priority(KEY_FRAME_PRIORITY);
    tokio::select! {
        written = async {
            stream.write_all(&bytes).await?;
            stream.finish().await
        } => {
            let _ = written;
        }
        () = overtaken.notified() => {
            let _ = stream.reset(VarInt::from_u32(OVERTAKEN));
        }
    }
}

/// Sends the frames of `subscription` to the session until the encoder's
/// socket ends or the session does. `events` carries what the page is told
/// on its control stream.
pub async fn serve(
    connection: Connection,
    mut subscription: Subscription,
    control: Arc<VideoControl>,
    counters: Arc<VideoCounters>,
    events: mpsc::Sender<GatewayControl>,
) {
    // Nothing can be decoded before a key frame.
    let mut needs_key = true;
    let mut announced: Option<(u32, u16, u16)> = None;
    let mut key_in_flight: Option<Arc<Notify>> = None;
    while let Some(record) = subscription.frames.recv().await {
        if subscription.take_lagged() {
            // Frames were lost to a full queue: the chain is broken.
            counters.frames_dropped.fetch_add(1, Ordering::Relaxed);
            needs_key = true;
            subscription.request_key();
        }
        if control.page_needs_key.swap(false, Ordering::AcqRel) {
            needs_key = true;
            subscription.request_key();
        }
        let header = &record.header;
        match header.kind {
            Kind::Key => {
                let Ok(head) = FrameHeader::key(
                    header.stream,
                    header.frame,
                    header.size,
                    header.capture_micros,
                )
                .encode() else {
                    continue;
                };
                let picture = (header.stream, header.width, header.height);
                if announced != Some(picture) {
                    announced = Some(picture);
                    let _ = events
                        .send(GatewayControl::Video {
                            state: VideoState::Started,
                            stream: Some(header.stream),
                            width: Some(header.width),
                            height: Some(header.height),
                            reason: None,
                        })
                        .await;
                }
                // A key frame still on its way is overtaken by this one.
                if let Some(earlier) = key_in_flight.take() {
                    earlier.notify_one();
                }
                let overtaken = Arc::new(Notify::new());
                key_in_flight = Some(Arc::clone(&overtaken));
                let mut bytes = Vec::with_capacity(head.len() + record.data.len());
                bytes.extend_from_slice(&head);
                bytes.extend_from_slice(&record.data);
                tokio::spawn(send_key_frame(connection.clone(), bytes, overtaken));
                needs_key = false;
                counters.key_frames.fetch_add(1, Ordering::Relaxed);
                counters.frames_sent.fetch_add(1, Ordering::Relaxed);
            }
            Kind::Delta => {
                if needs_key {
                    counters.frames_dropped.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                let limit = connection
                    .max_datagram_size()
                    .unwrap_or(0)
                    .min(MAX_DATAGRAM_BYTES);
                let fragments = frame::fragment(
                    header.stream,
                    header.frame,
                    header.reference,
                    header.capture_micros,
                    &record.data,
                    limit,
                );
                let sent = fragments.is_ok_and(|fragments| {
                    // The whole frame, or nothing of it: half a frame is of
                    // no use, and it would take the place of a later one.
                    let bytes: usize = fragments.iter().map(Vec::len).sum();
                    connection.quic_connection().datagram_send_buffer_space() >= bytes
                        && fragments
                            .into_iter()
                            .all(|fragment| connection.send_datagram(fragment).is_ok())
                });
                if sent {
                    counters.frames_sent.fetch_add(1, Ordering::Relaxed);
                } else {
                    counters.frames_dropped.fetch_add(1, Ordering::Relaxed);
                    needs_key = true;
                    subscription.request_key();
                }
            }
        }
    }
    if let Some(earlier) = key_in_flight.take() {
        earlier.notify_one();
    }
    // The encoder's socket ended: the page is told, and may ask again.
    let _ = events
        .send(GatewayControl::Video {
            state: VideoState::Stopped,
            stream: None,
            width: None,
            height: None,
            reason: Some("encoder".into()),
        })
        .await;
}
