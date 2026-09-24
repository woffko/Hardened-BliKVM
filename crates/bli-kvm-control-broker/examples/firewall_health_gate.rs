//! Exercise the production probe against a local test TLS front.
use bli_kvm_control_broker::{
    firewall_health::{CURL, HttpsProbe},
    firewall_settings::FirewallRunner,
};
use std::{path::Path, process::Command};
struct Runner;
impl FirewallRunner for Runner {
    fn run(&mut self, program: &str, args: &[&str]) -> Result<String, String> {
        if program != CURL {
            return Err("unexpected executable".into());
        }
        let output = Command::new(program)
            .args(args)
            .output()
            .map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err("TLS probe process rejected request".into());
        }
        String::from_utf8(output.stdout).map_err(|e| e.to_string())
    }
    fn exists(&mut self, _: &Path) -> bool {
        false
    }
    fn read_file(&mut self, _: &Path) -> Result<String, String> {
        Err("unexpected read".into())
    }
    fn write_file(&mut self, _: &Path, _: &str) -> Result<(), String> {
        Err("unexpected write".into())
    }
    fn remove_file(&mut self, _: &Path) -> Result<(), String> {
        Err("unexpected removal".into())
    }
}
fn main() -> Result<(), String> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 5 || !matches!(args[4].as_str(), "pass" | "fail") {
        return Err("expected name, CA, port, pass/fail".into());
    }
    let probe = HttpsProbe {
        server_name: args[1].clone(),
        ca_certificate: args[2].clone(),
    };
    let result = probe.check(&mut Runner, args[3].parse().map_err(|_| "invalid port")?);
    if result.is_ok() != (args[4] == "pass") {
        return Err(format!("unexpected TLS gate result: {result:?}"));
    }
    Ok(())
}
