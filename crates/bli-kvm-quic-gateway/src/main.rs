use std::process::ExitCode;

use bli_kvm_quic_gateway::{Gateway, config::Config};
use tokio::signal::unix::{SignalKind, signal};

fn main() -> ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(reason) => {
            eprintln!("quic gateway: {reason}");
            return ExitCode::from(2);
        }
    };
    // One thread: a frame's datagrams leave together, and nothing here needs
    // a second core (docs/design/quic-transport-v1.md, phase Q0).
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("quic gateway: no runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(async {
        let gateway = match Gateway::bind(config) {
            Ok(gateway) => gateway,
            Err(reason) => {
                eprintln!("quic gateway: {reason}");
                return ExitCode::FAILURE;
            }
        };
        match gateway.local_addr() {
            Ok(address) => eprintln!("quic gateway: listening on udp {address}"),
            Err(reason) => eprintln!("quic gateway: {reason}"),
        }
        let (mut terminate, mut interrupt) = match (
            signal(SignalKind::terminate()),
            signal(SignalKind::interrupt()),
        ) {
            (Ok(terminate), Ok(interrupt)) => (terminate, interrupt),
            _ => {
                eprintln!("quic gateway: no signal handlers");
                return ExitCode::FAILURE;
            }
        };
        gateway
            .serve(async {
                tokio::select! {
                    _ = terminate.recv() => {}
                    _ = interrupt.recv() => {}
                }
            })
            .await;
        ExitCode::SUCCESS
    })
}
