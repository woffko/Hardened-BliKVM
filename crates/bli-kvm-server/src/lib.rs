use std::{path::PathBuf, sync::Arc};

use axum::{
    Json, Router,
    extract::{Request, State},
    http::{HeaderName, HeaderValue, StatusCode},
    middleware::{Next, from_fn, from_fn_with_state},
    response::Response,
    routing::{get, post},
};
use bli_kvm_broker_client::BrokerReadClient;
use bli_kvm_core::{
    CapabilityManifest, CapabilityProvider, DiagnosticsView, EvidenceLevel, FeatureStatus,
    RuntimeInventory,
};
use bli_kvm_platform_bli::BliH616PikvmPlatform;
use bli_kvm_update_client::{UpdateActivationClient, UpdateStatusClient, UpdateUploadClient};
use bli_kvm_video_client::VideoReadClient;
use serde::Serialize;
use tokio::sync::Mutex;
use tower_http::services::{ServeDir, ServeFile};

use crate::{
    auth::{
        middleware::protected, password::AccountStore, rate_limit::LoginRateLimiter,
        session::SessionStore,
    },
    error::{ApiResponse, AppError, AppResult},
};

mod auth;
mod error;
mod update;
mod video;

#[derive(Clone)]
pub(crate) struct AppState {
    pub capabilities: Arc<CapabilityManifest>,
    pub inventory: Arc<BrokerReadClient>,
    pub video: Arc<VideoReadClient>,
    pub update_status: Option<Arc<UpdateStatusClient>>,
    pub update_upload: Option<Arc<UpdateUploadClient>>,
    pub update_activation: Option<Arc<UpdateActivationClient>>,
    pub accounts: Arc<AccountStore>,
    pub sessions: Arc<SessionStore>,
    pub login_limiter: Arc<Mutex<LoginRateLimiter>>,
    pub setup_lock: Arc<Mutex<()>>,
    pub options: Arc<ServerOptions>,
}

#[derive(Debug, Clone)]
pub struct ServerOptions {
    pub web_root: Option<PathBuf>,
    pub state_dir: PathBuf,
    pub secure_cookies: bool,
    pub allow_missing_origin: bool,
    pub session_ttl_secs: u64,
    pub login_max_failures: u32,
    pub login_lockout_secs: u64,
    pub broker_socket: PathBuf,
    pub video_socket: PathBuf,
    /// Read-only updater IPC. No value keeps the capability unavailable.
    pub update_status_socket: Option<PathBuf>,
    /// Stage-only IPC; disabled by default, never enables activation.
    pub update_upload_socket: Option<PathBuf>,
    /// Explicit generation-fenced activation; disabled by default.
    pub update_activation_socket: Option<PathBuf>,
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            web_root: None,
            state_dir: PathBuf::from("state"),
            secure_cookies: false,
            allow_missing_origin: false,
            session_ttl_secs: 900,
            login_max_failures: 5,
            login_lockout_secs: 600,
            broker_socket: PathBuf::from("/run/hardened-blikvm/platform-broker.sock"),
            video_socket: PathBuf::from("/run/hardened-blikvm-video/mjpeg.sock"),
            update_status_socket: None,
            update_upload_socket: None,
            update_activation_socket: None,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Health {
    status: &'static str,
    backend: &'static str,
    platform_id: String,
}

#[derive(Debug, Serialize)]
struct CurrentApplicationVersion {
    current: &'static str,
}

pub fn app() -> Router {
    app_with_options(ServerOptions::default())
}

pub fn app_with_web_root(web_root: Option<PathBuf>) -> Router {
    app_with_options(ServerOptions {
        web_root,
        ..ServerOptions::default()
    })
}

pub fn app_with_options(options: ServerOptions) -> Router {
    if let Some(activation) = &options.update_activation_socket {
        assert!(
            options
                .update_status_socket
                .as_ref()
                .is_some_and(|status| status != activation)
                && options
                    .update_upload_socket
                    .as_ref()
                    .is_some_and(|upload| upload != activation),
            "activation requires separate configured status and upload sockets"
        );
    }
    if let Some(upload) = &options.update_upload_socket {
        assert!(
            options
                .update_status_socket
                .as_ref()
                .is_some_and(|status| status != upload),
            "upload requires a separate configured status socket"
        );
    }
    let mut manifest = BliH616PikvmPlatform.capabilities();
    if options.update_status_socket.is_some() {
        manifest.system.signed_application_updates =
            FeatureStatus::read_only(EvidenceLevel::SourceVerified);
    }
    manifest
        .validate()
        .expect("built-in Bli H616 capability manifest must be valid");
    let state = AppState {
        capabilities: Arc::new(manifest),
        update_activation: options.update_activation_socket.as_ref().map(|path| {
            Arc::new(
                UpdateActivationClient::new(path)
                    .expect("activation socket path must be a bounded absolute Unix path"),
            )
        }),
        update_upload: options.update_upload_socket.as_ref().map(|path| {
            Arc::new(
                UpdateUploadClient::new(path)
                    .expect("upload socket path must be a bounded absolute Unix path"),
            )
        }),
        update_status: options.update_status_socket.as_ref().map(|path| {
            Arc::new(
                UpdateStatusClient::new(path)
                    .expect("update status socket path must be a bounded absolute Unix path"),
            )
        }),
        inventory: Arc::new(
            BrokerReadClient::new(&options.broker_socket)
                .expect("broker socket path must be absolute"),
        ),
        video: Arc::new(
            VideoReadClient::new(&options.video_socket)
                .expect("video proxy socket path must be absolute"),
        ),
        accounts: Arc::new(AccountStore::new(options.state_dir.join("account.json"))),
        sessions: Arc::new(SessionStore::new()),
        login_limiter: Arc::new(Mutex::new(LoginRateLimiter::new(
            options.login_max_failures,
            options.login_lockout_secs,
        ))),
        setup_lock: Arc::new(Mutex::new(())),
        options: Arc::new(options.clone()),
    };

    let public = Router::new()
        .route("/health", get(health))
        .route("/capabilities", get(capability_manifest))
        .route(
            "/application/current-version",
            get(current_application_version),
        )
        .route(
            "/auth/setup",
            get(auth::routes::setup_state).post(auth::routes::setup_first_account),
        )
        .route("/auth/login", post(auth::routes::login));
    let protected_routes = Router::new()
        .route("/auth/account", get(auth::routes::account))
        .route("/auth/logout", post(auth::routes::logout))
        .route(
            "/auth/password",
            get(auth::routes::password_state).post(auth::routes::change_password),
        )
        .route("/inventory", get(runtime_inventory))
        .route("/diagnostics", get(diagnostics_view))
        .route("/application-update/status", get(update::status))
        .route("/application-update/upload", post(update::upload))
        .route("/application-update/activate", post(update::activate))
        .route("/stream/mjpeg", get(video::mjpeg))
        .route_layer(from_fn_with_state(state.clone(), protected));
    let api = public.merge(protected_routes).fallback(api_not_found);
    let router = Router::new().nest("/api", api).with_state(state.clone());
    let router = if let Some(root) = options.web_root {
        let index = root.join("index.html");
        router.fallback_service(ServeDir::new(root).fallback(ServeFile::new(index)))
    } else {
        router
    };

    router.layer(from_fn(security_headers))
}

async fn health(State(state): State<AppState>) -> Json<ApiResponse<Health>> {
    Json(ApiResponse::ok(Health {
        status: "ok",
        backend: "rust",
        platform_id: state.capabilities.platform.id.clone(),
    }))
}

async fn capability_manifest(
    State(state): State<AppState>,
) -> Json<ApiResponse<CapabilityManifest>> {
    Json(ApiResponse::ok((*state.capabilities).clone()))
}

async fn runtime_inventory(
    State(state): State<AppState>,
) -> AppResult<Json<ApiResponse<RuntimeInventory>>> {
    let inventory = state
        .inventory
        .inventory()
        .await
        .map_err(|_| AppError::ServiceUnavailable("platform broker unavailable".into()))?;
    Ok(Json(ApiResponse::ok(inventory)))
}

async fn diagnostics_view(
    State(state): State<AppState>,
) -> AppResult<Json<ApiResponse<DiagnosticsView>>> {
    let inventory = state
        .inventory
        .inventory()
        .await
        .map_err(|_| AppError::ServiceUnavailable("platform broker unavailable".into()))?;
    let diagnostics = DiagnosticsView::from_inventory(&inventory)
        .map_err(|_| AppError::ServiceUnavailable("diagnostics view unavailable".into()))?;
    Ok(Json(ApiResponse::ok(diagnostics)))
}

async fn current_application_version() -> Json<ApiResponse<CurrentApplicationVersion>> {
    Json(ApiResponse::ok(CurrentApplicationVersion {
        current: env!("CARGO_PKG_VERSION"),
    }))
}

async fn api_not_found() -> (StatusCode, Json<ApiResponse<()>>) {
    (
        StatusCode::NOT_FOUND,
        Json(ApiResponse::error(404, "API route not found")),
    )
}

async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    for (name, value) in [
        ("x-content-type-options", "nosniff"),
        ("x-frame-options", "DENY"),
        ("referrer-policy", "no-referrer"),
        (
            "permissions-policy",
            "camera=(), geolocation=(), microphone=()",
        ),
    ] {
        response.headers_mut().insert(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use std::{fs, net::SocketAddr};

    use axum::{
        body::{Body, to_bytes},
        extract::ConnectInfo,
        http::{Method, Request, StatusCode, header},
    };
    use bli_kvm_broker_protocol::{
        BROKER_SCHEMA_VERSION, BrokerReadRequest, BrokerReadResponse, BrokerResponseEnvelope,
        decode_request, encode_frame, encode_response,
    };
    use bli_kvm_core::{
        ComponentObservation, InventoryBackend, RUNTIME_INVENTORY_SCHEMA_VERSION, RtcObservation,
        RuntimeVersions, ThermalObservation,
    };
    use serde_json::Value;
    use tempfile::tempdir;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::UnixListener,
    };
    use tower::ServiceExt;

    use super::*;

    fn auth_app(state_dir: PathBuf, max_failures: u32) -> Router {
        app_with_options(ServerOptions {
            state_dir,
            secure_cookies: true,
            login_max_failures: max_failures,
            ..ServerOptions::default()
        })
    }

    fn json_request(method: Method, uri: &str, body: Value) -> Request<Body> {
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::HOST, "kvm.test")
            .header(header::ORIGIN, "https://kvm.test")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        request.extensions_mut().insert(ConnectInfo(
            "192.0.2.10:4242".parse::<SocketAddr>().unwrap(),
        ));
        request
    }

    async fn json_body(response: Response) -> Value {
        let body = to_bytes(response.into_body(), 256 * 1024).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    async fn setup_and_login(app: &Router) -> (String, String) {
        let setup = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/api/auth/setup",
                serde_json::json!({
                    "username": "operator",
                    "password": "initial password"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(json_body(setup).await["code"], 0);

        let login = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/api/auth/login",
                serde_json::json!({
                    "username": "operator",
                    "password": "initial password"
                }),
            ))
            .await
            .unwrap();
        let cookie = login.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let login_body = json_body(login).await;
        let csrf = login_body["data"]["csrfToken"].as_str().unwrap().to_owned();
        (cookie, csrf)
    }

    #[tokio::test]
    async fn update_status_requires_login_before_contacting_ipc() {
        let root = tempdir().unwrap();
        let socket = root.path().join("update.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let app = app_with_options(ServerOptions {
            state_dir: root.path().join("account"),
            update_status_socket: Some(socket),
            ..ServerOptions::default()
        });
        let response = app
            .clone()
            .oneshot(json_request(
                Method::GET,
                "/api/application-update/status",
                Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), listener.accept())
                .await
                .is_err()
        );
        let manifest = json_body(
            app.oneshot(json_request(Method::GET, "/api/capabilities", Value::Null))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            manifest["data"]["system"]["signedApplicationUpdates"]["availability"],
            "read_only"
        );
        assert_eq!(
            manifest["data"]["system"]["signedSystemUpdates"]["availability"],
            "unavailable"
        );
    }

    #[tokio::test]
    async fn activation_is_authorized_bounded_once_and_awaiting_health_only() {
        use bli_kvm_update_protocol::{self as protocol, activation as ipc};
        let root = tempdir().unwrap();
        let socket = root.path().join("activate.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let app = app_with_options(ServerOptions {
            state_dir: root.path().join("account"),
            update_status_socket: Some(root.path().join("status.sock")),
            update_upload_socket: Some(root.path().join("upload.sock")),
            update_activation_socket: Some(socket),
            ..ServerOptions::default()
        });
        let (cookie, csrf) = setup_and_login(&app).await;
        let slot = format!("app-00000000000000000002-{}", "a".repeat(64));
        let payload =
            serde_json::json!({"slotId":slot, "expectedGeneration": u64::MAX.to_string()});
        for mode in [
            "no-login",
            "no-csrf",
            "cross-origin",
            "extra-field",
            "numeric-generation",
            "oversize",
        ] {
            let mut body = payload.clone();
            if mode == "extra-field" {
                body["command"] = serde_json::json!("forbidden");
            }
            if mode == "numeric-generation" {
                body["expectedGeneration"] = serde_json::json!(1);
            }
            if mode == "oversize" {
                body["slotId"] = serde_json::json!("a".repeat(17000));
            }
            let mut request = json_request(Method::POST, "/api/application-update/activate", body);
            if mode != "no-login" {
                request
                    .headers_mut()
                    .insert(header::COOKIE, cookie.parse().unwrap());
            }
            if mode != "no-csrf" {
                request
                    .headers_mut()
                    .insert("x-csrf-token", csrf.parse().unwrap());
            }
            if mode == "cross-origin" {
                request
                    .headers_mut()
                    .insert(header::ORIGIN, "https://attacker.invalid".parse().unwrap());
            }
            let response = app.clone().oneshot(request).await.unwrap();
            match mode {
                "no-login" => assert_eq!(response.status(), StatusCode::UNAUTHORIZED),
                "no-csrf" | "cross-origin" => assert_eq!(response.status(), StatusCode::FORBIDDEN),
                _ => assert_eq!(json_body(response).await["code"], -1),
            }
        }
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), listener.accept())
                .await
                .is_err()
        );
        let worker = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).await.unwrap();
            assert_eq!(
                u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize,
                bytes.len() - 4
            );
            let request = ipc::decode_request(&bytes[4..]).unwrap();
            assert_eq!(
                request.request,
                ipc::ActivationRequest::Activate {
                    slot_id: slot.clone(),
                    expected_generation: u64::MAX
                }
            );
            let response = ipc::ActivationResponseEnvelope {
                schema_version: 1,
                request_id: request.request_id,
                response: ipc::ActivationResponse::AwaitingHealth {
                    receipt: ipc::ActivationReceipt {
                        operation_id: "1".repeat(32),
                        target_slot_id: slot,
                    },
                },
            };
            stream
                .write_all(
                    &protocol::encode_frame(&ipc::encode_response(&response).unwrap()).unwrap(),
                )
                .await
                .unwrap();
        });
        let mut request = json_request(Method::POST, "/api/application-update/activate", payload);
        request
            .headers_mut()
            .insert(header::COOKIE, cookie.parse().unwrap());
        request
            .headers_mut()
            .insert("x-csrf-token", csrf.parse().unwrap());
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let result = json_body(response).await;
        assert_eq!(result["data"]["phase"], "awaiting_health");
        assert_eq!(result["data"]["receipt"]["operationId"], "1".repeat(32));
        assert!(result["data"].get("confirmed").is_none());
        worker.await.unwrap();
        let manifest = json_body(
            app.oneshot(json_request(Method::GET, "/api/capabilities", Value::Null))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            manifest["data"]["system"]["signedApplicationUpdates"]["availability"],
            "read_only"
        );
    }

    #[tokio::test]
    async fn upload_authorization_and_headers_precede_body_and_ipc() {
        struct NeverBody;
        impl http_body::Body for NeverBody {
            type Data = bytes::Bytes;
            type Error = std::io::Error;
            fn poll_frame(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>>
            {
                panic!("rejected upload body was consumed")
            }
        }
        let root = tempdir().unwrap();
        let socket = root.path().join("upload.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let app = app_with_options(ServerOptions {
            state_dir: root.path().join("account"),
            update_status_socket: Some(root.path().join("status.sock")),
            update_upload_socket: Some(socket),
            ..ServerOptions::default()
        });
        let (cookie, csrf) = setup_and_login(&app).await;
        for mode in [
            "no-login",
            "no-csrf",
            "cross-origin",
            "missing-origin",
            "bad-type",
        ] {
            let mut request = Request::builder()
                .method(Method::POST)
                .uri("/api/application-update/upload")
                .header(header::HOST, "localhost")
                .header(
                    header::CONTENT_TYPE,
                    if mode == "bad-type" {
                        "multipart/form-data"
                    } else {
                        "application/octet-stream"
                    },
                )
                .header(header::CONTENT_LENGTH, "3");
            if mode != "no-login" {
                request = request.header(header::COOKIE, &cookie);
            }
            if mode != "no-csrf" {
                request = request.header("x-csrf-token", &csrf);
            }
            if mode != "missing-origin" {
                request = request.header(
                    header::ORIGIN,
                    if mode == "cross-origin" {
                        "https://elsewhere.invalid"
                    } else {
                        "https://localhost"
                    },
                );
            }
            let response = app
                .clone()
                .oneshot(request.body(Body::new(NeverBody)).unwrap())
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                match mode {
                    "no-login" => StatusCode::UNAUTHORIZED,
                    "bad-type" => StatusCode::OK,
                    _ => StatusCode::FORBIDDEN,
                }
            );
            if mode == "bad-type" {
                assert_eq!(json_body(response).await["code"], -1);
            }
        }
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), listener.accept())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn authenticated_upload_streams_stage_only_and_preserves_exact_counters() {
        use bli_kvm_update_protocol::{self as updates, upload};
        let root = tempdir().unwrap();
        let socket = root.path().join("upload.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let app = app_with_options(ServerOptions {
            state_dir: root.path().join("account"),
            update_status_socket: Some(root.path().join("status.sock")),
            update_upload_socket: Some(socket),
            ..ServerOptions::default()
        });
        let (cookie, csrf) = setup_and_login(&app).await;
        let worker = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let length = stream.read_u32().await.unwrap() as usize;
            let mut metadata = vec![0; length];
            stream.read_exact(&mut metadata).await.unwrap();
            let request = upload::decode_request(&metadata).unwrap();
            assert_eq!(
                request.request,
                upload::UploadRequest::Stage { archive_bytes: 3 }
            );
            let mut body = Vec::new();
            stream.read_to_end(&mut body).await.unwrap();
            assert_eq!(body, b"abc");
            let receipt = upload::UploadReceipt {
                slot: updates::ApplicationSlotVersion {
                    slot_id: format!("app-{:020}-{}", u64::MAX, "a".repeat(64)),
                    version: "1.0.0".into(),
                    sequence: u64::MAX,
                    minimum_system_sequence: 1,
                },
                disposition: upload::UploadDisposition::AlreadyPresent,
                manifest_sha256: "a".repeat(64),
                payload_bytes: 1,
            };
            let response = upload::UploadResponseEnvelope {
                schema_version: 1,
                request_id: request.request_id,
                response: upload::UploadResponse::Staged { receipt },
            };
            stream
                .write_all(
                    &updates::encode_frame(&upload::encode_response(&response).unwrap()).unwrap(),
                )
                .await
                .unwrap();
        });
        let request = Request::builder()
            .method(Method::POST)
            .uri("/api/application-update/upload")
            .header(header::HOST, "localhost")
            .header(header::ORIGIN, "https://localhost")
            .header(header::COOKIE, cookie)
            .header("x-csrf-token", csrf)
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header(header::CONTENT_LENGTH, "3")
            .body(Body::from("abc"))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["code"], 0);
        assert_eq!(body["data"]["slot"]["sequence"], "18446744073709551615");
        assert_eq!(body["data"]["payloadBytes"], "1");
        assert_eq!(body["data"]["disposition"], "already_present");
        worker.await.unwrap();
        let manifest = json_body(
            app.oneshot(json_request(Method::GET, "/api/capabilities", Value::Null))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            manifest["data"]["system"]["signedApplicationUpdates"]["availability"],
            "read_only"
        );
    }

    #[tokio::test]
    async fn logout_interrupts_stalled_upload_without_complete_ipc_body() {
        struct StalledBody;
        impl http_body::Body for StalledBody {
            type Data = bytes::Bytes;
            type Error = std::io::Error;
            fn poll_frame(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>>
            {
                std::task::Poll::Pending
            }
        }
        let root = tempdir().unwrap();
        let socket = root.path().join("upload.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let app = app_with_options(ServerOptions {
            state_dir: root.path().join("account"),
            update_status_socket: Some(root.path().join("status.sock")),
            update_upload_socket: Some(socket),
            ..ServerOptions::default()
        });
        let (cookie, csrf) = setup_and_login(&app).await;
        let request = Request::builder()
            .method(Method::POST)
            .uri("/api/application-update/upload")
            .header(header::HOST, "localhost")
            .header(header::ORIGIN, "https://localhost")
            .header(header::COOKIE, &cookie)
            .header("x-csrf-token", &csrf)
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header(header::CONTENT_LENGTH, "3")
            .body(Body::new(StalledBody))
            .unwrap();
        let transfer = tokio::spawn(app.clone().oneshot(request));
        let (mut stream, _) = listener.accept().await.unwrap();
        let length = stream.read_u32().await.unwrap() as usize;
        stream.read_exact(&mut vec![0; length]).await.unwrap();
        let mut logout = json_request(Method::POST, "/api/auth/logout", Value::Null);
        logout
            .headers_mut()
            .insert(header::COOKIE, cookie.parse().unwrap());
        logout
            .headers_mut()
            .insert("x-csrf-token", csrf.parse().unwrap());
        assert_eq!(
            json_body(app.oneshot(logout).await.unwrap()).await["code"],
            0
        );
        let response = tokio::time::timeout(std::time::Duration::from_secs(1), transfer)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).await.unwrap();
        assert!(bytes.is_empty());
    }

    #[tokio::test]
    async fn authenticated_update_status_uses_only_typed_read_ipc() {
        use bli_kvm_update_protocol as updates;
        let root = tempdir().unwrap();
        let socket = root.path().join("update.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let app = app_with_options(ServerOptions {
            state_dir: root.path().join("account"),
            secure_cookies: true,
            update_status_socket: Some(socket),
            ..ServerOptions::default()
        });
        let (cookie, csrf) = setup_and_login(&app).await;
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut input = Vec::new();
            stream.read_to_end(&mut input).await.unwrap();
            let request = updates::decode_request(updates::decode_frame(&input).unwrap()).unwrap();
            assert_eq!(request.request, updates::UpdateReadRequest::Status {});
            let slot = updates::ApplicationSlotVersion {
                slot_id: format!("app-00000000000000000002-{}", "a".repeat(64)),
                version: "0.1.0-dev2".into(),
                sequence: 2,
                minimum_system_sequence: 1,
            };
            let status = updates::ApplicationUpdateStatus {
                generation: 10,
                floor_sequence: 2,
                system_sequence: 1,
                selected: slot.clone(),
                confirmed: slot,
                previous: None,
                pending: None,
                last_rollback: Some(updates::ApplicationRollbackReason::BootChanged),
            };
            let response = updates::UpdateResponseEnvelope {
                schema_version: 1,
                request_id: request.request_id,
                response: updates::UpdateReadResponse::Status {
                    status: Box::new(status),
                },
            };
            stream
                .write_all(
                    &updates::encode_frame(&updates::encode_response(&response).unwrap()).unwrap(),
                )
                .await
                .unwrap();
        });
        let mut request = json_request(Method::GET, "/api/application-update/status", Value::Null);
        request
            .headers_mut()
            .insert(header::COOKIE, cookie.parse().unwrap());
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["data"]["generation"], "10");
        assert_eq!(body["data"]["floorSequence"], "2");
        assert_eq!(body["data"]["confirmed"]["version"], "0.1.0-dev2");
        assert_eq!(body["data"]["lastRollback"], "boot_changed");
        task.await.unwrap();
        let mut mutation = json_request(
            Method::POST,
            "/api/application-update/activate",
            serde_json::json!({"slotId": "untrusted"}),
        );
        mutation
            .headers_mut()
            .insert(header::COOKIE, cookie.parse().unwrap());
        mutation
            .headers_mut()
            .insert("x-csrf-token", csrf.parse().unwrap());
        assert_eq!(
            app.oneshot(mutation).await.unwrap().status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[tokio::test]
    async fn unconfigured_update_status_remains_unavailable_after_login() {
        let root = tempdir().unwrap();
        let app = auth_app(root.path().join("account"), 5);
        let (cookie, _) = setup_and_login(&app).await;
        let mut request = json_request(Method::GET, "/api/application-update/status", Value::Null);
        request
            .headers_mut()
            .insert(header::COOKIE, cookie.parse().unwrap());
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(json_body(response).await["msg"], "service unavailable");
    }

    fn diagnostics_inventory() -> RuntimeInventory {
        RuntimeInventory {
            schema_version: RUNTIME_INVENTORY_SCHEMA_VERSION,
            platform_id: "bli-h616-pikvm".into(),
            backend: InventoryBackend::KvmdTransition,
            versions: RuntimeVersions {
                kvmd: Some("3.291".into()),
                streamer: Some("6.11".into()),
            },
            video: ComponentObservation {
                enabled: Some(true),
                online: Some(true),
                busy: Some(false),
                connected: None,
            },
            hid: ComponentObservation {
                online: Some(true),
                ..ComponentObservation::default()
            },
            power: ComponentObservation {
                enabled: Some(true),
                online: Some(true),
                ..ComponentObservation::default()
            },
            virtual_media: ComponentObservation {
                enabled: Some(true),
                online: Some(true),
                busy: Some(false),
                connected: Some(false),
            },
            thermal: ThermalObservation {
                temperature_millidegrees_celsius: Some(42_125),
            },
            rtc: RtcObservation {
                unix_seconds: Some(1_759_489_393),
                offset_from_system_seconds: Some(-28_647_995),
            },
            power_led: Some(true),
            janus_enabled: Some(false),
        }
    }

    fn spawn_inventory_broker(
        listener: UnixListener,
        inventory: RuntimeInventory,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut prefix = [0_u8; 4];
            stream.read_exact(&mut prefix).await.unwrap();
            let mut payload = vec![0_u8; u32::from_be_bytes(prefix) as usize];
            stream.read_exact(&mut payload).await.unwrap();
            let request = decode_request(&payload).unwrap();
            assert_eq!(request.schema_version, BROKER_SCHEMA_VERSION);
            assert_eq!(request.request, BrokerReadRequest::Inventory);
            let response = BrokerResponseEnvelope {
                schema_version: BROKER_SCHEMA_VERSION,
                request_id: request.request_id,
                response: BrokerReadResponse::Inventory { inventory },
            };
            let payload = encode_response(&response).unwrap();
            stream
                .write_all(&encode_frame(&payload).unwrap())
                .await
                .unwrap();
        })
    }

    #[tokio::test]
    async fn health_is_small_and_platform_specific() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["code"], 0);
        assert_eq!(value["data"]["backend"], "rust");
        assert_eq!(value["data"]["platformId"], "bli-h616-pikvm");
    }

    #[tokio::test]
    async fn capability_endpoint_preserves_evidence_level() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/api/capabilities")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 256 * 1024).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["data"]["schemaVersion"], 1);
        assert_eq!(
            value["data"]["video"]["status"]["evidence"],
            "runtime_verified"
        );
        assert_eq!(
            value["data"]["video"]["status"]["availability"],
            "read_only"
        );
        assert_eq!(
            value["data"]["video"]["transports"],
            serde_json::json!(["mjpeg"])
        );
        assert_eq!(
            value["data"]["power"]["status"]["availability"],
            "unavailable"
        );
        assert_eq!(
            value["data"]["power"]["ledState"]["availability"],
            "read_only"
        );
        assert_eq!(
            value["data"]["virtualMedia"]["status"]["availability"],
            "read_only"
        );
        assert_eq!(
            value["data"]["virtualMedia"]["remoteDownload"]["availability"],
            "unavailable"
        );
    }

    #[tokio::test]
    async fn runtime_inventory_requires_an_authenticated_session() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/api/inventory")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn diagnostics_require_an_authenticated_session() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/api/diagnostics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn authenticated_diagnostics_are_current_only_typed_and_private() {
        let directory = tempdir().unwrap();
        let broker_socket = directory.path().join("broker.sock");
        let listener = UnixListener::bind(&broker_socket).unwrap();
        let app = app_with_options(ServerOptions {
            state_dir: directory.path().join("state"),
            broker_socket,
            secure_cookies: true,
            ..ServerOptions::default()
        });
        let (cookie, _) = setup_and_login(&app).await;
        let broker = spawn_inventory_broker(listener, diagnostics_inventory());
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/diagnostics")
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let value = json_body(response).await;
        let data = &value["data"];
        assert_eq!(data["schemaVersion"], 1);
        assert_eq!(data["availability"], "current_only");
        assert_eq!(data["current"]["temperature"]["state"], "available");
        assert_eq!(
            data["current"]["temperature"]["millidegreesCelsius"],
            42_125
        );
        assert_eq!(data["current"]["components"]["video"]["online"], "active");
        assert_eq!(data["history"]["reason"], "persistence_not_provisioned");
        assert!(data.get("platformId").is_none());
        assert!(data.get("versions").is_none());
        let serialized = serde_json::to_string(data).unwrap().to_ascii_lowercase();
        for forbidden in [
            "bli-h616-pikvm",
            "3.291",
            "6.11",
            "bundle",
            "binding",
            "instance_id",
            "window_id",
            "password",
            "credential",
            "serial",
            "address",
            "raw_log",
        ] {
            assert!(!serialized.contains(forbidden));
        }
        broker.await.unwrap();
    }

    #[tokio::test]
    async fn diagnostics_are_get_only_and_backend_errors_are_sanitized() {
        let directory = tempdir().unwrap();
        let app = app_with_options(ServerOptions {
            state_dir: directory.path().join("state"),
            broker_socket: directory.path().join("missing-broker.sock"),
            secure_cookies: true,
            ..ServerOptions::default()
        });
        let (cookie, csrf) = setup_and_login(&app).await;

        let mut request = json_request(Method::POST, "/api/diagnostics", serde_json::json!({}));
        request.headers_mut().insert(
            header::COOKIE,
            header::HeaderValue::from_str(&cookie).unwrap(),
        );
        request.headers_mut().insert(
            "x-csrf-token",
            header::HeaderValue::from_str(&csrf).unwrap(),
        );
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/diagnostics")
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let value = json_body(response).await;
        assert_eq!(value["code"], -503);
        assert_eq!(value["msg"], "service unavailable");
        assert!(value.get("data").is_none());
    }

    #[tokio::test]
    async fn mjpeg_stream_requires_an_authenticated_session() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/api/stream/mjpeg")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn authenticated_mjpeg_stream_preserves_only_safe_headers_and_body() {
        let directory = tempdir().unwrap();
        let video_socket = directory.path().join("video.sock");
        let listener = UnixListener::bind(&video_socket).unwrap();
        let app = app_with_options(ServerOptions {
            state_dir: directory.path().join("state"),
            video_socket,
            secure_cookies: true,
            ..ServerOptions::default()
        });
        let (cookie, _) = setup_and_login(&app).await;
        let proxy = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut buffer = [0_u8; 1024];
                let read = stream.read(&mut buffer).await.unwrap();
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8(request).unwrap().to_ascii_lowercase();
            assert!(request.starts_with("get /mjpeg http/1.1\r\n"));
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: multipart/x-mixed-replace; boundary=frame\r\nX-Private-Backend: forbidden\r\nContent-Length: 4\r\n\r\nJPEG",
                )
                .await
                .unwrap();
        });

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/stream/mjpeg")
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "multipart/x-mixed-replace; boundary=frame"
        );
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "no-store, no-cache, must-revalidate"
        );
        assert!(!response.headers().contains_key("x-private-backend"));
        assert_eq!(
            to_bytes(response.into_body(), 64 * 1024).await.unwrap(),
            "JPEG"
        );
        proxy.await.unwrap();
    }

    #[tokio::test]
    async fn unavailable_video_proxy_returns_sanitized_service_unavailable() {
        let directory = tempdir().unwrap();
        let app = app_with_options(ServerOptions {
            state_dir: directory.path().join("state"),
            video_socket: directory.path().join("missing-video.sock"),
            ..ServerOptions::default()
        });
        let (cookie, _) = setup_and_login(&app).await;

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/stream/mjpeg")
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = json_body(response).await;
        assert_eq!(body["code"], -503);
        assert_eq!(body["msg"], "service unavailable");
        assert!(!body.to_string().contains("missing-video"));
    }

    #[tokio::test]
    async fn unsupported_method_is_rejected() {
        let response = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/capabilities")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn unknown_api_route_does_not_fall_through_to_the_spa() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("index.html"), "<h1>BLi-KVM</h1>").unwrap();
        let response = app_with_web_root(Some(directory.path().to_path_buf()))
            .oneshot(
                Request::builder()
                    .uri("/api/not-real")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn static_files_and_spa_fallback_are_served_from_the_selected_root() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("index.html"), "<h1>BLi-KVM</h1>").unwrap();
        fs::write(directory.path().join("asset.txt"), "asset").unwrap();
        let app = app_with_web_root(Some(directory.path().to_path_buf()));

        let asset = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/asset.txt")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(asset.status(), StatusCode::OK);
        assert_eq!(
            to_bytes(asset.into_body(), 64 * 1024).await.unwrap(),
            "asset"
        );

        let spa = app
            .oneshot(
                Request::builder()
                    .uri("/desktop/deep-link")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(spa.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn first_account_session_and_logout_flow_is_enforced() {
        let directory = tempdir().unwrap();
        let app = auth_app(directory.path().join("state"), 5);

        let setup_state = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/auth/setup")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(json_body(setup_state).await["data"]["required"], true);

        let missing_origin = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/auth/setup")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "username": "operator",
                            "password": "initial password"
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing_origin.status(), StatusCode::FORBIDDEN);

        let setup = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/api/auth/setup",
                serde_json::json!({
                    "username": "operator",
                    "password": "initial password"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(json_body(setup).await["code"], 0);

        let duplicate = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/api/auth/setup",
                serde_json::json!({
                    "username": "other",
                    "password": "another password"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(json_body(duplicate).await["code"], -409);

        let wrong = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/api/auth/login",
                serde_json::json!({
                    "username": "operator",
                    "password": "wrong password"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(json_body(wrong).await["code"], -2);

        let login = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/api/auth/login",
                serde_json::json!({
                    "username": "operator",
                    "password": "initial password"
                }),
            ))
            .await
            .unwrap();
        let set_cookie = login.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .to_owned();
        assert!(set_cookie.contains("HttpOnly"));
        assert!(set_cookie.contains("SameSite=Strict"));
        assert!(set_cookie.contains("Secure"));
        let cookie = set_cookie.split(';').next().unwrap().to_owned();
        let login_body = json_body(login).await;
        let csrf = login_body["data"]["csrfToken"].as_str().unwrap().to_owned();

        let account = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/auth/account")
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(json_body(account).await["data"]["username"], "operator");

        let logout_without_csrf = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/auth/logout")
                    .header(header::COOKIE, &cookie)
                    .header(header::HOST, "kvm.test")
                    .header(header::ORIGIN, "https://kvm.test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(logout_without_csrf.status(), StatusCode::FORBIDDEN);

        let logout = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/auth/logout")
                    .header(header::COOKIE, &cookie)
                    .header("x-csrf-token", csrf)
                    .header(header::HOST, "kvm.test")
                    .header(header::ORIGIN, "https://kvm.test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            logout.headers()[header::SET_COOKIE]
                .to_str()
                .unwrap()
                .contains("Max-Age=0")
        );
        assert_eq!(json_body(logout).await["code"], 0);

        let revoked = app
            .oneshot(
                Request::builder()
                    .uri("/api/auth/account")
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn login_lockout_uses_the_frontend_compatible_error_code() {
        let directory = tempdir().unwrap();
        let app = auth_app(directory.path().join("state"), 2);
        let setup = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/api/auth/setup",
                serde_json::json!({
                    "username": "operator",
                    "password": "initial password"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(json_body(setup).await["code"], 0);

        for expected in [-2, -2, -5] {
            let response = app
                .clone()
                .oneshot(json_request(
                    Method::POST,
                    "/api/auth/login",
                    serde_json::json!({
                        "username": "operator",
                        "password": "wrong password"
                    }),
                ))
                .await
                .unwrap();
            assert_eq!(json_body(response).await["code"], expected);
        }
    }
}
