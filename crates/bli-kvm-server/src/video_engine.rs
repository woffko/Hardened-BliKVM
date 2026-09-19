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
    time::Duration,
};

const IO_TIMEOUT: Duration = Duration::from_millis(800);
const MAX_RESPONSE: u64 = 64 * 1024;

/// The reviewed subset of the engine's `/state` the interface reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct EngineState {
    pub captured_fps: Option<u16>,
    pub width: Option<u16>,
    pub height: Option<u16>,
    pub queued_fps: Option<u16>,
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
    stream.take(MAX_RESPONSE).read_to_end(&mut raw).ok()?;
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (_, body) = text.split_once("\r\n\r\n")?;
    Some(body.to_string())
}

/// The engine wraps its fields in `result` and nests them differently between
/// releases, so each reviewed number is read wherever its key appears.
fn parse_number(body: &str, key: &str) -> Option<u16> {
    let needle = format!("\"{key}\"");
    let start = body.find(&needle)?;
    let rest = &body[start + needle.len()..];
    let colon = rest.find(':')?;
    let rest = rest[colon + 1..].trim_start();
    let digits: String = rest
        .chars()
        .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
        .collect();
    if digits.is_empty() {
        return None;
    }
    let value: f64 = digits.parse().ok()?;
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    Some(value.round() as u16)
}

fn parse_state(body: &str) -> EngineState {
    EngineState {
        captured_fps: parse_number(body, "captured_fps"),
        width: parse_number(body, "width"),
        height: parse_number(body, "height"),
        queued_fps: parse_number(body, "queued_fps"),
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
        assert_eq!(parse_number(body, "captured_fps"), Some(10));
    }

    #[test]
    fn accepts_a_float_rate_and_ignores_unrelated_text() {
        let body = r#"{"result":{"streamed_fps":9.8,"captured_fps":9.97}}"#;
        assert_eq!(parse_number(body, "captured_fps"), Some(10));
    }

    #[test]
    fn a_missing_or_invalid_field_reports_nothing() {
        assert_eq!(parse_number("{}", "captured_fps"), None);
        assert_eq!(parse_number(r#"{"result":{}}"#, "captured_fps"), None);
        assert_eq!(
            parse_number(r#"{"captured_fps":"fast"}"#, "captured_fps"),
            None
        );
    }

    #[test]
    fn a_missing_socket_reports_nothing_instead_of_failing() {
        assert!(read_state(Path::new("/run/definitely/absent.sock")).is_none());
    }

    #[test]
    fn reads_the_reviewed_state_subset() {
        let body = r#"{"ok":true,"result":{"encoder":{"type":"HW","quality":0},"source":{"resolution":{"width":1280,"height":720},"online":true,"desired_fps":60,"captured_fps":10},"stream":{"queued_fps":9,"clients":0}}}"#;
        let state = parse_state(body);
        assert_eq!(state.captured_fps, Some(10));
        assert_eq!((state.width, state.height), (Some(1280), Some(720)));
        assert_eq!(state.queued_fps, Some(9));
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
            Some(10)
        );
        server.join().unwrap();
        let _ = std::fs::remove_dir_all(&directory);
    }
}
