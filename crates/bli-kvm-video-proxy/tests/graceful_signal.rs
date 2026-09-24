use std::{
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    time::{sleep, timeout},
};

struct ReapedChild(Child);
impl Drop for ReapedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn real_proxy_handles_both_signals_with_idle_request_and_clean_exit() {
    for (signal, replaced) in [("TERM", false), ("INT", false), ("TERM", true)] {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("proxy.sock");
        let mut child = ReapedChild(
            Command::new(env!("CARGO_BIN_EXE_bli-kvm-video-proxy"))
                .env_clear()
                .env("BLI_KVM_VIDEO_PROXY_SOCKET", &socket)
                .env(
                    "BLI_KVM_USTREAMER_SOCKET",
                    directory.path().join("absent-upstream.sock"),
                )
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        while !socket.exists() {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "proxy exited before binding"
            );
            assert!(Instant::now() < deadline, "proxy bind deadline expired");
            sleep(Duration::from_millis(5)).await;
        }
        let mut client = UnixStream::connect(&socket).await.unwrap();
        client.write_all(b"GET ").await.unwrap();
        if replaced {
            std::fs::rename(&socket, directory.path().join("original.sock")).unwrap();
            std::fs::write(&socket, b"preserve replacement").unwrap();
        }
        assert!(
            Command::new("/usr/bin/kill")
                .args(["-s", signal, &child.0.id().to_string()])
                .status()
                .unwrap()
                .success()
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                assert_eq!(
                    status.success(),
                    !replaced,
                    "{signal} cleanup result: {status}"
                );
                break;
            }
            assert!(Instant::now() < deadline, "proxy drain deadline expired");
            sleep(Duration::from_millis(5)).await;
        }
        if replaced {
            assert_eq!(std::fs::read(&socket).unwrap(), b"preserve replacement");
        } else {
            assert!(!socket.exists());
        }
        let mut byte = [0_u8; 1];
        let closed = timeout(Duration::from_secs(1), client.read(&mut byte))
            .await
            .unwrap();
        assert!(
            matches!(closed, Ok(0))
                || closed.is_err_and(|error| error.kind() == std::io::ErrorKind::ConnectionReset),
            "proxy retained connection"
        );
    }
}
