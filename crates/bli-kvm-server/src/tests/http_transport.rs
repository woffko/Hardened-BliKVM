use super::*;
use std::process::{Child, Command, Stdio};
use tokio::net::{TcpListener, UnixStream};
use tokio::time::{Duration, sleep, timeout};

struct OwnedNginx(Child);
impl Drop for OwnedNginx {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

// Explicitly opt-in: uses an unpacked local binary, not a system daemon. All
// sockets, account data and proxy temp files live in one disposable directory.
#[tokio::test]
#[ignore = "set BLI_KVM_TEST_NGINX to a reviewed local nginx executable"]
async fn nginx_buffering_logout_transport_diagnostic() {
    let binary = std::env::var_os("BLI_KVM_TEST_NGINX").expect("explicit nginx path required");
    for buffering in [true, false] {
        timeout(Duration::from_secs(15), async {
            const BODY: usize = 8 * 1024 * 1024;
            const FIRST: usize = 32768;
            let root = tempdir().unwrap();
            let worker = UnixListener::bind(root.path().join("upload.sock")).unwrap();
            let app = app_with_options(ServerOptions {
                state_dir: root.path().join("account"),
                update_status_socket: Some(root.path().join("status.sock")),
                update_upload_socket: Some(root.path().join("upload.sock")),
                update_activation_socket: Some(root.path().join("activation.sock")),
                update_writes_enabled: true,
                ..ServerOptions::default()
            });
            let (cookie, csrf) = setup_and_login(&app).await;
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let upstream = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
                    .await.unwrap();
            });
            let proxy = root.path().join("proxy.sock");
            let config = root.path().join("nginx.conf");
            let mode = if buffering { "on" } else { "off" };
            std::fs::write(&config, format!(
                "daemon off; master_process off; pid {0}/nginx.pid; error_log {0}/error.log info;\n\
                 events {{ worker_connections 32; }}\n\
                 http {{ access_log off; client_body_temp_path {0}/body; proxy_temp_path {0}/proxy;\n\
                 fastcgi_temp_path {0}/fastcgi; uwsgi_temp_path {0}/uwsgi; scgi_temp_path {0}/scgi;\n\
                 server {{ listen unix:{1}; client_max_body_size 16m; client_body_buffer_size 8k;\n\
                 location / {{ proxy_pass http://{2}; proxy_http_version 1.1;\n\
                 proxy_set_header Host localhost; proxy_set_header Connection close;\n\
                 proxy_buffering off; proxy_request_buffering {3}; }} }} }}\n",
                root.path().display(), proxy.display(), upstream, mode
            )).unwrap();
            let mut nginx = OwnedNginx(Command::new(&binary)
                .args(["-p", root.path().to_str().unwrap(), "-c", config.to_str().unwrap()])
                .stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
            let mut upload = loop {
                assert!(nginx.0.try_wait().unwrap().is_none(), "local nginx failed startup");
                match UnixStream::connect(&proxy).await {
                    Ok(stream) => break stream,
                    Err(_) => sleep(Duration::from_millis(10)).await,
                }
            };
            println!("request_buffering={mode}, phase=proxy-ready");
            upload.write_all(format!(
                "POST /api/application-update/upload HTTP/1.1\r\nHost: localhost\r\nOrigin: https://localhost\r\nCookie: {cookie}\r\nx-csrf-token: {csrf}\r\nContent-Type: application/octet-stream\r\nContent-Length: {BODY}\r\nConnection: close\r\n\r\n"
            ).as_bytes()).await.unwrap();
            upload.write_all(&vec![b'a'; FIRST]).await.unwrap();
            let mut ipc = if buffering {
                assert!(timeout(Duration::from_millis(150), worker.accept()).await.is_err(),
                    "buffered request reached worker before its complete body");
                None
            } else {
                let (mut stream, _) = worker.accept().await.unwrap();
                let length = stream.read_u32().await.unwrap() as usize;
                assert!(length < 65536);
                stream.read_exact(&mut vec![0; length]).await.unwrap();
                let mut prefix = vec![0; FIRST];
                stream.read_exact(&mut prefix).await.unwrap();
                assert!(prefix.iter().all(|byte| *byte == b'a'));
                Some(stream)
            };
            let mut logout = UnixStream::connect(&proxy).await.unwrap();
            println!("request_buffering={mode}, phase=partial-body");
            logout.write_all(format!(
                "POST /api/auth/logout HTTP/1.1\r\nHost: localhost\r\nOrigin: https://localhost\r\nCookie: {cookie}\r\nx-csrf-token: {csrf}\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
            ).as_bytes()).await.unwrap();
            let mut result = Vec::new();
            logout.read_to_end(&mut result).await.unwrap();
            assert!(result.starts_with(b"HTTP/1.1 200"));
            println!("request_buffering={mode}, phase=logout-completed");
            let _ = upload.write_all(&vec![b'b'; BODY - FIRST]).await;
            result.clear();
            let _ = upload.read_to_end(&mut result).await;
            let status = if result.starts_with(b"HTTP/1.1 401") { 401 }
                else if result.starts_with(b"HTTP/1.1 502") { 502 } else { 0 };
            println!("request_buffering={mode}, upload_status={status}, initial_worker_contact={}", !buffering);
            if buffering {
                assert!([401, 502].contains(&status), "unexpected buffered response");
                assert!(timeout(Duration::from_millis(50), worker.accept()).await.is_err());
            } else {
                assert_eq!(status, 401, "streaming proxy must expose revocation");
                let mut remaining = Vec::new();
                ipc.as_mut().unwrap().read_to_end(&mut remaining).await.unwrap();
                assert!(remaining.is_empty(), "bytes reached worker after logout");
            }
            server.abort();
        }).await.expect("bounded local nginx fixture timed out");
    }
}
