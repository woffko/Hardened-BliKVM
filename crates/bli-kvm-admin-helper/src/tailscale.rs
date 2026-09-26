//! Tailscale as in the reference: install, start, stop, up, down, login and
//! logout, and the reference's status view, run as root.
//!
//! Installation uses Tailscale's own apt repository, verified by the
//! repository key shipped in our package, through a private source list and
//! package-list directory so the system's apt configuration is untouched.
//! The login is a detached `tailscale login` whose authentication URL is read
//! back from the daemon's status.

use std::fs;
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use bli_kvm_admin_helper::TailscaleStatus;
use serde::Deserialize;

pub const TAILSCALE: &str = "/usr/bin/tailscale";
pub const TAILSCALED: &str = "/usr/sbin/tailscaled";
const UNIT: &str = "tailscaled.service";
const LOGIN_UNIT: &str = "hardened-blikvm-tailscale-login";
const KEYRING: &str = "/usr/share/hardened-blikvm/tailscale/tailscale-archive-keyring.asc";
const APT_STATE: &str = "/var/lib/hardened-blikvm-tailscale";
const INSTALL_TIMEOUT: Duration = Duration::from_secs(9 * 60);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(45);
const LOGIN_URL_WAIT: Duration = Duration::from_secs(20);

/// Runs a command with a time limit; `Ok(stdout)` on success.
fn run(program: &str, args: &[&str], timeout: Duration) -> io::Result<String> {
    let mut child = Command::new(program)
        .args(args)
        .env("DEBIAN_FRONTEND", "noninteractive")
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let read = |stream: Option<Box<dyn io::Read + Send>>| {
        thread::spawn(move || {
            let mut text = Vec::new();
            if let Some(mut stream) = stream {
                let _ = io::Read::read_to_end(&mut stream, &mut text);
            }
            String::from_utf8_lossy(&text).into_owned()
        })
    };
    let out = read(stdout.map(|s| Box::new(s) as Box<dyn io::Read + Send>));
    let err = read(stderr.map(|s| Box::new(s) as Box<dyn io::Read + Send>));
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other(format!("{program} timed out")));
        }
        thread::sleep(Duration::from_millis(100));
    };
    let stdout = out.join().unwrap_or_default();
    let stderr = err.join().unwrap_or_default();
    if status.success() {
        Ok(stdout)
    } else {
        let reason = stderr.lines().rev().find(|line| !line.trim().is_empty());
        Err(io::Error::other(
            reason.unwrap_or("the command failed").trim().to_string(),
        ))
    }
}

pub fn installed() -> bool {
    Path::new(TAILSCALE).is_file() && Path::new(TAILSCALED).is_file()
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DaemonStatus {
    #[serde(default)]
    backend_state: String,
    #[serde(rename = "AuthURL", default)]
    auth_url: String,
    #[serde(rename = "Self", default)]
    own: Option<Peer>,
    #[serde(default)]
    current_tailnet: Option<Tailnet>,
}

#[derive(Debug, Default, Deserialize)]
struct Peer {
    #[serde(rename = "HostName", default)]
    host_name: String,
    #[serde(rename = "TailscaleIPs", default)]
    addresses: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
struct Tailnet {
    #[serde(rename = "Name", default)]
    name: String,
}

/// `tailscale status --json`, which may print a notice before the JSON.
fn daemon_status() -> io::Result<DaemonStatus> {
    let output = run(TAILSCALE, &["status", "--json"], COMMAND_TIMEOUT)?;
    parse_daemon_status(&output)
}

fn parse_daemon_status(output: &str) -> io::Result<DaemonStatus> {
    let start = output
        .find('{')
        .ok_or_else(|| io::Error::other("unknown output"))?;
    serde_json::from_str(&output[start..]).map_err(|_| io::Error::other("unknown output"))
}

/// The reference's states: notInstall, notRunning, notLogin, stopped, running.
fn view(status: Option<DaemonStatus>) -> TailscaleStatus {
    let Some(status) = status else {
        return TailscaleStatus::state("notRunning");
    };
    let state = match status.backend_state.as_str() {
        "NeedsLogin" | "NeedsMachineAuth" | "InUseOtherUser" => "notLogin",
        "Running" => "running",
        "Stopped" => "stopped",
        _ => "notRunning",
    };
    let own = status.own.unwrap_or_default();
    TailscaleStatus {
        state: state.into(),
        ip: own
            .addresses
            .unwrap_or_default()
            .into_iter()
            .rfind(|address| address.parse::<std::net::Ipv4Addr>().is_ok())
            .unwrap_or_default(),
        name: own.host_name,
        account: status.current_tailnet.unwrap_or_default().name,
    }
}

pub fn status() -> TailscaleStatus {
    if !installed() {
        return TailscaleStatus::state("notInstall");
    }
    view(daemon_status().ok())
}

/// The distribution and release codename the repository is published for.
fn release() -> io::Result<(String, String)> {
    let text = fs::read_to_string("/etc/os-release")?;
    let field = |key: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(&format!("{key}=")))
            .map(|value| value.trim().trim_matches('"').to_string())
            .unwrap_or_default()
    };
    let (id, codename) = (field("ID"), field("VERSION_CODENAME"));
    let known = matches!(
        (id.as_str(), codename.as_str()),
        ("ubuntu", "focal" | "jammy" | "noble") | ("debian", "bullseye" | "bookworm" | "trixie")
    );
    if known {
        Ok((id, codename))
    } else {
        Err(io::Error::other("this system release is not supported"))
    }
}

fn apt_options() -> Vec<String> {
    vec![
        format!("-oDir::Etc::SourceList={APT_STATE}/sources.list"),
        "-oDir::Etc::SourceParts=/nonexistent".into(),
        format!("-oDir::State::Lists={APT_STATE}/lists"),
        "-oAPT::Get::List-Cleanup=0".into(),
    ]
}

pub fn install() -> io::Result<()> {
    if installed() {
        return start();
    }
    let (id, codename) = release()?;
    fs::create_dir_all(format!("{APT_STATE}/lists/partial"))?;
    fs::write(
        format!("{APT_STATE}/sources.list"),
        format!(
            "deb [signed-by={KEYRING}] https://pkgs.tailscale.com/stable/{id} {codename} main\n"
        ),
    )?;
    let options = apt_options();
    let mut update: Vec<&str> = options.iter().map(String::as_str).collect();
    update.push("update");
    run("/usr/bin/apt-get", &update, INSTALL_TIMEOUT)?;
    let mut install: Vec<&str> = options.iter().map(String::as_str).collect();
    install.extend(["-y", "--no-install-recommends", "install", "tailscale"]);
    run("/usr/bin/apt-get", &install, INSTALL_TIMEOUT)?;
    start()
}

pub fn uninstall() -> io::Result<()> {
    let _ = stop();
    run(
        "/usr/bin/apt-get",
        &["-y", "remove", "tailscale"],
        INSTALL_TIMEOUT,
    )?;
    Ok(())
}

pub fn start() -> io::Result<()> {
    run(
        "/usr/bin/systemctl",
        &["enable", "--now", UNIT],
        COMMAND_TIMEOUT,
    )
    .map(|_| ())
}

pub fn stop() -> io::Result<()> {
    let _ = run("/usr/bin/systemctl", &["stop", LOGIN_UNIT], COMMAND_TIMEOUT);
    run(
        "/usr/bin/systemctl",
        &["disable", "--now", UNIT],
        COMMAND_TIMEOUT,
    )
    .map(|_| ())
}

pub fn restart() -> io::Result<()> {
    run("/usr/bin/systemctl", &["restart", UNIT], COMMAND_TIMEOUT).map(|_| ())
}

pub fn up() -> io::Result<()> {
    run(
        TAILSCALE,
        &["up", "--accept-dns=false", "--timeout=30s"],
        COMMAND_TIMEOUT,
    )
    .map(|_| ())
}

pub fn down() -> io::Result<()> {
    run(TAILSCALE, &["down"], COMMAND_TIMEOUT).map(|_| ())
}

pub fn logout() -> io::Result<()> {
    run(TAILSCALE, &["logout"], COMMAND_TIMEOUT).map(|_| ())
}

/// The authentication URL, or an empty string when already logged in.
pub fn login() -> io::Result<String> {
    if !installed() {
        return Err(io::Error::other("tailscale is not installed"));
    }
    let current = match daemon_status() {
        Ok(status) => status,
        Err(_) => {
            start()?;
            daemon_status()?
        }
    };
    if current.backend_state == "Running" {
        return Ok(String::new());
    }
    if !current.auth_url.is_empty() {
        return Ok(current.auth_url);
    }
    // The login waits for the operator in its own transient unit, so it
    // outlives this helper session.
    let _ = run(
        "/usr/bin/systemd-run",
        &[
            "--quiet",
            "--collect",
            &format!("--unit={LOGIN_UNIT}"),
            "--property=RuntimeMaxSec=660",
            TAILSCALE,
            "login",
            "--accept-dns=false",
            "--timeout=10m",
        ],
        COMMAND_TIMEOUT,
    );
    let started = Instant::now();
    while started.elapsed() < LOGIN_URL_WAIT {
        if let Ok(status) = daemon_status() {
            if status.backend_state == "Running" {
                return Ok(String::new());
            }
            if !status.auth_url.is_empty() {
                return Ok(status.auth_url);
            }
        }
        thread::sleep(Duration::from_millis(500));
    }
    Err(io::Error::other("tailscale did not provide a login URL"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_states_map_to_the_reference_view() {
        let json = r#"Warning: client version differs
{"BackendState":"Running","AuthURL":"","Self":{"HostName":"kvm","TailscaleIPs":["100.64.0.7","fd7a:115c:a1e0::7"]},"CurrentTailnet":{"Name":"example.org"}}"#;
        let status = view(parse_daemon_status(json).ok());
        assert_eq!(status.state, "running");
        assert_eq!(status.ip, "100.64.0.7");
        assert_eq!(status.name, "kvm");
        assert_eq!(status.account, "example.org");
        let needs = r#"{"BackendState":"NeedsLogin","AuthURL":"https://login.tailscale.com/a/x","Self":{"HostName":"kvm","TailscaleIPs":null},"CurrentTailnet":null}"#;
        let parsed = parse_daemon_status(needs).unwrap();
        assert_eq!(parsed.auth_url, "https://login.tailscale.com/a/x");
        assert_eq!(view(Some(parsed)).state, "notLogin");
        assert_eq!(view(None).state, "notRunning");
        assert!(parse_daemon_status("no json").is_err());
    }
}
