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

struct OwnedTask<T>(tokio::task::JoinHandle<T>);
impl<T> Drop for OwnedTask<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires reviewed TLS nginx and locally installed Playwright"]
async fn nginx_tls_browser_logout_transport_diagnostic() {
    timeout(Duration::from_secs(60), async {
        let binary = std::env::var_os("BLI_KVM_TEST_NGINX").expect("explicit nginx path required");
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
        let app = app.route("/transport-probe", axum::routing::get(|| async {
            axum::response::Html("<!doctype html><title>Local transport probe</title>")
        }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = listener.local_addr().unwrap();
        let _server = OwnedTask(tokio::spawn(async move {
            axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
        }));
        // Drain the synthetic worker until cancellation, without publishing or
        // accepting an archive. Record only a byte count, never request bytes.
        let mut worker_task = OwnedTask(tokio::spawn(async move {
            let (mut stream, _) = worker.accept().await.unwrap();
            let header_length = stream.read_u32().await.unwrap() as usize;
            assert!(header_length < 65536);
            stream.read_exact(&mut vec![0; header_length]).await.unwrap();
            tokio::io::copy(&mut stream, &mut tokio::io::sink()).await.unwrap()
        }));
        let port_probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = port_probe.local_addr().unwrap();
        drop(port_probe);
        let cert = root.path().join("cert.pem");
        let key = root.path().join("key.pem");
        assert!(Command::new("openssl").args([
            "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
            "-subj", "/CN=localhost", "-keyout", key.to_str().unwrap(),
            "-out", cert.to_str().unwrap(),
        ]).stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap().success());
        let config = root.path().join("nginx.conf");
        std::fs::write(&config, format!(
            "daemon off; master_process off; pid {0}/nginx.pid; error_log {0}/error.log info;\n\
             events {{ worker_connections 64; }}\n\
             http {{ access_log off; client_body_temp_path {0}/body; proxy_temp_path {0}/proxy;\n\
             fastcgi_temp_path {0}/fastcgi; uwsgi_temp_path {0}/uwsgi; scgi_temp_path {0}/scgi;\n\
             server {{ listen {1} ssl; ssl_certificate {0}/cert.pem; ssl_certificate_key {0}/key.pem;\n\
             client_max_body_size 16m;\n\
             location / {{ proxy_pass http://{2}; proxy_http_version 1.1;\n\
             proxy_set_header Host $http_host; proxy_set_header Connection close;\n\
             proxy_set_header X-Forwarded-Proto https; proxy_buffering off; }}\n\
             location = /api/application-update/upload {{ proxy_pass http://{2}; proxy_http_version 1.1;\n\
             proxy_set_header Host $http_host; proxy_set_header Connection close;\n\
             proxy_set_header X-Forwarded-Proto https; proxy_buffering off; proxy_request_buffering off; }} }} }}\n",
            root.path().display(), address, upstream
        )).unwrap();
        let mut nginx = OwnedNginx(Command::new(binary)
            .args(["-p", root.path().to_str().unwrap(), "-c", config.to_str().unwrap()])
            .stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
        loop {
            assert!(nginx.0.try_wait().unwrap().is_none(), "TLS nginx startup failed");
            if tokio::net::TcpStream::connect(address).await.is_ok() { break; }
            sleep(Duration::from_millis(10)).await;
        }
        let input = root.path().join("browser.json");
        std::fs::write(&input, serde_json::to_vec(&serde_json::json!({
            "origin": format!("https://{address}"), "cookie": cookie, "csrf": csrf,
            "proxyPid": nginx.0.id(),
        })).unwrap()).unwrap();
        let mut browser = OwnedNginx(Command::new("node")
            .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools/playwright/local_logout_transport.mjs"))
            .arg(input).spawn().unwrap());
        let status = loop {
            if let Some(status) = browser.0.try_wait().unwrap() { break status; }
            sleep(Duration::from_millis(50)).await;
        };
        let log = std::fs::read_to_string(root.path().join("error.log")).unwrap();
        println!("tls_proxy_writev_errors={}, tls_proxy_upstream_send_errors={}",
            log.matches("writev() failed").count(), log.matches("while sending request to upstream").count());
        assert!(status.success(), "local browser transport failed");
        let received = timeout(Duration::from_secs(2), &mut worker_task.0).await.unwrap().unwrap();
        assert!(received > 0 && received < 8_724_480, "worker must see only a partial upload");
    }).await.expect("bounded TLS browser fixture timed out");
}

// Explicitly opt-in: uses an unpacked local binary, not a system daemon. All
// sockets, account data and proxy temp files live in one disposable directory.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "set BLI_KVM_TEST_NGINX to a reviewed local nginx executable"]
async fn nginx_buffering_logout_transport_diagnostic() {
    let binary = std::env::var_os("BLI_KVM_TEST_NGINX").expect("explicit nginx path required");
    for (buffering, pressure, continuous, revoked_before_dispatch) in [
        (true, 0, false, false),
        (false, 0, false, false),
        (false, 1024 * 1024, false, false),
        (false, 1024 * 1024, true, false),
        (false, 0, false, true),
    ] {
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
            let _server = OwnedTask(tokio::spawn(async move {
                axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
                    .await.unwrap();
            }));
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
            let logout_request = format!(
                "POST /api/auth/logout HTTP/1.1\r\nHost: localhost\r\nOrigin: https://localhost\r\nCookie: {cookie}\r\nx-csrf-token: {csrf}\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
            );
            if revoked_before_dispatch {
                let mut logout = UnixStream::connect(&proxy).await.unwrap();
                logout.write_all(logout_request.as_bytes()).await.unwrap();
                let mut result = Vec::new();
                logout.read_to_end(&mut result).await.unwrap();
                assert!(result.starts_with(b"HTTP/1.1 200"));
            }
            upload.write_all(format!(
                "POST /api/application-update/upload HTTP/1.1\r\nHost: localhost\r\nOrigin: https://localhost\r\nCookie: {cookie}\r\nx-csrf-token: {csrf}\r\nContent-Type: application/octet-stream\r\nContent-Length: {BODY}\r\nConnection: close\r\n\r\n"
            ).as_bytes()).await.unwrap();
            upload.write_all(&vec![b'a'; FIRST]).await.unwrap();
            let mut ipc = if buffering || revoked_before_dispatch {
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
            if pressure != 0 {
                // The synthetic worker stops reading after FIRST. Queue more
                // authenticated body data so revocation races a blocked IPC
                // write and an upstream socket with unread request bytes.
                upload.write_all(&vec![b'b'; pressure]).await.unwrap();
                sleep(Duration::from_millis(100)).await;
            }
            let (mut upload_read, mut upload_write) = upload.into_split();
            let mut writer = OwnedTask(tokio::spawn(async move {
                if !continuous {
                    return upload_write;
                }
                // Keep the proxy writing upstream while Logout revokes the
                // session, rather than resuming only after Logout responds.
                let _ = upload_write.write_all(&vec![b'b'; BODY - FIRST - pressure]).await;
                upload_write
            }));
            println!("request_buffering={mode}, phase=partial-body");
            let mut result = Vec::new();
            if !revoked_before_dispatch {
                logout.write_all(logout_request.as_bytes()).await.unwrap();
                logout.read_to_end(&mut result).await.unwrap();
                assert!(result.starts_with(b"HTTP/1.1 200"));
            }
            println!("request_buffering={mode}, phase=logout-completed");
            let mut upload_write = (&mut writer.0).await.unwrap();
            if !continuous {
                let _ = upload_write.write_all(&vec![b'c'; BODY - FIRST - pressure]).await;
            }
            result.clear();
            let _ = upload_read.read_to_end(&mut result).await;
            let status = if result.starts_with(b"HTTP/1.1 401") { 401 }
                else if result.starts_with(b"HTTP/1.1 502") { 502 } else { 0 };
            println!("request_buffering={mode}, pressure_bytes={pressure}, continuous={continuous}, revoked_before_dispatch={revoked_before_dispatch}, upload_status={status}, initial_worker_contact={}", ipc.is_some());
            if buffering {
                assert!([401, 502].contains(&status), "unexpected buffered response");
                assert!(timeout(Duration::from_millis(50), worker.accept()).await.is_err());
            } else {
                assert_eq!(status, 401, "streaming proxy must expose revocation");
                if revoked_before_dispatch {
                    assert!(timeout(Duration::from_millis(50), worker.accept()).await.is_err());
                    return;
                }
                let mut remaining = Vec::new();
                ipc.as_mut().unwrap().read_to_end(&mut remaining).await.unwrap();
                let allowed = if continuous { BODY - FIRST } else { pressure };
                assert!(remaining.iter().all(|byte| *byte == b'b') && remaining.len() <= allowed,
                    "post-logout body bytes reached worker");
            }
        }).await.expect("bounded local nginx fixture timed out");
    }
}
