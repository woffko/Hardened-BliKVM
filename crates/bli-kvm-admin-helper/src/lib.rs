//! Closed protocol between the web server and the root administration helper.
//!
//! One connection carries one request. The client sends a single JSON line;
//! the helper answers with a single JSON line and, for an interactive
//! operation, both sides then exchange length-prefixed frames. Every size is
//! bounded, unknown fields are refused, and nothing in a request names a
//! path, a command or a user.

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

/// Socket-activated helper endpoint (one service instance per connection).
pub const ADMIN_SOCKET: &str = "/run/hardened-blikvm-admin/admin.sock";
/// The unprivileged image download helper (network access, image store only).
pub const DOWNLOAD_SOCKET: &str = "/run/hardened-blikvm-download-api/download.sock";
/// Progress of the current or last download, read by the web server.
pub const DOWNLOAD_STATUS: &str = "/run/hardened-blikvm-download/status.json";
/// Largest image a download may produce, as in the reference.
pub const MAX_IMAGE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
/// The PicoClaw configuration helper, running as the runtime's own user.
pub const PICOCLAW_SOCKET: &str = "/run/hardened-blikvm-picoclaw-api/picoclaw.sock";
/// Home of the PicoClaw runtime user; `PICOCLAW_HOME` is `.picoclaw` in it.
pub const PICOCLAW_STATE: &str = "/var/lib/hardened-blikvm-picoclaw";
/// Installed runtime binary, owned by the web-server account.
pub const PICOCLAW_RUNTIME_DIR: &str = "/var/lib/hardened-blikvm-picoclaw-runtime";
pub const PICOCLAW_BINARY: &str = "/var/lib/hardened-blikvm-picoclaw-runtime/picoclaw";
/// Install progress, written by the download helper.
pub const PICOCLAW_INSTALL_STATUS: &str = "/run/hardened-blikvm-download/picoclaw-install.json";
/// The upstream release the reference also runs, pinned by digest.
pub const PICOCLAW_RELEASE_URL: &str =
    "https://github.com/sipeed/picoclaw/releases/download/v0.2.8/picoclaw_Linux_arm64.tar.gz";
pub const PICOCLAW_RELEASE_SHA256: &str =
    "9ce9fbf76da9c967baffb18c4b502dec2055b3ad050fc2381b6260429e1acf70";
pub const PICOCLAW_RELEASE_BYTES: u64 = 19_999_445;
pub const PICOCLAW_SERVICE: &str = "hardened-blikvm-picoclaw.service";
pub const PICOCLAW_GATEWAY_PORT: u16 = 18790;
/// Header carrying the internal token on the runtime's KVM calls.
pub const INTERNAL_TOKEN_HEADER: &str = "X-BliKVM-Internal-Token";
pub const MAX_LINE_BYTES: usize = 4096;
/// Replies may carry a chat history.
pub const MAX_REPLY_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_FRAME_BYTES: usize = 64 * 1024;
/// Script limits, as in the reference.
pub const MAX_SCRIPT_BYTES: usize = 256 * 1024;
pub const MAX_SCRIPT_OUTPUT_BYTES: usize = 64 * 1024;
pub const MAX_SCRIPTS: usize = 64;

/// Terminal bytes in either direction.
pub const FRAME_DATA: u8 = 0;
/// Window size from the client: rows then columns, big-endian `u16`.
pub const FRAME_RESIZE: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum AdminRequest {
    /// A root login shell on a pseudo-terminal.
    Terminal {
        rows: u16,
        cols: u16,
    },
    /// Names of the stored scripts.
    ScriptList {},
    /// Store a script; `size` bytes follow as data frames.
    ScriptUpload {
        name: String,
        size: u32,
    },
    ScriptDelete {
        name: String,
    },
    /// Run a stored script, in the foreground (bounded output) or detached.
    ScriptRun {
        name: String,
        background: bool,
    },
    /// Download an ISO image into the virtual media store.
    ImageDownload {
        url: String,
    },
    /// Download helper: fetch and unpack the pinned PicoClaw release.
    PicoclawInstall {},
    /// Download helper: remove the installed PicoClaw binary.
    PicoclawRemove {},
    /// Admin helper: start or stop the PicoClaw runtime unit.
    PicoclawService {
        start: bool,
    },
    /// PicoClaw helper: configuration and gateway facts, never the API key.
    PicoclawState {},
    /// PicoClaw helper: create the configuration if needed and apply the
    /// defaults, the gateway token and the KVM MCP server entry.
    PicoclawPrepare {
        internal_token: String,
        mcp_url: String,
    },
    PicoclawModel {
        model: String,
        api_base: String,
        api_key: String,
    },
    PicoclawProfile {
        profile: String,
    },
    PicoclawSessions {
        offset: u32,
        limit: u32,
    },
    PicoclawSession {
        id: String,
    },
    PicoclawSessionDelete {
        id: String,
    },
    /// PicoClaw helper: remove the runtime's configuration and history.
    PicoclawReset {},
}

/// What the PicoClaw helper reports about the runtime's configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PicoclawFacts {
    pub config_present: bool,
    pub model_configured: bool,
    pub model_name: String,
    pub gateway_token: String,
    pub agent_profile: String,
    pub ping_interval_ms: u32,
    pub read_timeout_ms: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PicoclawSessionItem {
    pub id: String,
    pub title: String,
    pub preview: String,
    pub message_count: u32,
    pub created: String,
    pub updated: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PicoclawMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PicoclawSessionDetail {
    pub id: String,
    pub messages: Vec<PicoclawMessage>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    pub created: String,
    pub updated: String,
}

/// Install progress in the reference's runtime-status terms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PicoclawInstallStatus {
    pub stage: String,
    pub progress: u8,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
}

/// PicoClaw session IDs as the runtime names them: the browser's UUID or an
/// opaque `sk_v1_` key, never a path.
pub fn valid_picoclaw_session_id(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && !id.starts_with('.')
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// The reference download status shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DownloadStatus {
    pub status: String,
    pub file: String,
    pub percentage: String,
}

impl DownloadStatus {
    pub fn idle() -> Self {
        Self {
            status: "idle".into(),
            file: String::new(),
            percentage: String::new(),
        }
    }
}

/// A validated remote ISO: the URL and the image name it ends with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteIso {
    pub url: String,
    pub filename: String,
}

/// HTTP(S) URLs without credentials whose last path segment is an `.iso`
/// image name the store accepts, as in the reference.
pub fn validate_remote_iso(raw: &str) -> Option<RemoteIso> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > 2048 || raw.chars().any(|ch| ch.is_control() || ch == ' ') {
        return None;
    }
    let (scheme, rest) = raw.split_once("://")?;
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return None;
    }
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    let path = rest[authority_end..]
        .split(['?', '#'])
        .next()
        .unwrap_or_default();
    let filename = path.rsplit('/').find(|segment| !segment.is_empty())?;
    let lower = filename.to_ascii_lowercase();
    let valid = (5..=128).contains(&filename.len())
        && lower.ends_with(".iso")
        && !filename.starts_with('.')
        && filename
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    valid.then(|| RemoteIso {
        url: raw.to_string(),
        filename: filename.to_string(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum AdminReply {
    Ready,
    Refused { reason: String },
    Scripts { files: Vec<String> },
    Output { log: String, success: bool },
    Done,
    Picoclaw { facts: PicoclawFacts },
    PicoclawModelSaved { model_name: String },
    PicoclawSessions { items: Vec<PicoclawSessionItem> },
    PicoclawSession { detail: PicoclawSessionDetail },
}

/// Script names as the reference accepts them: `[A-Za-z0-9._-]`, ending in
/// `.sh` or `.py`, 4 to 128 characters.
pub fn valid_script_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    (4..=128).contains(&name.len())
        && (lower.ends_with(".sh") || lower.ends_with(".py"))
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// Writes one JSON line.
pub fn write_line<T: Serialize>(writer: &mut impl Write, value: &T) -> io::Result<()> {
    let mut line = serde_json::to_vec(value).map_err(io::Error::other)?;
    if line.len() >= MAX_LINE_BYTES {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    line.push(b'\n');
    writer.write_all(&line)?;
    writer.flush()
}

/// Writes one reply line, which may be larger than a request.
pub fn write_reply(writer: &mut impl Write, reply: &AdminReply) -> io::Result<()> {
    let mut line = serde_json::to_vec(reply).map_err(io::Error::other)?;
    if line.len() >= MAX_REPLY_BYTES {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    line.push(b'\n');
    writer.write_all(&line)?;
    writer.flush()
}

/// Reads one JSON line of at most `MAX_LINE_BYTES`, one byte at a time so
/// that nothing after the newline is consumed.
pub fn read_line<T: for<'de> Deserialize<'de>>(reader: &mut impl Read) -> io::Result<T> {
    let mut line = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        if reader.read(&mut byte)? == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        if byte[0] == b'\n' {
            break;
        }
        line.push(byte[0]);
        if line.len() >= MAX_LINE_BYTES {
            return Err(io::ErrorKind::InvalidData.into());
        }
    }
    serde_json::from_slice(&line).map_err(|_| io::ErrorKind::InvalidData.into())
}

pub fn encode_frame(kind: u8, payload: &[u8]) -> io::Result<Vec<u8>> {
    if payload.len() > MAX_FRAME_BYTES {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut frame = Vec::with_capacity(5 + payload.len());
    frame.push(kind);
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(payload);
    Ok(frame)
}

/// Reads one frame; `None` at a clean end of stream.
pub fn read_frame(reader: &mut impl Read) -> io::Result<Option<(u8, Vec<u8>)>> {
    let mut header = [0_u8; 5];
    match reader.read_exact(&mut header[..1]) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    reader.read_exact(&mut header[1..])?;
    let length = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
    if length > MAX_FRAME_BYTES || !matches!(header[0], FRAME_DATA | FRAME_RESIZE) {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload)?;
    Ok(Some((header[0], payload)))
}

pub fn encode_resize(rows: u16, cols: u16) -> [u8; 4] {
    let [r0, r1] = rows.to_be_bytes();
    let [c0, c1] = cols.to_be_bytes();
    [r0, r1, c0, c1]
}

/// A usable window size, or `None` for an empty or oversized one.
pub fn decode_resize(payload: &[u8]) -> Option<(u16, u16)> {
    let [r0, r1, c0, c1] = payload.try_into().ok()?;
    let rows = u16::from_be_bytes([r0, r1]);
    let cols = u16::from_be_bytes([c0, c1]);
    (1..=1000).contains(&rows).then_some(())?;
    (1..=1000).contains(&cols).then_some((rows, cols))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_and_frames_round_trip_within_bounds() {
        let mut buffer = Vec::new();
        write_line(&mut buffer, &AdminRequest::Terminal { rows: 24, cols: 80 }).unwrap();
        buffer.extend(encode_frame(FRAME_DATA, b"ls\r").unwrap());
        buffer.extend(encode_frame(FRAME_RESIZE, &encode_resize(40, 120)).unwrap());
        let mut reader = buffer.as_slice();
        assert_eq!(
            read_line::<AdminRequest>(&mut reader).unwrap(),
            AdminRequest::Terminal { rows: 24, cols: 80 }
        );
        assert_eq!(
            read_frame(&mut reader).unwrap(),
            Some((FRAME_DATA, b"ls\r".to_vec()))
        );
        let (kind, payload) = read_frame(&mut reader).unwrap().unwrap();
        assert_eq!(kind, FRAME_RESIZE);
        assert_eq!(decode_resize(&payload), Some((40, 120)));
        assert_eq!(read_frame(&mut reader).unwrap(), None);
    }

    #[test]
    fn remote_iso_urls_are_validated() {
        let iso = validate_remote_iso("https://mirror.example/dist/ubuntu-24.04.iso?x=1").unwrap();
        assert_eq!(iso.filename, "ubuntu-24.04.iso");
        assert!(validate_remote_iso("http://example.org/a/b/Debian_12.ISO").is_some());
        for bad in [
            "ftp://example.org/a.iso",
            "https://someone@example.org/a.iso",
            "https://example.org/a.img",
            "https://example.org/",
            "https:///a.iso",
            "https://example.org/a b.iso",
            "https://example.org/..%2f.iso",
            "file:///etc/passwd.iso",
        ] {
            assert!(validate_remote_iso(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn picoclaw_session_ids_are_names_not_paths() {
        for good in ["3f2a9c1e-5b7d-4e8f-9a0b-1c2d3e4f5a6b", "sk_v1_abc123"] {
            assert!(valid_picoclaw_session_id(good), "{good}");
        }
        for bad in ["", "../x", "a/b", ".hidden", "a b", "a:b"] {
            assert!(!valid_picoclaw_session_id(bad), "{bad}");
        }
        let reply = AdminReply::PicoclawModelSaved {
            model_name: "gpt".into(),
        };
        let mut buffer = Vec::new();
        write_reply(&mut buffer, &reply).unwrap();
        assert_eq!(buffer.last(), Some(&b'\n'));
    }

    #[test]
    fn script_names_follow_the_reference_rules() {
        for good in ["backup.sh", "Report_1.py", "a-b.c.sh"] {
            assert!(valid_script_name(good), "{good}");
        }
        for bad in [
            "x.sh.txt",
            ".hidden.sh",
            "../a.sh",
            "a b.sh",
            "sh",
            "a/b.sh",
            "",
        ] {
            assert!(!valid_script_name(bad), "{bad}");
        }
        assert!(!valid_script_name(&format!("{}.sh", "a".repeat(126))));
    }

    #[test]
    fn malformed_requests_and_frames_are_refused() {
        for bad in [
            &b"{\"op\":\"terminal\",\"rows\":24,\"cols\":80,\"shell\":\"/bin/x\"}\n"[..],
            b"{\"op\":\"exec\"}\n",
            b"not json\n",
        ] {
            assert!(read_line::<AdminRequest>(&mut &bad[..]).is_err());
        }
        let long = vec![b'a'; MAX_LINE_BYTES + 1];
        assert!(read_line::<AdminRequest>(&mut long.as_slice()).is_err());
        assert!(encode_frame(FRAME_DATA, &vec![0; MAX_FRAME_BYTES + 1]).is_err());
        let mut oversized = vec![FRAME_DATA];
        oversized.extend(((MAX_FRAME_BYTES + 1) as u32).to_be_bytes());
        assert!(read_frame(&mut oversized.as_slice()).is_err());
        let unknown = [9_u8, 0, 0, 0, 0];
        assert!(read_frame(&mut unknown.as_slice()).is_err());
        assert_eq!(decode_resize(&encode_resize(0, 80)), None);
        assert_eq!(decode_resize(&[0, 1]), None);
    }
}
