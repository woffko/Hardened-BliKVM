//! Reviewed HTTPS redirect and certificate view for the board owner.
//!
//! The interface toggles HTTPS by switching the HTTP listener to a redirect.
//! The reviewed nginx configuration is one root-only file, so the owner edits
//! exactly that file with a marker block, tests the configuration, reloads the
//! service and restores the previous file when either step fails. The
//! certificate view runs `openssl` on the configured public certificate; no
//! private key is read or printed.

use std::path::{Path, PathBuf};
use std::process::Command;

use bli_kvm_control_protocol::{ControlRejectionCode, ControlResponse};

pub const NGINX_CONF: &str = "/etc/hardened-blikvm/nginx.conf";
pub const NGINX_UNIT: &str = "hardened-blikvm-nginx.service";
const NGINX: &str = "/usr/sbin/nginx";
const OPENSSL: &str = "/usr/bin/openssl";
const SYSTEMCTL: &str = "/usr/bin/systemctl";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CertificateView {
    pub subject: String,
    pub issuer: String,
    pub not_after: String,
    pub fingerprint_sha256: String,
}

/// Runs the reviewed commands and reads/writes the configuration. Injected so
/// a test asserts the marker edit, the reload and the failure path.
pub trait TlsRunner: Send {
    fn read_conf(&mut self, path: &Path) -> Result<String, String>;
    fn write_conf(&mut self, path: &Path, text: &str) -> Result<(), String>;
    fn run(&mut self, program: &str, args: &[&str]) -> Result<String, String>;
}

pub struct SystemTlsRunner;

impl TlsRunner for SystemTlsRunner {
    fn read_conf(&mut self, path: &Path) -> Result<String, String> {
        std::fs::read_to_string(path).map_err(|error| error.to_string())
    }

    fn write_conf(&mut self, path: &Path, text: &str) -> Result<(), String> {
        let parent = path
            .parent()
            .ok_or_else(|| "the configuration has no parent directory".to_string())?;
        let metadata = std::fs::symlink_metadata(parent)
            .map_err(|error| format!("the configuration directory is unavailable: {error}"))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err("the configuration directory must be a real directory".into());
        }
        let temporary = path.with_extension("conf.new");
        std::fs::write(&temporary, text).map_err(|error| {
            format!("the temporary configuration could not be written: {error}")
        })?;
        std::fs::set_permissions(
            &temporary,
            std::os::unix::fs::PermissionsExt::from_mode(0o600),
        )
        .map_err(|error| format!("the configuration mode could not be set: {error}"))?;
        std::fs::rename(&temporary, path)
            .map_err(|error| format!("the configuration could not be replaced: {error}"))
    }

    fn run(&mut self, program: &str, args: &[&str]) -> Result<String, String> {
        let output = Command::new(program)
            .args(args)
            .output()
            .map_err(|error| error.to_string())?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

pub struct TlsState {
    conf_path: PathBuf,
    unit: String,
    runner: Box<dyn TlsRunner>,
    capability_enabled: bool,
}

impl TlsState {
    pub fn open(
        conf_path: impl Into<PathBuf>,
        unit: impl Into<String>,
        runner: Box<dyn TlsRunner>,
        capability_enabled: bool,
    ) -> Self {
        Self {
            conf_path: conf_path.into(),
            unit: unit.into(),
            runner,
            capability_enabled,
        }
    }

    pub fn enabled(&mut self) -> Result<bool, String> {
        Ok(parse_conf(&self.runner.read_conf(&self.conf_path)?).0)
    }

    /// Reads the public certificate the configuration points at.
    pub fn certificate(&mut self) -> Result<CertificateView, String> {
        let text = self.runner.read_conf(&self.conf_path)?;
        let Some(path) = certificate_path(&text) else {
            return Err("the configuration names no certificate".into());
        };
        let output = self.runner.run(
            OPENSSL,
            &[
                "x509",
                "-in",
                &path,
                "-noout",
                "-subject",
                "-issuer",
                "-enddate",
                "-fingerprint",
                "-sha256",
            ],
        )?;
        Ok(parse_openssl(&output))
    }

    pub fn set(&mut self, enabled: bool) -> ControlResponse {
        if !self.capability_enabled {
            return reject(ControlRejectionCode::CapabilityDisabled);
        }
        let previous = match self.runner.read_conf(&self.conf_path) {
            Ok(text) => text,
            Err(reason) => {
                eprintln!("tls: configuration read failed: {reason}");
                return reject(ControlRejectionCode::BackendRejected);
            }
        };
        let (current, next) = parse_conf(&previous);
        if current == enabled {
            return ControlResponse::Applied;
        }
        let _ = next;
        let rendered = render_conf(&previous, enabled);
        if let Err(reason) = self.runner.write_conf(&self.conf_path, &rendered) {
            eprintln!("tls: configuration write failed: {reason}");
            return reject(ControlRejectionCode::BackendRejected);
        }
        if let Err(reason) = self
            .runner
            .run(NGINX, &["-t", "-c", &self.conf_path.to_string_lossy()])
        {
            eprintln!("tls: configuration test failed: {reason}");
            if let Err(restore) = self.runner.write_conf(&self.conf_path, &previous) {
                eprintln!("tls: configuration restore failed: {restore}");
            }
            return reject(ControlRejectionCode::BackendRejected);
        }
        if let Err(reason) = self.runner.run(SYSTEMCTL, &["reload", &self.unit]) {
            eprintln!("tls: reload failed: {reason}");
            let _ = self.runner.write_conf(&self.conf_path, &previous);
            let _ = self
                .runner
                .run(NGINX, &["-t", "-c", &self.conf_path.to_string_lossy()]);
            return reject(ControlRejectionCode::BackendRejected);
        }
        ControlResponse::Applied
    }
}

/// Whether the listener on port 80 redirects to HTTPS: the marker block this
/// module writes, or the `return 30x https://` the shipped configuration has.
/// The text is returned unchanged for the caller's rewrite.
fn parse_conf(text: &str) -> (bool, String) {
    (
        bli_kvm_core::nginx_redirect::redirects_port_80(text),
        text.to_string(),
    )
}

fn render_conf(text: &str, enabled: bool) -> String {
    bli_kvm_core::nginx_redirect::render_redirect(text, enabled)
}

fn certificate_path(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.trim().strip_prefix("ssl_certificate "))
        .map(|value| value.trim().trim_end_matches(';').to_string())
}

fn parse_openssl(output: &str) -> CertificateView {
    let mut view = CertificateView::default();
    for line in output.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("subject=") {
            view.subject = value.trim().to_string();
        } else if let Some(value) = line.strip_prefix("issuer=") {
            view.issuer = value.trim().to_string();
        } else if let Some(value) = line.strip_prefix("notAfter=") {
            view.not_after = value.trim().to_string();
        } else if let Some((label, value)) = line.split_once('=')
            // OpenSSL 1.1 prints "SHA256 Fingerprint=", OpenSSL 3 "sha256 Fingerprint=".
            && label.eq_ignore_ascii_case("SHA256 Fingerprint")
        {
            view.fingerprint_sha256 = value.trim().to_string();
        }
    }
    view
}

fn reject(code: ControlRejectionCode) -> ControlResponse {
    ControlResponse::Rejected { code }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    const BASE: &str = "events {}\nhttp {\n    server {\n        listen 80 default_server;\n        listen [::]:80 default_server;\n        server_name _;\n    }\n    server {\n        listen 443 ssl default_server;\n        ssl_certificate /etc/kvmd/nginx/ssl/server.crt;\n    }\n}\n";

    #[derive(Default)]
    struct RecordingRunner {
        conf: String,
        fail_test: bool,
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl TlsRunner for RecordingRunner {
        fn read_conf(&mut self, _path: &Path) -> Result<String, String> {
            Ok(self.conf.clone())
        }

        fn write_conf(&mut self, _path: &Path, text: &str) -> Result<(), String> {
            self.calls.lock().unwrap().push("write".into());
            self.conf = text.to_string();
            Ok(())
        }

        fn run(&mut self, program: &str, args: &[&str]) -> Result<String, String> {
            let call = format!("{program} {}", args.join(" "));
            self.calls.lock().unwrap().push(call.clone());
            if self.fail_test && call.contains("-t ") {
                return Err("syntax error".into());
            }
            if program == OPENSSL {
                return Ok("subject=CN = mangopimcore\nissuer=CN = mangopimcore\nnotAfter=Sep 18 12:00:00 2036 GMT\nSHA256 Fingerprint=AA:BB\n".into());
            }
            Ok(String::new())
        }
    }

    fn fixture(conf: &str, fail_test: bool) -> (TlsState, Arc<Mutex<Vec<String>>>) {
        let runner = RecordingRunner {
            conf: conf.into(),
            fail_test,
            ..Default::default()
        };
        let calls = runner.calls.clone();
        (
            TlsState::open(NGINX_CONF, NGINX_UNIT, Box::new(runner), true),
            calls,
        )
    }

    #[test]
    fn the_redirect_toggles_only_inside_the_http_listener() {
        let enabled = render_conf(BASE, true);
        assert!(enabled.contains("return 301 https://$host$request_uri;"));
        assert_eq!(enabled.matches("return 301").count(), 1);
        // The HTTPS block keeps its certificate and gains no redirect.
        let https_block = enabled.split("listen 443").nth(1).unwrap();
        assert!(!https_block.contains("return 301"));
        assert!(parse_conf(&enabled).0);

        let disabled = render_conf(&enabled, false);
        assert!(!disabled.contains("return 301"));
        assert!(!parse_conf(&disabled).0);
        // Idempotent: rendering twice changes nothing.
        assert_eq!(render_conf(&disabled, false), disabled);
    }

    #[test]
    fn the_shipped_redirect_counts_as_https_being_on() {
        let shipped = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../packaging/nginx/hardened-blikvm.conf"),
        )
        .unwrap();
        assert!(parse_conf(&shipped).0, "the shipped conf redirects port 80");
        // A redirect in the HTTPS block alone is not one on port 80.
        let only_https = "http {\n    server {\n        listen 80;\n    }\n    server {\n        listen 443 ssl;\n        return 301 https://$host$request_uri;\n    }\n}\n";
        assert!(!parse_conf(only_https).0);
        // A commented-out redirect does not count.
        let commented = "http {\n    server {\n        listen 80;\n        # return 308 https://$host$request_uri;\n    }\n}\n";
        assert!(!parse_conf(commented).0);
    }

    #[test]
    fn enabling_reloads_and_fails_closed() {
        let (mut state, calls) = fixture(BASE, false);
        assert_eq!(state.set(true), ControlResponse::Applied);
        let calls = calls.lock().unwrap().clone();
        assert!(calls.iter().any(|call| call.contains("-t")));
        assert!(
            calls
                .iter()
                .any(|call| call.ends_with("reload hardened-blikvm-nginx.service"))
        );

        // A failing configuration test restores the previous file.
        let (mut failing, calls) = fixture(BASE, true);
        assert_eq!(
            failing.set(true),
            ControlResponse::Rejected {
                code: ControlRejectionCode::BackendRejected
            }
        );
        let calls = calls.lock().unwrap().clone();
        assert_eq!(calls.iter().filter(|call| *call == "write").count(), 2);
        assert!(!calls.iter().any(|call| call.contains("reload")));
    }

    #[test]
    fn reads_the_public_certificate_view() {
        let (mut state, _) = fixture(BASE, false);
        let view = state.certificate().unwrap();
        assert_eq!(view.subject, "CN = mangopimcore");
        assert_eq!(view.not_after, "Sep 18 12:00:00 2036 GMT");
        assert_eq!(view.fingerprint_sha256, "AA:BB");
        // OpenSSL 3 spells the digest in lowercase.
        assert_eq!(
            parse_openssl("sha256 Fingerprint=CC:DD\n").fingerprint_sha256,
            "CC:DD"
        );
        assert!(!state.enabled().unwrap());
    }

    #[test]
    fn a_disabled_capability_never_touches_the_configuration() {
        let runner = RecordingRunner {
            conf: BASE.into(),
            ..Default::default()
        };
        let calls = runner.calls.clone();
        let mut state = TlsState::open(NGINX_CONF, NGINX_UNIT, Box::new(runner), false);
        assert_eq!(
            state.set(true),
            ControlResponse::Rejected {
                code: ControlRejectionCode::CapabilityDisabled
            }
        );
        assert!(calls.lock().unwrap().is_empty());
    }
}
