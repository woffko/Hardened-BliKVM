//! Reversible credential-delivery gate for the HID transition channel.
//!
//! One argument-free entry point: load the KVMD credential delivered by
//! systemd to the fixed HID credential unit. It prints only a fixed status
//! token and never the credential value.

use std::process::ExitCode;

use bli_kvm_hid_credential::load_from_credentials_directory;

fn main() -> ExitCode {
    match load_from_credentials_directory() {
        Ok(credentials) => {
            drop(credentials);
            println!("hid-credential-delivery=ok");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("hid-credential-delivery=failed: {error}");
            ExitCode::FAILURE
        }
    }
}
