//! The encoder binary's frame socket, end to end: capture frames from a
//! stand-in for uStreamer are encoded by x264 and reach a client of the
//! socket as records it can check.

use std::{
    io::{Read, Write},
    net::UdpSocket,
    os::unix::net::{UnixListener, UnixStream},
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use bli_kvm_quic_protocol::frame::{HEADER_BYTES, Kind, RecordHeader};
use bli_kvm_video_encoder::frames::{KEY_COOLDOWN, KEY_REQUEST, monotonic_micros};

/// One picture in three subsamplings: every frame differs from the one
/// before it, so every frame is encoded.
const FRAMES: [&[u8]; 3] = [
    include_bytes!("fixtures/pattern-320x180-420.jpg"),
    include_bytes!("fixtures/pattern-320x180-422.jpg"),
    include_bytes!("fixtures/pattern-320x180-444.jpg"),
];

struct Encoder(Child);

impl Drop for Encoder {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// uStreamer's side of its socket: the MJPEG stream, thirty frames a second,
/// the given pictures one after the other.
fn capture(path: &Path, pictures: &'static [&'static [u8]]) {
    let listener = UnixListener::bind(path).unwrap();
    thread::spawn(move || {
        for mut connection in listener.incoming().flatten() {
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                if connection.read(&mut byte).unwrap_or(0) == 0 {
                    break;
                }
                request.push(byte[0]);
            }
            let head =
                "HTTP/1.0 200 OK\r\nContent-Type: multipart/x-mixed-replace;boundary=frame\r\n\r\n";
            if connection.write_all(head.as_bytes()).is_err() {
                continue;
            }
            for index in 0.. {
                let jpeg = pictures[index % pictures.len()];
                let part = format!(
                    "--frame\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
                    jpeg.len()
                );
                if connection.write_all(part.as_bytes()).is_err()
                    || connection.write_all(jpeg).is_err()
                    || connection.write_all(b"\r\n").is_err()
                {
                    break;
                }
                thread::sleep(Duration::from_millis(33));
            }
        }
    });
}

fn read_record(client: &mut UnixStream) -> (RecordHeader, Vec<u8>) {
    let mut head = [0u8; HEADER_BYTES];
    client.read_exact(&mut head).expect("no record came");
    let header = RecordHeader::decode(&head).expect("a header the reader refuses");
    let mut frame = vec![0u8; header.size as usize];
    client.read_exact(&mut frame).unwrap();
    (header, frame)
}

/// Whether an Annex B frame has a NAL unit of this type.
fn has_nal(frame: &[u8], kind: u8) -> bool {
    frame
        .windows(4)
        .any(|window| window[..3] == [0, 0, 1] && window[3] & 0x1f == kind)
}

/// The encoder's own process on a capture stream of `pictures`, and a
/// client of its frame socket.
fn encoder_and_client(
    pictures: &'static [&'static [u8]],
) -> (tempfile::TempDir, UdpSocket, Encoder, UnixStream) {
    let directory = tempfile::tempdir().unwrap();
    let upstream = directory.path().join("ustreamer.sock");
    let socket = directory.path().join("frames.sock");
    capture(&upstream, pictures);
    // The RTP packets go to a port that takes them.
    let rtp = UdpSocket::bind("127.0.0.1:0").unwrap();
    let encoder = Encoder(
        Command::new(env!("CARGO_BIN_EXE_bli-kvm-video-encoder"))
            .env("BLI_KVM_USTREAMER_SOCKET", &upstream)
            .env("BLI_KVM_ENCODER_CODEC", "h264-ultrafast")
            .env("BLI_KVM_ENCODER_FRAME_SOCKET", &socket)
            .env(
                "BLI_KVM_ENCODER_RTP_PORT",
                rtp.local_addr().unwrap().port().to_string(),
            )
            .env("BLI_KVM_VP8_THREADS", "1")
            .env_remove("BLI_KVM_VP8_DEMAND_SOCKET")
            .env_remove("BLI_KVM_ENCODER_FRAME_USER")
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let client = loop {
        match UnixStream::connect(&socket) {
            Ok(client) => break client,
            Err(_) => {
                assert!(Instant::now() < deadline, "the frame socket did not open");
                thread::sleep(Duration::from_millis(20));
            }
        }
    };
    client
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    (directory, rtp, encoder, client)
}

#[test]
fn the_encoder_serves_numbered_frames_and_a_key_frame_on_request() {
    let (_directory, _rtp, _encoder, mut client) = encoder_and_client(&FRAMES);

    // The first record is a key frame with the stream's parameter sets.
    let (first, frame) = read_record(&mut client);
    assert_eq!(first.kind, Kind::Key);
    assert_eq!((first.width, first.height), (320, 180));
    assert_eq!(first.reference, first.frame);
    assert!(first.stream != 0);
    assert!(has_nal(&frame, 7) && has_nal(&frame, 8) && has_nal(&frame, 5));
    let now = monotonic_micros();
    assert!(
        first.capture_micros <= now && now - first.capture_micros < 2_000_000,
        "captured at {} against {now}",
        first.capture_micros
    );

    // The frames after it: one id after the other, each naming the last key
    // frame, captured in order.
    let mut last = first;
    let mut key = first.frame;
    let mut deltas = 0;
    for _ in 0..40 {
        let (header, frame) = read_record(&mut client);
        assert_eq!(header.stream, first.stream);
        assert_eq!(header.frame, last.frame + 1);
        assert!(header.capture_micros >= last.capture_micros);
        match header.kind {
            Kind::Key => {
                key = header.frame;
                assert!(has_nal(&frame, 7));
            }
            Kind::Delta => {
                deltas += 1;
                assert!(has_nal(&frame, 1), "a delta frame without a slice");
                assert!(!has_nal(&frame, 5));
            }
        }
        assert_eq!(header.reference, key);
        last = header;
    }
    assert!(deltas >= 30, "{deltas} delta frames of 40");

    // A key frame on request. The encoder also makes one every two seconds
    // by itself, so the request is sent just after one of those: the next
    // key frame within a dozen frames can only be the request's.
    loop {
        let (header, _) = read_record(&mut client);
        assert_eq!(header.frame, last.frame + 1);
        last = header;
        if header.kind == Kind::Key {
            break;
        }
    }
    for _ in 0..5 {
        let (header, _) = read_record(&mut client);
        assert_eq!(header.kind, Kind::Delta);
        last = header;
    }
    thread::sleep(KEY_COOLDOWN);
    client.write_all(&[KEY_REQUEST]).unwrap();
    let mut waited = 0;
    loop {
        let (header, _) = read_record(&mut client);
        assert_eq!(header.frame, last.frame + 1);
        last = header;
        // Frames that were on their way when the request was written.
        if header.kind == Kind::Key {
            break;
        }
        waited += 1;
        assert!(waited < 30, "no key frame came for the request");
    }
    assert_eq!(last.reference, last.frame);
}

/// A still screen gives the encoder a frame only every two seconds. A client
/// that comes, and one that asks for a key frame, do not wait for it: the
/// next captured frame is let through for them.
#[test]
fn on_a_still_picture_a_key_frame_comes_when_it_is_asked_for() {
    let started = Instant::now();
    let (_directory, _rtp, _encoder, mut client) = encoder_and_client(&FRAMES[..1]);
    let (first, _) = read_record(&mut client);
    assert_eq!(first.kind, Kind::Key);
    let first_at = Instant::now();
    // Without a request the next frame is the periodic one, two seconds
    // after the first; every frame of a still picture is a key frame.
    let (periodic, _) = read_record(&mut client);
    assert_eq!(periodic.kind, Kind::Key);
    let period = first_at.elapsed();
    assert!(
        period >= Duration::from_millis(1500),
        "a still picture gave a frame after {period:?}"
    );

    // Asked for after the cooldown, and long before the next periodic one.
    thread::sleep(KEY_COOLDOWN + Duration::from_millis(100));
    let asked = Instant::now();
    client.write_all(&[KEY_REQUEST]).unwrap();
    let (answer, frame) = read_record(&mut client);
    let waited = asked.elapsed();
    assert_eq!(answer.kind, Kind::Key);
    assert_eq!(answer.frame, periodic.frame + 1);
    assert!(has_nal(&frame, 7) && has_nal(&frame, 5));
    assert!(
        waited < Duration::from_millis(900),
        "the key frame came {waited:?} after it was asked for"
    );
    // The first one did not wait for a period either.
    assert!(first_at.duration_since(started) < Duration::from_secs(8));
}
