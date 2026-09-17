//! Read-only proxy to the board daemon status socket.
//!
//! The daemon owns the board lines, including the ATX host LED inputs. The
//! owner proxies one status read so the interface can show the host power
//! state without giving any other component direct line access.

use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    time::Duration,
};

use serde_json::Value;

/// One reviewed slice of the daemon status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoardStatus {
    pub power_led_present: bool,
    pub power_led_on: bool,
    pub hdd_led_present: bool,
    pub hdd_led_on: bool,
}

#[derive(Debug, Clone)]
pub struct BoardStatusClient {
    socket: PathBuf,
    timeout: Duration,
}

impl BoardStatusClient {
    pub fn new(socket: impl Into<PathBuf>, timeout: Duration) -> Self {
        Self {
            socket: socket.into(),
            timeout,
        }
    }

    /// Read one status line from the daemon; the response is bounded.
    pub fn read(&self) -> Result<BoardStatus, BoardStatusError> {
        let stream =
            UnixStream::connect(&self.socket).map_err(|_| BoardStatusError::Unavailable)?;
        let _ = stream.set_read_timeout(Some(self.timeout));
        let _ = stream.set_write_timeout(Some(self.timeout));
        let mut writer = stream
            .try_clone()
            .map_err(|_| BoardStatusError::Unavailable)?;
        writer
            .write_all(b"{\"request\":\"status\"}\n")
            .map_err(|_| BoardStatusError::Unavailable)?;
        writer.flush().map_err(|_| BoardStatusError::Unavailable)?;
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        let read = reader
            .read_line(&mut line)
            .map_err(|_| BoardStatusError::Unavailable)?;
        if read == 0 || read > 16 * 1024 {
            return Err(BoardStatusError::Unavailable);
        }
        let payload: Value =
            serde_json::from_str(line.trim_end()).map_err(|_| BoardStatusError::Invalid)?;
        let data = payload.get("data").ok_or(BoardStatusError::Invalid)?;
        let boolean = |key: &str| -> Result<bool, BoardStatusError> {
            data.get(key)
                .and_then(Value::as_bool)
                .ok_or(BoardStatusError::Invalid)
        };
        Ok(BoardStatus {
            power_led_present: boolean("atxPowerLedPresent")?,
            power_led_on: boolean("atxPowerLedOn")?,
            hdd_led_present: boolean("atxHddLedPresent")?,
            hdd_led_on: boolean("atxHddLedOn")?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardStatusError {
    Unavailable,
    Invalid,
}

/// The reviewed configuration: the daemon socket path, when configured.
pub fn board_status_client_from_environment() -> Option<BoardStatusClient> {
    let socket = std::env::var("BLI_KVM_BOARDD_STATUS_SOCKET").ok()?;
    if !socket.starts_with('/') || socket.contains('\n') {
        return None;
    }
    Some(BoardStatusClient::new(socket, Duration::from_millis(1500)))
}

#[cfg(test)]
mod tests {
    use std::{
        io::{BufRead, BufReader, Write},
        os::unix::net::UnixListener,
        thread,
    };

    use super::*;

    fn serve_once(directory: &std::path::Path, response: String) -> PathBuf {
        let socket = directory.join("boardd.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                let _ = reader.read_line(&mut line);
                assert_eq!(line.trim_end(), "{\"request\":\"status\"}");
                let mut writer = stream;
                let _ = writer.write_all(response.as_bytes());
            }
        });
        socket
    }

    fn status_line(power_on: bool) -> String {
        format!(
            "{{\"ok\":true,\"data\":{{\"atxPowerLedPresent\":true,\"atxPowerLedOn\":{power_on},\"atxHddLedPresent\":true,\"atxHddLedOn\":false}}}}\n"
        )
    }

    #[test]
    fn one_status_read_reports_the_led_inputs() {
        let directory = tempfile::tempdir().unwrap();
        let socket = serve_once(directory.path(), status_line(true));
        let client = BoardStatusClient::new(socket, Duration::from_secs(2));
        let status = client.read().unwrap();
        assert!(status.power_led_present);
        assert!(status.power_led_on);
        assert!(status.hdd_led_present);
        assert!(!status.hdd_led_on);
    }

    #[test]
    fn a_missing_socket_or_unexpected_shape_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let missing = BoardStatusClient::new(
            directory.path().join("absent.sock"),
            Duration::from_millis(200),
        );
        assert_eq!(missing.read(), Err(BoardStatusError::Unavailable));
        let socket = serve_once(directory.path(), "{\"ok\":true}\n".to_string());
        let client = BoardStatusClient::new(socket, Duration::from_secs(2));
        assert_eq!(client.read(), Err(BoardStatusError::Invalid));
    }
}
