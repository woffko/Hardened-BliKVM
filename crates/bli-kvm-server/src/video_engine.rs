//! Read-only read-back of the running MJPEG engine.
//!
//! The owner starts `ustreamer` with the requested frame rate, but the bridge
//! may deliver fewer frames than requested (the USB2 capture path hands over
//! about 10 fps regardless of the desired value). The interface should not
//! claim a rate the stream does not deliver, so the server asks the engine's
//! own `/state` endpoint for the measured `captured_fps` and reports it next to
//! the requested value. The socket is read-only: nothing is written back and a
//! missing or unreadable engine only drops the extra field.

use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::Path,
    time::{Duration, Instant},
};

const IO_TIMEOUT: Duration = Duration::from_millis(800);
const MAX_RESPONSE: u64 = 64 * 1024;

/// The reviewed subset of the engine's `/state` the interface reports.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct EngineState {
    pub captured_fps: Option<f64>,
    pub width: Option<u16>,
    pub height: Option<u16>,
    pub queued_fps: Option<f64>,
    /// Whether the capture source reports a signal.
    pub online: Option<bool>,
}

/// Reads the reviewed state subset from the engine. Returns `None` only when
/// the engine is absent or unreadable; individual fields stay optional.
pub(crate) fn read_state(socket: &Path) -> Option<EngineState> {
    let body = fetch_state(socket)?;
    Some(parse_state(&body))
}

fn fetch_state(socket: &Path) -> Option<String> {
    let mut stream = UnixStream::connect(socket).ok()?;
    stream.set_read_timeout(Some(IO_TIMEOUT)).ok()?;
    stream.set_write_timeout(Some(IO_TIMEOUT)).ok()?;
    let request = "GET /state HTTP/1.0\r\nHost: ustreamer\r\nConnection: close\r\n\r\n";
    stream.write_all(request.as_bytes()).ok()?;
    let mut raw = Vec::new();
    let started = Instant::now();
    let mut buffer = [0; 4096];
    loop {
        let remaining = IO_TIMEOUT.checked_sub(started.elapsed())?;
        if remaining.is_zero() {
            return None;
        }
        stream.set_read_timeout(Some(remaining)).ok()?;
        let count = stream.read(&mut buffer).ok()?;
        if count == 0 {
            break;
        }
        if raw.len() + count > MAX_RESPONSE as usize {
            return None;
        }
        raw.extend_from_slice(&buffer[..count]);
    }
    if raw.len() as u64 > MAX_RESPONSE {
        return None;
    }
    let text = String::from_utf8(raw).ok()?;
    let (headers, body) = text.split_once("\r\n\r\n")?;
    let mut status = headers.lines().next()?.split_whitespace();
    if !matches!(status.next(), Some("HTTP/1.0" | "HTTP/1.1")) || status.next() != Some("200") {
        return None;
    }
    Some(body.to_string())
}

fn rate(value: &serde_json::Value) -> Option<f64> {
    value
        .as_f64()
        .filter(|value| value.is_finite() && *value >= 0.0)
}

fn dimension(value: &serde_json::Value) -> Option<u16> {
    value
        .as_u64()
        .and_then(|value| u16::try_from(value).ok())
        .filter(|value| *value > 0)
}

fn parse_state(body: &str) -> EngineState {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return EngineState::default();
    };
    if value["ok"] != true {
        return EngineState::default();
    }
    let source = &value["result"]["source"];
    EngineState {
        captured_fps: rate(&source["captured_fps"]),
        width: dimension(&source["resolution"]["width"]),
        height: dimension(&source["resolution"]["height"]),
        queued_fps: rate(&value["result"]["stream"]["queued_fps"]),
        online: source["online"].as_bool(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_rate_from_the_real_engine_report() {
        // Exact shape returned by ustreamer 6.11 on the board (2026-09-19):
        // captured_fps and desired_fps live under result.source.
        let body = r#"{"ok":true,"result":{"instance_id":"","encoder":{"type":"HW","quality":0},"source":{"resolution":{"width":1280,"height":720},"online":true,"desired_fps":60,"captured_fps":10},"stream":{"queued_fps":0,"clients":0,"clients_stat":{}}}}"#;
        assert_eq!(parse_state(body).captured_fps, Some(10.0));
        assert_eq!(parse_state(body).online, Some(true));
    }

    #[test]
    fn accepts_a_float_rate_and_ignores_unrelated_text() {
        let body = r#"{"ok":true,"captured_fps":999,"result":{"source":{"captured_fps":9.97},"stream":{"queued_fps":1e1}}}"#;
        assert_eq!(parse_state(body).captured_fps, Some(9.97));
        assert_eq!(parse_state(body).queued_fps, Some(10.0));
    }

    #[test]
    fn a_missing_or_invalid_field_reports_nothing() {
        for body in [
            "{}",
            r#"{"result":{}}"#,
            r#"{"ok":true,"result":{"source":{"captured_fps":"fast"}}}"#,
            r#"{"ok":false,"result":{"source":{"captured_fps":30}}}"#,
            "not json",
        ] {
            assert_eq!(parse_state(body).captured_fps, None);
        }
    }

    #[test]
    fn a_missing_socket_reports_nothing_instead_of_failing() {
        assert!(read_state(Path::new("/run/definitely/absent.sock")).is_none());
    }

    #[test]
    fn reads_the_reviewed_state_subset() {
        let body = r#"{"ok":true,"result":{"encoder":{"type":"HW","quality":0},"source":{"resolution":{"width":1280,"height":720},"online":true,"desired_fps":60,"captured_fps":10},"stream":{"queued_fps":9,"clients":0}}}"#;
        let state = parse_state(body);
        assert_eq!(state.captured_fps, Some(10.0));
        assert_eq!((state.width, state.height), (Some(1280), Some(720)));
        assert_eq!(state.queued_fps, Some(9.0));
    }

    #[test]
    fn dimensions_do_not_saturate_round_or_come_from_unrelated_fields() {
        let body = r#"{"ok":true,"width":1920,"height":1080,"result":{"source":{"resolution":{"width":65536,"height":720.5}}}}"#;
        assert_eq!(
            (parse_state(body).width, parse_state(body).height),
            (None, None)
        );
        let body =
            r#"{"ok":true,"result":{"source":{"captured_fps":-1},"stream":{"queued_fps":null}}}"#;
        assert_eq!(
            (parse_state(body).captured_fps, parse_state(body).queued_fps),
            (None, None)
        );
    }

    #[test]
    fn slow_response_cannot_extend_the_total_read_deadline() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("engine.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 256];
            assert!(stream.read(&mut request).unwrap() > 0);
            for chunk in [
                "HTTP/1.0 200 OK\r\n\r\n",
                "{\"ok\":true,",
                "\"result\":{",
                "\"source\":{",
                "\"captured_fps\":10",
                "}}}",
            ] {
                std::thread::sleep(Duration::from_millis(200));
                if stream.write_all(chunk.as_bytes()).is_err() {
                    break;
                }
            }
        });
        assert!(read_state(&socket).is_none());
        server.join().unwrap();
    }

    #[test]
    fn non_successful_http_cannot_supply_measured_state() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("engine.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 256];
            assert!(stream.read(&mut request).unwrap() > 0);
            stream.write_all(b"HTTP/1.0 503 Unavailable\r\n\r\n{\"ok\":true,\"result\":{\"source\":{\"captured_fps\":60}}}").unwrap();
        });
        assert!(read_state(&socket).is_none());
        server.join().unwrap();
    }

    #[test]
    fn reads_the_measured_rate_from_a_live_engine_socket() {
        use std::os::unix::net::UnixListener;

        let directory = std::env::temp_dir().join(format!(
            "blikvm-video-engine-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let socket = directory.join("engine.sock");
        let listener = UnixListener::bind(&socket).unwrap();

        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 256];
            let _ = std::io::Read::read(&mut stream, &mut request).unwrap();
            let body = r#"{"ok":true,"result":{"source":{"desired_fps":60,"captured_fps":10}}}"#;
            let response =
                format!("HTTP/1.0 200 OK\r\nContent-Type: application/json\r\n\r\n{body}");
            let _ = std::io::Write::write_all(&mut stream, response.as_bytes());
        });

        assert_eq!(
            read_state(&socket).and_then(|state| state.captured_fps),
            Some(10.0)
        );
        server.join().unwrap();
        let _ = std::fs::remove_dir_all(&directory);
    }
}
