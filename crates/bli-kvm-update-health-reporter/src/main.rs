use std::{
    env,
    ffi::OsString,
    fs::OpenOptions,
    io::{self, Read, Write},
    net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, TcpStream},
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        net::UnixStream,
    },
    path::{Component, Path, PathBuf},
    process::ExitCode,
    thread,
    time::{Duration, Instant},
};

use thiserror::Error;
use zeroize::Zeroize;

const CREDENTIALS_DIRECTORY_ENV: &str = "CREDENTIALS_DIRECTORY";
const CREDENTIAL_FILE_NAME: &str = "application-health-token";
const CREDENTIAL_ALIAS_ROOT: &str = "/run/hardened-blikvm-candidate-credentials";
const HEALTH_ATTEMPTS: usize = 30;
const HEALTH_RETRY_DELAY: Duration = Duration::from_millis(200);
const IO_TIMEOUT: Duration = Duration::from_millis(500);
const MAX_RESPONSE_BYTES: usize = 8 * 1024;

#[derive(Debug, Error)]
enum ReporterError {
    #[error("reporter arguments are invalid")]
    InvalidArguments,
    #[error("systemd credential directory is missing or unsafe")]
    InvalidCredentialDirectory,
    #[error("one-use health credential is missing or unsafe")]
    InvalidCredential,
    #[error("candidate health endpoint did not become ready")]
    CandidateNotReady,
    #[error("candidate health response is invalid")]
    InvalidHealthResponse,
    #[error("confirmation socket is unavailable")]
    ConfirmationUnavailable,
    #[error("reporter I/O failed")]
    Io(#[from] io::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReporterConfig {
    health_endpoint: SocketAddr,
    confirmation_socket: PathBuf,
    credential_directory: Option<PathBuf>,
}

fn main() -> ExitCode {
    match run(env::args_os()) {
        Ok(()) => {
            println!("Hardened BliKVM candidate health submitted");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("Hardened BliKVM health reporter failed: {error}");
            ExitCode::from(error.exit_code())
        }
    }
}

impl ReporterError {
    fn exit_code(&self) -> u8 {
        match self {
            Self::InvalidArguments => 2,
            Self::InvalidCredentialDirectory => 10,
            Self::InvalidCredential => 11,
            Self::CandidateNotReady => 12,
            Self::InvalidHealthResponse => 13,
            Self::ConfirmationUnavailable => 14,
            Self::Io(_) => 15,
        }
    }
}

fn run(arguments: impl IntoIterator<Item = OsString>) -> Result<(), ReporterError> {
    let config = parse_arguments(arguments)?;
    let credential_directory = config
        .credential_directory
        .clone()
        .or_else(|| env::var_os(CREDENTIALS_DIRECTORY_ENV).map(PathBuf::from))
        .ok_or(ReporterError::InvalidCredentialDirectory)?;
    report(&config, &credential_directory)
}

fn parse_arguments(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<ReporterConfig, ReporterError> {
    let mut arguments = arguments.into_iter();
    let _program = arguments.next().ok_or(ReporterError::InvalidArguments)?;
    if arguments.next().as_deref() != Some(std::ffi::OsStr::new("--health-endpoint")) {
        return Err(ReporterError::InvalidArguments);
    }
    let endpoint = arguments
        .next()
        .and_then(|value| value.into_string().ok())
        .and_then(|value| value.parse::<SocketAddr>().ok())
        .ok_or(ReporterError::InvalidArguments)?;
    if endpoint.ip() != IpAddr::V4(Ipv4Addr::LOCALHOST) || endpoint.port() < 1024 {
        return Err(ReporterError::InvalidArguments);
    }
    if arguments.next().as_deref() != Some(std::ffi::OsStr::new("--confirm-socket")) {
        return Err(ReporterError::InvalidArguments);
    }
    let confirmation_socket =
        PathBuf::from(arguments.next().ok_or(ReporterError::InvalidArguments)?);
    if !valid_runtime_socket(&confirmation_socket) {
        return Err(ReporterError::InvalidArguments);
    }
    let credential_directory = match arguments.next() {
        None => None,
        Some(option) if option == "--credential-directory" => {
            let path = PathBuf::from(arguments.next().ok_or(ReporterError::InvalidArguments)?);
            if path.parent() != Some(Path::new(CREDENTIAL_ALIAS_ROOT))
                || !matches!(path.components().next_back(), Some(Component::Normal(_)))
            {
                return Err(ReporterError::InvalidArguments);
            }
            Some(path)
        }
        Some(_) => return Err(ReporterError::InvalidArguments),
    };
    if arguments.next().is_some() {
        return Err(ReporterError::InvalidArguments);
    }
    Ok(ReporterConfig {
        health_endpoint: endpoint,
        confirmation_socket,
        credential_directory,
    })
}

fn valid_runtime_socket(path: &Path) -> bool {
    if !path.is_absolute()
        || path.file_name().is_none()
        || path.as_os_str().as_encoded_bytes().len() >= 100
        || path
            .components()
            .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
    {
        return false;
    }
    path.starts_with("/run/") || (cfg!(debug_assertions) && path.starts_with("/tmp/"))
}

fn report(config: &ReporterConfig, credential_directory: &Path) -> Result<(), ReporterError> {
    validate_credential_directory(credential_directory)?;
    let mut token = read_credential(credential_directory)?;
    let result = (|| {
        wait_until_healthy(config.health_endpoint)?;
        let mut socket = UnixStream::connect(&config.confirmation_socket)
            .map_err(|_| ReporterError::ConfirmationUnavailable)?;
        socket
            .set_write_timeout(Some(IO_TIMEOUT))
            .map_err(|_| ReporterError::ConfirmationUnavailable)?;
        socket
            .write_all(&token)
            .map_err(|_| ReporterError::ConfirmationUnavailable)?;
        socket
            .shutdown(Shutdown::Write)
            .map_err(|_| ReporterError::ConfirmationUnavailable)?;
        Ok(())
    })();
    token.zeroize();
    result
}

fn validate_credential_directory(path: &Path) -> Result<(), ReporterError> {
    if !path.is_absolute() || path.file_name().is_none() {
        return Err(ReporterError::InvalidCredentialDirectory);
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| ReporterError::InvalidCredentialDirectory)?;
    if canonical != path {
        return Err(ReporterError::InvalidCredentialDirectory);
    }
    let metadata = path
        .symlink_metadata()
        .map_err(|_| ReporterError::InvalidCredentialDirectory)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != effective_uid()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(ReporterError::InvalidCredentialDirectory);
    }
    Ok(())
}

fn read_credential(directory: &Path) -> Result<[u8; 32], ReporterError> {
    let path = directory.join(CREDENTIAL_FILE_NAME);
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| ReporterError::InvalidCredential)?;
    let metadata = file
        .metadata()
        .map_err(|_| ReporterError::InvalidCredential)?;
    if !metadata.is_file()
        || metadata.uid() != effective_uid()
        || metadata.nlink() != 1
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.len() != 32
    {
        return Err(ReporterError::InvalidCredential);
    }
    let mut token = [0_u8; 32];
    if let Err(error) = file.read_exact(&mut token) {
        token.zeroize();
        return Err(ReporterError::Io(error));
    }
    let mut extra = [0_u8; 1];
    match file.read(&mut extra) {
        Ok(0) => Ok(token),
        Ok(_) => {
            token.zeroize();
            Err(ReporterError::InvalidCredential)
        }
        Err(error) => {
            token.zeroize();
            Err(ReporterError::Io(error))
        }
    }
}

fn wait_until_healthy(endpoint: SocketAddr) -> Result<(), ReporterError> {
    for attempt in 0..HEALTH_ATTEMPTS {
        if health_once(endpoint).is_ok() {
            return Ok(());
        }
        if attempt + 1 < HEALTH_ATTEMPTS {
            thread::sleep(HEALTH_RETRY_DELAY);
        }
    }
    Err(ReporterError::CandidateNotReady)
}

fn health_once(endpoint: SocketAddr) -> Result<(), ReporterError> {
    let mut stream = TcpStream::connect_timeout(&endpoint, IO_TIMEOUT)
        .map_err(|_| ReporterError::CandidateNotReady)?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    stream
        .write_all(b"GET /api/health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")?;
    let mut response = Vec::with_capacity(1024);
    let deadline = Instant::now() + IO_TIMEOUT;
    let mut buffer = [0_u8; 1024];
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|duration| !duration.is_zero())
            .ok_or(ReporterError::CandidateNotReady)?;
        stream.set_read_timeout(Some(remaining))?;
        let capacity = buffer.len().min(MAX_RESPONSE_BYTES + 1 - response.len());
        let count = match stream.read(&mut buffer[..capacity]) {
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        };
        if Instant::now() >= deadline {
            return Err(ReporterError::CandidateNotReady);
        }
        if count == 0 {
            break;
        }
        response.extend_from_slice(&buffer[..count]);
        if response.len() > MAX_RESPONSE_BYTES {
            return Err(ReporterError::InvalidHealthResponse);
        }
    }
    validate_health_response(&response)
}

fn validate_health_response(response: &[u8]) -> Result<(), ReporterError> {
    let mut headers = [httparse::EMPTY_HEADER; 32];
    let mut parsed = httparse::Response::new(&mut headers);
    let body_offset = match parsed
        .parse(response)
        .map_err(|_| ReporterError::InvalidHealthResponse)?
    {
        httparse::Status::Complete(offset) => offset,
        httparse::Status::Partial => return Err(ReporterError::InvalidHealthResponse),
    };
    if parsed.code != Some(200)
        || !parsed.headers.iter().any(|header| {
            header.name.eq_ignore_ascii_case("content-type")
                && std::str::from_utf8(header.value)
                    .ok()
                    .is_some_and(|value| value.starts_with("application/json"))
        })
    {
        return Err(ReporterError::InvalidHealthResponse);
    }
    let value: serde_json::Value = serde_json::from_slice(&response[body_offset..])
        .map_err(|_| ReporterError::InvalidHealthResponse)?;
    if value["code"] != 0
        || value["data"]["status"] != "ok"
        || value["data"]["backend"] != "rust"
        || value["data"]["platformId"] != "bli-h616-pikvm"
    {
        return Err(ReporterError::InvalidHealthResponse);
    }
    Ok(())
}

fn effective_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and does not dereference pointers.
    unsafe { libc::geteuid() }
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{self, Permissions},
        net::TcpListener,
        os::unix::{fs::PermissionsExt, net::UnixListener},
        sync::mpsc,
    };

    use super::*;

    #[test]
    fn validates_ready_candidate_then_submits_exact_credential() {
        let root = tempfile::tempdir().unwrap();
        let credentials = root.path().join("credentials");
        fs::create_dir(&credentials).unwrap();
        fs::set_permissions(&credentials, Permissions::from_mode(0o700)).unwrap();
        let token = [73_u8; 32];
        fs::write(credentials.join(CREDENTIAL_FILE_NAME), token).unwrap();
        fs::set_permissions(
            credentials.join(CREDENTIAL_FILE_NAME),
            Permissions::from_mode(0o400),
        )
        .unwrap();

        let health = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let health_address = health.local_addr().unwrap();
        let socket_path = root.path().join("confirm.sock");
        let confirmation = UnixListener::bind(&socket_path).unwrap();
        let (sender, receiver) = mpsc::channel();
        let health_thread = thread::spawn(move || {
            let (mut connection, _) = health.accept().unwrap();
            let mut request = [0_u8; 256];
            let _ = connection.read(&mut request).unwrap();
            let body = br#"{"code":0,"data":{"status":"ok","backend":"rust","platformId":"bli-h616-pikvm"}}"#;
            write!(
                connection,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            connection.write_all(body).unwrap();
        });
        let confirmation_thread = thread::spawn(move || {
            let (mut connection, _) = confirmation.accept().unwrap();
            let mut received = [0_u8; 32];
            connection.read_exact(&mut received).unwrap();
            sender.send(received).unwrap();
        });

        report(
            &ReporterConfig {
                health_endpoint: health_address,
                confirmation_socket: socket_path,
                credential_directory: None,
            },
            &credentials,
        )
        .unwrap();
        assert_eq!(receiver.recv().unwrap(), token);
        health_thread.join().unwrap();
        confirmation_thread.join().unwrap();
    }

    #[test]
    fn explicit_credential_directory_is_confined_to_the_fixed_alias_root() {
        let base = [
            "reporter",
            "--health-endpoint",
            "127.0.0.1:18080",
            "--confirm-socket",
            "/run/confirm.sock",
        ];
        let valid = format!("{CREDENTIAL_ALIAS_ROOT}/candidate.service");
        let parse = |path: &str| {
            parse_arguments(
                base.into_iter()
                    .chain(["--credential-directory", path])
                    .map(OsString::from),
            )
        };
        assert_eq!(
            parse(&valid).unwrap().credential_directory,
            Some(PathBuf::from(&valid))
        );
        for invalid in [
            "/tmp/credentials",
            CREDENTIAL_ALIAS_ROOT,
            "/run/hardened-blikvm-candidate-credentials/../other",
            "/run/hardened-blikvm-candidate-credentials/unit/nested",
        ] {
            assert!(matches!(
                parse(invalid),
                Err(ReporterError::InvalidArguments)
            ));
        }
        assert!(
            parse_arguments(
                base.into_iter()
                    .chain(["--credential-directory", valid.as_str(), "unexpected"])
                    .map(OsString::from)
            )
            .is_err()
        );
        assert!(
            parse_arguments(
                base.into_iter()
                    .chain(["--credential-directory"])
                    .map(OsString::from)
            )
            .is_err()
        );
        assert!(
            parse_arguments(base.into_iter().map(OsString::from))
                .unwrap()
                .credential_directory
                .is_none()
        );
    }

    #[test]
    fn rejects_nonloopback_extra_arguments_and_unsafe_credentials() {
        assert!(matches!(
            parse_arguments([
                "reporter".into(),
                "--health-endpoint".into(),
                "0.0.0.0:18080".into(),
                "--confirm-socket".into(),
                "/run/hardened-blikvm-update-confirm/confirm.sock".into(),
            ]),
            Err(ReporterError::InvalidArguments)
        ));
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), Permissions::from_mode(0o750)).unwrap();
        assert!(matches!(
            validate_credential_directory(root.path()),
            Err(ReporterError::InvalidCredentialDirectory)
        ));
    }

    #[test]
    fn rejects_false_health_even_with_a_valid_token() {
        let root = tempfile::tempdir().unwrap();
        let credentials = root.path().join("credentials");
        fs::create_dir(&credentials).unwrap();
        fs::set_permissions(&credentials, Permissions::from_mode(0o700)).unwrap();
        fs::write(credentials.join(CREDENTIAL_FILE_NAME), [9_u8; 32]).unwrap();
        fs::set_permissions(
            credentials.join(CREDENTIAL_FILE_NAME),
            Permissions::from_mode(0o400),
        )
        .unwrap();
        let response = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"code\":0,\"data\":{\"status\":\"bad\",\"backend\":\"rust\",\"platformId\":\"bli-h616-pikvm\"}}";
        assert!(matches!(
            validate_health_response(response),
            Err(ReporterError::InvalidHealthResponse)
        ));
    }

    #[test]
    fn slow_health_response_cannot_extend_the_read_deadline() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let endpoint = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            let mut request = [0_u8; 256];
            assert!(socket.read(&mut request).unwrap() > 0);
            let response = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"code\":0,\"data\":{\"status\":\"ok\",\"backend\":\"rust\",\"platformId\":\"bli-h616-pikvm\"}}";
            socket.write_all(response).unwrap();
            for _ in 0..20 {
                thread::sleep(Duration::from_millis(100));
                if socket.write_all(b" ").is_err() {
                    break;
                }
            }
        });
        let started = std::time::Instant::now();
        let result = health_once(endpoint);
        let elapsed = started.elapsed();
        server.join().unwrap();
        assert!(
            result.is_err(),
            "a slowly completed response must not confirm health"
        );
        assert!(
            elapsed < Duration::from_millis(1500),
            "elapsed: {elapsed:?}"
        );
    }

    #[test]
    fn health_response_size_limit_includes_headers_and_body() {
        for length in [MAX_RESPONSE_BYTES, MAX_RESPONSE_BYTES + 1] {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
            let endpoint = listener.local_addr().unwrap();
            let server = thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
                let mut request = [0_u8; 256];
                assert!(socket.read(&mut request).unwrap() > 0);
                let mut response = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"code\":0,\"data\":{\"status\":\"ok\",\"backend\":\"rust\",\"platformId\":\"bli-h616-pikvm\"}}".to_vec();
                response.resize(length, b' ');
                socket.write_all(&response).unwrap();
            });
            let result = health_once(endpoint);
            server.join().unwrap();
            assert_eq!(result.is_ok(), length == MAX_RESPONSE_BYTES);
        }
    }

    #[test]
    fn failure_classes_have_stable_nonoverlapping_exit_codes() {
        assert_eq!(ReporterError::InvalidArguments.exit_code(), 2);
        assert_eq!(ReporterError::InvalidCredentialDirectory.exit_code(), 10);
        assert_eq!(ReporterError::InvalidCredential.exit_code(), 11);
        assert_eq!(ReporterError::CandidateNotReady.exit_code(), 12);
        assert_eq!(ReporterError::InvalidHealthResponse.exit_code(), 13);
        assert_eq!(ReporterError::ConfirmationUnavailable.exit_code(), 14);
        assert_eq!(
            ReporterError::Io(io::Error::from(io::ErrorKind::Other)).exit_code(),
            15
        );
    }
}
