//! Board daemon entry point.

use std::{env, path::PathBuf, process::ExitCode};

use bli_kvm_boardd::{run_once, run_serve_until};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

fn serve_signalled(
    serve: impl FnOnce(Arc<AtomicBool>) -> Result<(), String> + Send + 'static,
) -> Result<(), Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        use tokio::signal::unix::{SignalKind, signal};
        // Register both signals before the blocking worker touches hardware.
        let mut terminate = signal(SignalKind::terminate())?;
        let mut interrupt = signal(SignalKind::interrupt())?;
        let stopping = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stopping);
        let mut worker = tokio::task::spawn_blocking(move || serve(worker_stop));
        let result = tokio::select! {
            result = &mut worker => result,
            _ = terminate.recv() => {
                stopping.store(true, Ordering::SeqCst);
                worker.await
            }
            _ = interrupt.recv() => {
                stopping.store(true, Ordering::SeqCst);
                worker.await
            }
        };
        // Wait for the sampler/fan cleanup result, not just signal receipt.
        result
            .map_err(std::io::Error::other)?
            .map_err(std::io::Error::other)
    })?;
    Ok(())
}

fn usage() -> ExitCode {
    eprintln!("usage: bli-kvm-boardd --config <path> [--once]");
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let mut config: Option<PathBuf> = None;
    let mut once = false;
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--config" => config = arguments.next().map(PathBuf::from),
            "--once" => once = true,
            _ => return usage(),
        }
    }
    let Some(config) = config else {
        return usage();
    };
    let result = if once {
        run_once(&config)
    } else {
        serve_signalled(move |stopping| {
            run_serve_until(&config, stopping).map_err(|error| error.to_string())
        })
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bli-kvm-boardd: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };

    #[test]
    fn signal_fixture_child() {
        let Some(ready) = env::var_os("BOARD_DAEMON_SIGNAL_FIXTURE_READY") else {
            return;
        };
        let fail = env::var_os("BOARD_DAEMON_SIGNAL_FIXTURE_FAIL").is_some();
        let result = serve_signalled(move |stop| {
            std::fs::write(ready, b"ready").map_err(|error| error.to_string())?;
            while !stop.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(1));
            }
            if fail {
                Err("fixture cleanup failure".into())
            } else {
                Ok(())
            }
        });
        if fail {
            assert_eq!(result.unwrap_err().to_string(), "fixture cleanup failure");
        } else {
            result.unwrap();
        }
    }

    #[test]
    fn terminate_and_interrupt_wait_for_worker_cleanup() {
        for signal in ["TERM", "INT"] {
            let directory = tempfile::tempdir().unwrap();
            let ready = directory.path().join("ready");
            let mut command = Command::new(env::current_exe().unwrap());
            command
                .args(["--exact", "tests::signal_fixture_child", "--nocapture"])
                .env_clear();
            if signal == "INT" {
                command.env("BOARD_DAEMON_SIGNAL_FIXTURE_FAIL", "1");
            }
            let mut child = command
                .env("BOARD_DAEMON_SIGNAL_FIXTURE_READY", &ready)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(3);
            while !ready.exists() && Instant::now() < deadline {
                assert!(
                    child.try_wait().unwrap().is_none(),
                    "signal fixture exited before ready"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            if !ready.exists() {
                let _ = child.kill();
                let _ = child.wait();
                panic!("signal fixture readiness timeout");
            }
            let sent = Command::new("/usr/bin/kill")
                .args(["-s", signal, &child.id().to_string()])
                .status()
                .unwrap();
            if !sent.success() {
                let _ = child.kill();
                let _ = child.wait();
                panic!("signal send failed");
            }
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                if let Some(status) = child.try_wait().unwrap() {
                    assert!(status.success(), "{signal} failed: {status}");
                    break;
                }
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("signal fixture shutdown timeout");
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}
