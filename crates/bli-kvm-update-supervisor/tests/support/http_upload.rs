//! Full host route -> streaming client -> real CLI -> signed stager boundary.
//! Uses only synthetic archives and disposable unprivileged directories.
use super::*;
use axum::{
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::{Request, StatusCode, header},
};
use bli_kvm_server::{ServerOptions, app_with_options};
use std::{net::SocketAddr, os::fd::OwnedFd, time::Duration};
use tokio::net::UnixListener;
use tower::ServiceExt;

struct ChildGuard(std::process::Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn auth_request(route: &str, body: serde_json::Value) -> Request<Body> {
    let mut request = Request::builder()
        .method("POST")
        .uri(route)
        .header(header::HOST, "kvm.test")
        .header(header::ORIGIN, "https://kvm.test")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(
        "192.0.2.10:4242".parse::<SocketAddr>().unwrap(),
    ));
    request
}

async fn response_json(response: axum::response::Response) -> serde_json::Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 16 * 1024).await.unwrap()).unwrap()
}

#[tokio::test]
async fn authenticated_http_reaches_real_signed_stager_without_activation() {
    let uid = nix::unistd::Uid::effective();
    if uid.is_root() {
        return;
    } // Production rejects root Unix peers.
    let user = nix::unistd::User::from_uid(uid).unwrap().unwrap().name;
    let root = tempfile::tempdir().unwrap();
    let slots = create_private_directory(root.path(), "slots");
    let state = create_private_directory(root.path(), "state");
    let runtime = create_private_directory(root.path(), "runtime");
    let spool = create_private_directory(root.path(), "spool");
    let sources = create_private_directory(root.path(), "sources");
    let signing = SigningKey::from_bytes(&[57; 32]);
    let archive = fs::read(write_bundle(&sources, &signing, 1)).unwrap();
    let candidate = fs::read(write_bundle(&sources, &signing, 2)).unwrap();
    let untrusted = fs::read(write_bundle(
        &sources,
        &SigningKey::from_bytes(&[58; 32]),
        3,
    ))
    .unwrap();
    let config = root.path().join("supervisor.json");
    fs::write(
        &config,
        serde_json::to_vec(&json!({
            "schemaVersion": 1, "product": APPLICATION_PRODUCT, "target": APPLICATION_TARGET,
            "keyId": KEY_ID, "publicKeyHex": hex(&signing.verifying_key().to_bytes()),
            "slotRoot": slots, "stateRoot": state, "runtimeRoot": runtime,
            "systemSequence": 1, "healthTimeoutMs": 60_000,
        }))
        .unwrap(),
    )
    .unwrap();
    fs::set_permissions(&config, Permissions::from_mode(0o600)).unwrap();
    let socket = root.path().join("upload.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let app = app_with_options(ServerOptions {
        state_dir: root.path().join("account"),
        update_status_socket: Some(root.path().join("status.sock")),
        update_upload_socket: Some(socket),
        ..ServerOptions::default()
    });
    let login_body = json!({ "username": "operator", "password": "synthetic fixture password" });
    let setup = app
        .clone()
        .oneshot(auth_request("/api/auth/setup", login_body.clone()))
        .await
        .unwrap();
    assert_eq!(response_json(setup).await["code"], 0);
    let login = app
        .clone()
        .oneshot(auth_request("/api/auth/login", login_body))
        .await
        .unwrap();
    let cookie = login.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let csrf = response_json(login).await["data"]["csrfToken"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut published = None;
    let mut published_layout = None;
    for mode in ["publish", "retry", "untrusted", "surplus", "short"] {
        let (mut bytes, declared) = match mode {
            "untrusted" => (untrusted.clone(), untrusted.len()),
            "surplus" => (candidate.clone(), candidate.len()),
            "short" => (candidate.clone(), candidate.len() + 1),
            _ => (archive.clone(), archive.len()),
        };
        if mode == "surplus" {
            bytes.push(0);
        }
        let request = Request::builder()
            .method("POST")
            .uri("/api/application-update/upload")
            .header(header::HOST, "kvm.test")
            .header(header::ORIGIN, "https://kvm.test")
            .header(header::COOKIE, &cookie)
            .header("x-csrf-token", &csrf)
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header(header::CONTENT_LENGTH, declared)
            .body(Body::from(bytes))
            .unwrap();
        let worker = async {
            let (connection, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
                .await
                .unwrap()
                .unwrap();
            let stream = connection.into_std().unwrap();
            stream.set_nonblocking(false).unwrap();
            let fd: OwnedFd = stream.into();
            let mut child = ChildGuard(
                command(
                    env!("CARGO_BIN_EXE_bli-kvm-update-supervisor"),
                    &config,
                    &[
                        "upload-socket",
                        "--peer-user",
                        &user,
                        "--spool-root",
                        spool.to_str().unwrap(),
                    ],
                )
                .stdin(Stdio::from(fd))
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
            );
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if let Some(status) = child.0.try_wait().unwrap() {
                        return status;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap()
        };
        let (response, exit) = tokio::join!(
            tokio::time::timeout(Duration::from_secs(5), app.clone().oneshot(request)),
            worker
        );
        let response = response.unwrap().unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{mode}");
        let value = response_json(response).await;
        if matches!(mode, "publish" | "retry") {
            assert!(exit.success());
            assert_eq!(value["code"], 0);
            assert_eq!(
                value["data"]["disposition"],
                if mode == "publish" {
                    "published"
                } else {
                    "already_present"
                }
            );
            assert_eq!(value["data"]["slot"]["sequence"], "1");
            let slot = value["data"]["slot"]["slotId"].as_str().unwrap().to_owned();
            assert!(slots.join(&slot).join("share/web/index.html").is_file());
            if let Some(previous) = &published {
                assert_eq!(&slot, previous);
            }
            published = Some(slot);
        } else {
            assert_eq!(value["code"], -1, "{mode}");
        }
        assert_eq!(fs::read_dir(&spool).unwrap().count(), 0);
        assert_eq!(fs::read_dir(&state).unwrap().count(), 0);
        assert_eq!(fs::read_dir(&runtime).unwrap().count(), 0);
        assert!(!slots.join("current").exists());
        let staged = fs::read_dir(&slots)
            .unwrap()
            .filter_map(|entry| {
                let entry = entry.unwrap();
                entry
                    .file_name()
                    .to_str()
                    .unwrap()
                    .starts_with("app-")
                    .then(|| entry.file_name())
            })
            .collect::<Vec<_>>();
        assert_eq!(staged.len(), 1, "{mode} must not publish another slot");
        let layout = fs::read_dir(&slots)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<BTreeSet<_>>();
        if let Some(previous) = &published_layout {
            assert_eq!(&layout, previous, "{mode} must not leave staging debris");
        }
        published_layout = Some(layout);
    }
    let mut activate = auth_request(
        "/api/application-update/activate",
        json!({"slotId": published.unwrap()}),
    );
    activate
        .headers_mut()
        .insert(header::COOKIE, cookie.parse().unwrap());
    activate
        .headers_mut()
        .insert("x-csrf-token", csrf.parse().unwrap());
    assert_eq!(
        app.oneshot(activate).await.unwrap().status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
}
