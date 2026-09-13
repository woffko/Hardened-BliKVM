//! Argument-free entry point for the live ATX waiting-route gate.
//!
//! The KVMD credential is delivered by systemd through `CREDENTIALS_DIRECTORY`
//! and never appears in the environment or arguments.

use std::{
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

fn read_credential(directory: &Path, name: &str) -> Option<Vec<u8>> {
    let mut bytes = fs::read(directory.join(name)).ok()?;
    while matches!(bytes.last(), Some(b'\n' | b'\r')) {
        bytes.pop();
    }
    if bytes.is_empty() { None } else { Some(bytes) }
}

#[tokio::main]
async fn main() -> ExitCode {
    let Some(directory) = std::env::var_os("CREDENTIALS_DIRECTORY").map(PathBuf::from) else {
        eprintln!("atx-live-gate=no-credentials");
        return ExitCode::FAILURE;
    };
    let Some(username) = read_credential(&directory, "kvmd-user") else {
        eprintln!("atx-live-gate=no-username");
        return ExitCode::FAILURE;
    };
    let Some(password) = read_credential(&directory, "kvmd-password") else {
        eprintln!("atx-live-gate=no-password");
        return ExitCode::FAILURE;
    };
    let journal_dir = std::env::temp_dir().join("bli-kvm-atx-live-gate");
    if fs::create_dir_all(&journal_dir).is_err() {
        eprintln!("atx-live-gate=no-journal");
        return ExitCode::FAILURE;
    }
    let journal = journal_dir.join("journal.json");
    match bli_kvm_atx_live_gate::run_live_checks(
        Path::new("/run/kvmd/kvmd.sock"),
        &journal,
        &username,
        &password,
        bli_kvm_atx_live_gate::unix_millis(),
    )
    .await
    {
        Ok(facts) => {
            for (key, value) in facts {
                println!("{key}={value}");
            }
            let _ = fs::remove_dir_all(&journal_dir);
            ExitCode::SUCCESS
        }
        Err(_) => {
            eprintln!("atx-live-gate=failed");
            let _ = fs::remove_dir_all(&journal_dir);
            ExitCode::FAILURE
        }
    }
}
