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
    encoder_and_client_with(pictures, &[])
}

/// The same with more of the unit's settings.
fn encoder_and_client_with(
    pictures: &'static [&'static [u8]],
    more: &[(&str, &str)],
) -> (tempfile::TempDir, UdpSocket, Encoder, UnixStream) {
    let directory = tempfile::tempdir().unwrap();
    let upstream = directory.path().join("ustreamer.sock");
    let socket = directory.path().join("frames.sock");
    capture(&upstream, pictures);
    // The RTP packets go to a port that takes them, on Janus's address.
    let rtp = UdpSocket::bind("127.0.0.2:0").unwrap();
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
            .env_remove("BLI_KVM_ENCODER_HARDWARE")
            .env_remove("BLI_KVM_ENCODER_STATUS")
            .env_remove("BLI_KVM_ENCODER_REPORT")
            .envs(more.iter().map(|(name, value)| {
                // A value that begins with a dot is a file of the test's
                // own directory.
                match value.strip_prefix("./") {
                    Some(file) => (*name, directory.path().join(file).into_os_string()),
                    None => (*name, (*value).into()),
                }
            }))
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
fn the_rtp_goes_to_the_loopback_address_of_janus() {
    // Janus listens on 127.0.0.2, which the units that neither feed nor ask
    // it cannot reach (security review of 2026-10-18, MED-03).
    let (_directory, rtp, _encoder, _client) = encoder_and_client(&FRAMES);
    rtp.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let mut packet = [0_u8; 2048];
    let length = rtp
        .recv(&mut packet)
        .expect("no RTP packet reached Janus's address");
    assert!(length > 12, "{length}");
    assert_eq!(packet[0] >> 6, 2, "RTP version 2");
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
    // after the first; every frame of a still picture is a key frame. A
    // client that comes while the first frame is still being encoded is
    // let one more frame through a few frames after it, for the same
    // request: a delta frame, which comes within a second of the first and is
    // no part of the cadence. The periodic frame is the first one captured at
    // least 1.5 s after the first.
    let (periodic, _) = loop {
        let (header, frame) = read_record(&mut client);
        if header.capture_micros.saturating_sub(first.capture_micros) >= 1_500_000 {
            break (header, frame);
        }
    };
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

/// A unit that names the video engine's device on a board that has none:
/// x264 encodes, as before, and the encoder says so where the web server
/// reads it.
#[test]
fn without_the_video_engine_x264_encodes_and_the_encoder_says_so() {
    use std::os::unix::fs::PermissionsExt;
    let (directory, _rtp, _encoder, mut client) = encoder_and_client_with(
        &FRAMES,
        &[
            ("BLI_KVM_ENCODER_HARDWARE", "./no-such-device"),
            ("BLI_KVM_ENCODER_STATUS", "./encoder"),
            ("BLI_KVM_ENCODER_CONVERTERS", "3"),
        ],
    );
    let (first, frame) = read_record(&mut client);
    assert_eq!(first.kind, Kind::Key);
    assert!(has_nal(&frame, 7) && has_nal(&frame, 8) && has_nal(&frame, 5));
    for _ in 0..10 {
        read_record(&mut client);
    }
    let status = directory.path().join("encoder");
    assert_eq!(std::fs::read_to_string(&status).unwrap(), "x264\n");
    // Another user reads it.
    let mode = std::fs::metadata(&status).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o644);
}

/// Reads the encoder's report until `check` holds of it.
fn report_until(path: &Path, check: impl Fn(&serde_json::Value) -> bool) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(report) = std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            && check(&report)
        {
            return report;
        }
        assert!(Instant::now() < deadline, "the report did not come");
        thread::sleep(Duration::from_millis(20));
    }
}

/// Reads records until a key frame, each the one after the last; returns
/// it and how many came before it.
fn until_key(client: &mut UnixStream, last: &mut RecordHeader) -> usize {
    let mut before = 0;
    loop {
        let (header, _) = read_record(client);
        assert_eq!(header.stream, last.stream);
        assert_eq!(header.frame, last.frame + 1);
        *last = header;
        if header.kind == Kind::Key {
            return before;
        }
        before += 1;
    }
}

/// The board's choice of the engine on the demand socket, where the unit
/// names a device the board does not have: x264 encodes whatever is chosen,
/// the report says what is wanted and why the engine cannot be had, and
/// every choice is followed at once by a key frame in the same stream of
/// records.
#[test]
fn the_choice_of_the_engine_is_reported_and_followed_by_a_key_frame() {
    let (directory, _rtp, _encoder, mut client) = encoder_and_client_with(
        &FRAMES,
        &[
            ("BLI_KVM_ENCODER_HARDWARE", "./no-such-device"),
            ("BLI_KVM_ENCODER_STATUS", "./encoder"),
            ("BLI_KVM_ENCODER_REPORT", "./report.json"),
            ("BLI_KVM_VP8_DEMAND_SOCKET", "./demand.sock"),
        ],
    );
    let report = directory.path().join("report.json");
    // Before anyone watches: the device was looked at, nothing encodes.
    let idle = report_until(&report, |report| report["streaming"] == false);
    assert_eq!(idle["schema"], 1);
    assert_eq!(idle["codec"], "h264");
    assert!(idle["engine"].is_null());
    assert_eq!(idle["engineWanted"], "hardware");
    assert_eq!(idle["hardwareAvailable"], false);
    assert!(
        idle["hardwareReason"]
            .as_str()
            .unwrap()
            .contains("no-such-device")
    );
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(&report).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o644);

    let mut demand = UnixStream::connect(directory.path().join("demand.sock")).unwrap();
    demand
        .write_all(b"h264\nkbps 2500\nengine hardware\n")
        .unwrap();
    let (first, _) = read_record(&mut client);
    assert_eq!(first.kind, Kind::Key);
    let watched = report_until(&report, |report| report["streaming"] == true);
    assert_eq!(watched["engine"], "x264");
    assert_eq!(
        (watched["width"].as_u64(), watched["height"].as_u64()),
        (Some(320), Some(180))
    );
    assert_eq!(watched["kbps"], 2500);
    assert_eq!(
        (watched["maxFps"].as_u64(), watched["converters"].as_u64()),
        (Some(30), Some(2))
    );

    let mut last = first;
    for (line, wanted) in [
        (&b"engine software\n"[..], "software"),
        (&b"engine hardware\n"[..], "hardware"),
        (&b"engine software\n"[..], "software"),
    ] {
        // Just after a periodic key frame, so that the next one within a
        // dozen frames can only be the choice's.
        until_key(&mut client, &mut last);
        for _ in 0..5 {
            let (header, _) = read_record(&mut client);
            assert_eq!(header.frame, last.frame + 1);
            assert_eq!(header.kind, Kind::Delta);
            last = header;
        }
        let chosen = Instant::now();
        demand.write_all(line).unwrap();
        let before = until_key(&mut client, &mut last);
        let waited = chosen.elapsed();
        assert!(
            waited < Duration::from_millis(900) && before < 12,
            "the key frame came {waited:?} and {before} frames after the choice"
        );
        let now = report_until(&report, |report| report["engineWanted"] == wanted);
        assert_eq!(now["engine"], "x264");
        assert_eq!(now["hardwareAvailable"], false);
        assert_eq!(
            std::fs::read_to_string(directory.path().join("encoder")).unwrap(),
            "x264\n"
        );
    }
    // The viewer leaves: the report says so once.
    drop(demand);
    let left = report_until(&report, |report| report["streaming"] == false);
    assert_eq!(left["engine"], "x264");
}

/// While a viewer watches, the report is written about once a second with
/// its numbers: frames handed to the encoder, the gate's counts, the
/// stages' times; there is no engine on the host, so no engine's counts.
#[test]
fn the_report_counts_once_a_second_while_a_viewer_watches() {
    let (directory, _rtp, _encoder, mut client) = encoder_and_client_with(
        &FRAMES,
        &[
            ("BLI_KVM_ENCODER_REPORT", "./report.json"),
            ("BLI_KVM_VP8_DEMAND_SOCKET", "./demand.sock"),
        ],
    );
    let report = directory.path().join("report.json");
    let mut demand = UnixStream::connect(directory.path().join("demand.sock")).unwrap();
    demand.write_all(b"h264\nkbps 2500\n").unwrap();
    read_record(&mut client);
    let first = report_until(&report, |report| report["encodedFrames"].as_u64() > Some(0));
    let written = |report: &Path| std::fs::metadata(report).unwrap().modified().unwrap();
    // Every report written in three seconds, as the server would see them.
    let mut seen = vec![(written(&report), first)];
    let began = Instant::now();
    while began.elapsed() < Duration::from_millis(3300) {
        read_record(&mut client);
        let at = written(&report);
        if at != seen.last().unwrap().0 {
            let value: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&report).unwrap()).unwrap();
            seen.push((at, value));
        }
    }
    assert!(
        (3..=5).contains(&seen.len()),
        "{} reports in about three seconds",
        seen.len()
    );
    for pair in seen.windows(2) {
        let (before, after) = (&pair[0].1, &pair[1].1);
        let gap = pair[1].0.duration_since(pair[0].0).unwrap();
        assert!(
            gap >= Duration::from_millis(900),
            "two reports {gap:?} apart"
        );
        assert!(after["encodedFrames"].as_u64() > before["encodedFrames"].as_u64());
        assert!(after["gate"]["admitted"].as_u64() > before["gate"]["admitted"].as_u64());
        assert!(after["gate"]["tooSoon"].as_u64() >= before["gate"]["tooSoon"].as_u64());
    }
    let last = &seen.last().unwrap().1;
    assert_eq!(last["streaming"], true);
    assert!(last["stageMicros"]["decode"].as_u64() > Some(0));
    assert!(last["stageMicros"]["encode"].as_u64() > Some(0));
    assert!(last["hardware"].is_null());
    assert!(std::fs::read(&report).unwrap().len() < 4096);
}
