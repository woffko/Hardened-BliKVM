//! PicoClaw AI assistant (reference `/api/picoclaw/*`), switched off by default.
//!
//! See `docs/design/admin-features-v1.md`. The runtime runs as its own user;
//! the server keeps the reference's API, relays the chat WebSocket to the
//! runtime's local gateway and serves the runtime's KVM tools (screenshot,
//! HID actions) on an internal loopback route. Privileged steps go to the
//! helpers: the download helper installs the pinned release, the admin
//! helper starts and stops the unit, and the PicoClaw helper, running as the
//! runtime's user, edits its configuration and reads its chat history.

mod gateway;
mod keys;
mod kvm;

use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant, SystemTime};

use axum::{
    Json,
    extract::{Path as UrlPath, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use bli_kvm_admin_helper::{
    AdminReply, AdminRequest, PICOCLAW_BINARY, PICOCLAW_GATEWAY_PORT, PICOCLAW_INSTALL_STATUS,
    PICOCLAW_RELEASE_URL, PicoclawFacts, PicoclawInstallStatus,
};
use bli_kvm_core::AdminFeature;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::Mutex;

use crate::{AppState, auth::session::Session};

pub(crate) use gateway::connect_gateway;
pub(crate) use kvm::internal_router;

const SESSION_ID_HEADER: &str = "x-picoclaw-session-id";
/// The reference holds the assistant lock for 30 minutes, renewed by traffic.
const LOCK_DURATION: Duration = Duration::from_secs(30 * 60);
const READY_TIMEOUT: Duration = Duration::from_secs(15);
const INTERNAL_TOKEN_FILE: &str = "picoclaw-internal-token";

/// The assistant session currently relayed, with the operator who opened it.
pub(crate) struct Active {
    pub session_id: String,
    pub operator: Session,
    /// Text frames for the runtime (the reference's load-image instruction).
    pub upstream: tokio::sync::mpsc::Sender<String>,
    /// Text frames for the browser (screenshot observations).
    pub downstream: tokio::sync::mpsc::Sender<String>,
    pub close: tokio::sync::watch::Sender<Option<(u16, String)>>,
}

#[derive(Default)]
struct Lock {
    owner: Option<(String, Instant)>,
}

/// Releases the assistant lock for a session when dropped: a connection
/// whose upgrade never completes drops the closure that would have relayed it,
/// and must not leave the operator's keyboard and mouse blocked until the
/// lock expires.
pub(crate) struct ReleaseOnDrop {
    pub(crate) picoclaw: Arc<Picoclaw>,
    pub(crate) session_id: String,
}

impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.picoclaw.release(&self.session_id);
    }
}

pub(crate) struct Picoclaw {
    internal_token: String,
    mcp_url: String,
    lock: StdMutex<Lock>,
    pub(crate) active: Mutex<Option<Active>>,
    last_error: StdMutex<String>,
}

impl Picoclaw {
    /// Loads or creates the internal token the runtime's MCP calls carry.
    pub(crate) fn new(state_dir: &Path, mcp_url: String) -> Self {
        let path = state_dir.join(INTERNAL_TOKEN_FILE);
        let internal_token = std::fs::read_to_string(&path)
            .ok()
            .map(|token| token.trim().to_string())
            .filter(|token| token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()))
            .unwrap_or_else(|| {
                let token = random_hex(32);
                let _ = write_private(&path, token.as_bytes());
                token
            });
        Self {
            internal_token,
            mcp_url,
            lock: StdMutex::new(Lock::default()),
            active: Mutex::new(None),
            last_error: StdMutex::new(String::new()),
        }
    }

    pub(crate) fn internal_token(&self) -> &str {
        &self.internal_token
    }

    /// Takes or renews the lock for `session_id`; `Ok(true)` when newly taken.
    pub(crate) fn acquire(&self, session_id: &str) -> Result<bool, String> {
        let mut lock = self.lock.lock().expect("lock poisoned");
        let now = Instant::now();
        if lock
            .owner
            .as_ref()
            .is_some_and(|(_, expires)| *expires <= now)
        {
            lock.owner = None;
        }
        match &lock.owner {
            Some((owner, _)) if owner != session_id => Err(owner.clone()),
            current => {
                let taken = current.is_none();
                lock.owner = Some((session_id.to_string(), now + LOCK_DURATION));
                Ok(taken)
            }
        }
    }

    pub(crate) fn renew(&self, session_id: &str) -> bool {
        let mut lock = self.lock.lock().expect("lock poisoned");
        match &mut lock.owner {
            Some((owner, expires)) if owner == session_id && *expires > Instant::now() => {
                *expires = Instant::now() + LOCK_DURATION;
                true
            }
            _ => false,
        }
    }

    pub(crate) fn release(&self, session_id: &str) {
        let mut lock = self.lock.lock().expect("lock poisoned");
        if lock
            .owner
            .as_ref()
            .is_some_and(|(owner, _)| owner == session_id)
        {
            lock.owner = None;
        }
    }

    pub(crate) fn owner(&self) -> Option<String> {
        let mut lock = self.lock.lock().expect("lock poisoned");
        if lock
            .owner
            .as_ref()
            .is_some_and(|(_, expires)| *expires <= Instant::now())
        {
            lock.owner = None;
        }
        lock.owner.as_ref().map(|(owner, _)| owner.clone())
    }

    /// While an assistant session holds the lock, manual HID input is
    /// dropped, as in the reference.
    pub(crate) fn blocks_manual_input(&self) -> bool {
        self.owner().is_some()
    }

    fn set_error(&self, message: &str) {
        *self.last_error.lock().expect("lock poisoned") = message.to_string();
    }

    fn last_error(&self) -> String {
        self.last_error.lock().expect("lock poisoned").clone()
    }
}

fn random_hex(bytes: usize) -> String {
    use rand_core::{OsRng, RngCore};
    let mut buffer = vec![0_u8; bytes];
    OsRng.fill_bytes(&mut buffer);
    buffer.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn set_error(picoclaw: &Picoclaw, message: &str) {
    picoclaw.set_error(message);
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let temporary = path.with_extension("new");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)?;
    file.write_all(bytes)?;
    drop(file);
    std::fs::rename(temporary, path)
}

/// A reference-shaped PicoClaw error: `{code, message}` with a string code.
pub(crate) struct PicoclawError {
    status: StatusCode,
    code: &'static str,
    message: String,
    session_id: Option<String>,
}

impl PicoclawError {
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::OK,
            code,
            message: message.into(),
            session_id: None,
        }
    }

    fn with_status(mut self, status: StatusCode) -> Self {
        self.status = status;
        self
    }

    fn with_session(mut self, session_id: String) -> Self {
        self.session_id = Some(session_id);
        self
    }
}

impl IntoResponse for PicoclawError {
    fn into_response(self) -> Response {
        let mut body = json!({
            "code": self.code,
            "message": self.message,
            "msg": self.message,
        });
        if let Some(session_id) = self.session_id {
            body["session_id"] = json!(session_id);
        }
        (self.status, Json(body)).into_response()
    }
}

pub(crate) const RUNTIME_UNAVAILABLE: &str = "RUNTIME_UNAVAILABLE";
pub(crate) const RUNTIME_START_FAILED: &str = "RUNTIME_START_FAILED";
pub(crate) const INVALID_ACTION: &str = "INVALID_ACTION";
pub(crate) const LOCK_HELD: &str = "AI_LOCK_HELD";
pub(crate) const SESSION_ID_MISSING: &str = "SESSION_ID_MISSING";

type PicoResult<T> = Result<T, PicoclawError>;

fn success(data: impl Serialize) -> Response {
    Json(json!({"code": 0, "msg": "success", "data": data})).into_response()
}

/// Refuses unless the switch is on and every helper is configured.
fn enabled(state: &AppState) -> Result<&Picoclaw, Box<Response>> {
    if let Err(error) = crate::admin_features::require(AdminFeature::Picoclaw) {
        return Err(Box::new(error.into_response()));
    }
    state.picoclaw.as_deref().ok_or_else(|| {
        Box::new(
            PicoclawError::new(RUNTIME_UNAVAILABLE, "picoclaw is not configured").into_response(),
        )
    })
}

fn socket(path: &Option<PathBuf>) -> PicoResult<&Path> {
    path.as_deref()
        .ok_or_else(|| PicoclawError::new(RUNTIME_UNAVAILABLE, "picoclaw is not configured"))
}

pub(crate) async fn helper(
    path: &Option<PathBuf>,
    request: AdminRequest,
) -> PicoResult<AdminReply> {
    let socket = socket(path)?;
    match crate::scripts::call(socket, &request, None).await {
        Ok(AdminReply::Refused { reason }) => Err(PicoclawError::new(RUNTIME_UNAVAILABLE, reason)),
        Ok(reply) => Ok(reply),
        Err(_) => Err(PicoclawError::new(
            RUNTIME_UNAVAILABLE,
            "the picoclaw helper is unavailable",
        )),
    }
}

pub(crate) async fn facts(state: &AppState) -> PicoResult<PicoclawFacts> {
    match helper(
        &state.options.picoclaw_socket,
        AdminRequest::PicoclawState {},
    )
    .await?
    {
        AdminReply::Picoclaw { facts } => Ok(facts),
        _ => Err(PicoclawError::new(
            RUNTIME_UNAVAILABLE,
            "unexpected picoclaw helper answer",
        )),
    }
}

/// The reference's runtime status document.
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct RuntimeStatus {
    ready: bool,
    installed: bool,
    installing: bool,
    #[serde(skip_serializing_if = "is_zero")]
    install_progress: u8,
    #[serde(skip_serializing_if = "String::is_empty")]
    install_stage: String,
    install_path: &'static str,
    #[serde(skip_serializing_if = "String::is_empty")]
    agent_profile: String,
    model_configured: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    model_name: String,
    status: &'static str,
    #[serde(skip_serializing_if = "String::is_empty")]
    config_error: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    last_error: String,
    checked_at: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    current_session: String,
}

fn is_zero(value: &u8) -> bool {
    *value == 0
}

pub(crate) fn rfc3339_now() -> String {
    let seconds = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let days = (seconds / 86_400) as i64;
    let rest = seconds % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// The download helper's install progress while its lock is held.
fn install_progress() -> Option<PicoclawInstallStatus> {
    install_progress_at(Path::new(PICOCLAW_INSTALL_STATUS))
}

fn install_progress_at(path: &Path) -> Option<PicoclawInstallStatus> {
    let (bytes, _) = crate::remote_download::read_worker_file(path, 4096)?;
    let status: PicoclawInstallStatus = serde_json::from_slice(&bytes).ok()?;
    let running = matches!(
        status.stage.as_str(),
        "downloading" | "verifying" | "extracting" | "installing"
    );
    if running && !crate::remote_download::download_running() {
        return None;
    }
    Some(status)
}

pub(crate) async fn gateway_reachable() -> bool {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, PICOCLAW_GATEWAY_PORT));
    matches!(
        tokio::time::timeout(
            Duration::from_millis(500),
            tokio::net::TcpStream::connect(address)
        )
        .await,
        Ok(Ok(_))
    )
}

pub(crate) async fn runtime_status(state: &AppState, picoclaw: &Picoclaw) -> RuntimeStatus {
    let mut status = RuntimeStatus {
        install_path: PICOCLAW_BINARY,
        checked_at: rfc3339_now(),
        current_session: picoclaw.owner().unwrap_or_default(),
        ..RuntimeStatus::default()
    };
    if let Some(install) = install_progress() {
        match install.stage.as_str() {
            "installed" => {}
            "install_failed" if !Path::new(PICOCLAW_BINARY).is_file() => {
                status.status = "install_failed";
                status.install_stage = install.stage;
                status.last_error = install.error;
                return status;
            }
            "install_failed" => {}
            _ => {
                status.installing = true;
                status.install_progress = install.progress;
                status.install_stage = install.stage;
                status.status = "installing";
                return status;
            }
        }
    }
    status.installed = Path::new(PICOCLAW_BINARY).is_file();
    if !status.installed {
        status.status = "not_installed";
        return status;
    }
    status.install_progress = 100;
    status.install_stage = "installed".into();
    let facts = match facts(state).await {
        Ok(facts) => facts,
        Err(error) => {
            status.status = "config_error";
            status.config_error = error.message;
            return status;
        }
    };
    status.agent_profile = if facts.config_present {
        facts.agent_profile
    } else {
        "kvm".into()
    };
    status.model_name = facts.model_name;
    status.model_configured = facts.model_configured;
    if !facts.model_configured {
        status.status = "model_not_configured";
        return status;
    }
    if gateway_reachable().await {
        status.ready = true;
        status.status = "ready";
    } else {
        status.status = "stopped";
        status.last_error = picoclaw.last_error();
    }
    status
}

pub(crate) async fn get_runtime_status(State(state): State<AppState>) -> Response {
    let picoclaw = match enabled(&state) {
        Ok(picoclaw) => picoclaw,
        Err(response) => return *response,
    };
    success(runtime_status(&state, picoclaw).await)
}

async fn prepare(state: &AppState, picoclaw: &Picoclaw) -> PicoResult<()> {
    helper(
        &state.options.picoclaw_socket,
        AdminRequest::PicoclawPrepare {
            internal_token: picoclaw.internal_token.clone(),
            mcp_url: picoclaw.mcp_url.clone(),
        },
    )
    .await
    .map(|_| ())
}

async fn service(state: &AppState, start: bool) -> PicoResult<()> {
    helper(
        &state.options.admin_socket,
        AdminRequest::PicoclawService { start },
    )
    .await
    .map(|_| ())
}

async fn start_runtime(state: &AppState, picoclaw: &Picoclaw) -> PicoResult<()> {
    if !Path::new(PICOCLAW_BINARY).is_file() {
        return Err(PicoclawError::new(
            RUNTIME_START_FAILED,
            "picoclaw is not installed",
        ));
    }
    prepare(state, picoclaw).await?;
    service(state, true).await.map_err(|error| {
        picoclaw.set_error(&error.message);
        PicoclawError::new(RUNTIME_START_FAILED, error.message)
    })?;
    let deadline = Instant::now() + READY_TIMEOUT;
    while Instant::now() < deadline {
        if gateway_reachable().await {
            picoclaw.set_error("");
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    picoclaw.set_error("gateway is unavailable");
    Err(PicoclawError::new(
        RUNTIME_START_FAILED,
        "gateway is unavailable",
    ))
}

/// Stops the runtime and closes the relayed session; used by the stop route
/// and when the switch is turned off.
pub(crate) async fn stop_runtime(state: &AppState) -> PicoResult<()> {
    if let Some(picoclaw) = state.picoclaw.as_deref() {
        gateway::close_active(state, picoclaw, 1000, "runtime stopped").await;
    }
    service(state, false).await
}

#[derive(Serialize)]
struct RuntimeStartResult {
    started: bool,
    command: &'static str,
    output: String,
    status: RuntimeStatus,
}

pub(crate) async fn start(State(state): State<AppState>) -> Response {
    let picoclaw = match enabled(&state) {
        Ok(picoclaw) => picoclaw,
        Err(response) => return *response,
    };
    if let Err(error) = start_runtime(&state, picoclaw).await {
        return error.into_response();
    }
    success(RuntimeStartResult {
        started: true,
        command: "start",
        output: String::new(),
        status: runtime_status(&state, picoclaw).await,
    })
}

pub(crate) async fn stop(State(state): State<AppState>) -> Response {
    let picoclaw = match enabled(&state) {
        Ok(picoclaw) => picoclaw,
        Err(response) => return *response,
    };
    if let Err(error) = stop_runtime(&state).await {
        return PicoclawError::new(RUNTIME_START_FAILED, error.message).into_response();
    }
    success(RuntimeStartResult {
        started: false,
        command: "stop",
        output: String::new(),
        status: runtime_status(&state, picoclaw).await,
    })
}

#[derive(Serialize)]
struct RuntimeInstallResult {
    installed: bool,
    binary: &'static str,
    download: &'static str,
    output: &'static str,
    status: RuntimeStatus,
}

pub(crate) async fn install(State(state): State<AppState>) -> Response {
    let picoclaw = match enabled(&state) {
        Ok(picoclaw) => picoclaw,
        Err(response) => return *response,
    };
    let output = if Path::new(PICOCLAW_BINARY).is_file() {
        "picoclaw is already installed"
    } else {
        match helper(
            &state.options.download_socket,
            AdminRequest::PicoclawInstall {},
        )
        .await
        {
            Ok(_) => "picoclaw installation started",
            Err(error) => {
                return PicoclawError::new(RUNTIME_START_FAILED, error.message).into_response();
            }
        }
    };
    let status = runtime_status(&state, picoclaw).await;
    success(RuntimeInstallResult {
        installed: status.installed && !status.installing,
        binary: PICOCLAW_BINARY,
        download: PICOCLAW_RELEASE_URL,
        output,
        status,
    })
}

pub(crate) async fn uninstall(State(state): State<AppState>) -> Response {
    let picoclaw = match enabled(&state) {
        Ok(picoclaw) => picoclaw,
        Err(response) => return *response,
    };
    if install_progress()
        .is_some_and(|install| !matches!(install.stage.as_str(), "installed" | "install_failed"))
    {
        return PicoclawError::new(
            RUNTIME_START_FAILED,
            "cannot uninstall while installation is in progress",
        )
        .into_response();
    }
    let mut output = "picoclaw uninstalled successfully".to_string();
    if let Err(error) = stop_runtime(&state).await {
        output = format!(
            "picoclaw uninstalled successfully (stop failed before uninstall: {})",
            error.message
        );
    }
    for (socket, request) in [
        (
            &state.options.picoclaw_socket,
            AdminRequest::PicoclawReset {},
        ),
        (
            &state.options.download_socket,
            AdminRequest::PicoclawRemove {},
        ),
    ] {
        if let Err(error) = helper(socket, request).await {
            return PicoclawError::new(RUNTIME_START_FAILED, error.message).into_response();
        }
    }
    let status = runtime_status(&state, picoclaw).await;
    success(json!({
        "installed": false,
        "binary": PICOCLAW_BINARY,
        "download": PICOCLAW_RELEASE_URL,
        "output": output,
        "status": status,
    }))
}

#[derive(Debug, Deserialize)]
pub(crate) struct ModelBody {
    #[serde(default)]
    model: String,
    #[serde(default)]
    api_base: String,
    #[serde(default)]
    api_key: String,
}

pub(crate) async fn model_config(
    State(state): State<AppState>,
    Json(body): Json<ModelBody>,
) -> Response {
    let picoclaw = match enabled(&state) {
        Ok(picoclaw) => picoclaw,
        Err(response) => return *response,
    };
    let was_ready = gateway_reachable().await;
    if let Err(error) = prepare(&state, picoclaw).await {
        return error.into_response();
    }
    let model_name = match helper(
        &state.options.picoclaw_socket,
        AdminRequest::PicoclawModel {
            model: body.model,
            api_base: body.api_base,
            api_key: body.api_key,
        },
    )
    .await
    {
        Ok(AdminReply::PicoclawModelSaved { model_name }) => model_name,
        Ok(_) => {
            return PicoclawError::new(RUNTIME_UNAVAILABLE, "unexpected picoclaw helper answer")
                .into_response();
        }
        Err(error) => return error.into_response(),
    };
    if was_ready {
        let restarted = match stop_runtime(&state).await {
            Ok(()) => start_runtime(&state, picoclaw).await,
            Err(error) => Err(error),
        };
        if let Err(error) = restarted {
            return PicoclawError::new(
                RUNTIME_UNAVAILABLE,
                format!(
                    "model config saved, but failed to restart picoclaw runtime: {}",
                    error.message
                ),
            )
            .into_response();
        }
    }
    success(json!({
        "model_name": model_name,
        "status": runtime_status(&state, picoclaw).await,
    }))
}

#[derive(Debug, Deserialize)]
pub(crate) struct ProfileBody {
    #[serde(default)]
    profile: String,
}

pub(crate) async fn agent_profile(
    State(state): State<AppState>,
    Json(body): Json<ProfileBody>,
) -> Response {
    let picoclaw = match enabled(&state) {
        Ok(picoclaw) => picoclaw,
        Err(response) => return *response,
    };
    let profile = body.profile.trim().to_string();
    if !matches!(profile.as_str(), "default" | "kvm") {
        return PicoclawError::new(INVALID_ACTION, "invalid agent profile").into_response();
    }
    if let Err(error) = prepare(&state, picoclaw).await {
        return error.into_response();
    }
    if let Err(error) = helper(
        &state.options.picoclaw_socket,
        AdminRequest::PicoclawProfile {
            profile: profile.clone(),
        },
    )
    .await
    {
        return error.into_response();
    }
    success(json!({
        "profile": profile,
        "status": runtime_status(&state, picoclaw).await,
    }))
}

#[derive(Debug, Deserialize)]
pub(crate) struct SessionsQuery {
    #[serde(default)]
    offset: u32,
    #[serde(default)]
    limit: u32,
}

pub(crate) async fn list_sessions(
    State(state): State<AppState>,
    Query(query): Query<SessionsQuery>,
) -> Response {
    if let Err(response) = enabled(&state) {
        return *response;
    }
    match helper(
        &state.options.picoclaw_socket,
        AdminRequest::PicoclawSessions {
            offset: query.offset,
            limit: query.limit,
        },
    )
    .await
    {
        Ok(AdminReply::PicoclawSessions { items }) => success(items),
        Ok(_) => PicoclawError::new(RUNTIME_UNAVAILABLE, "unexpected picoclaw helper answer")
            .into_response(),
        Err(error) => error.into_response(),
    }
}

pub(crate) async fn get_session(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
) -> Response {
    if let Err(response) = enabled(&state) {
        return *response;
    }
    match helper(
        &state.options.picoclaw_socket,
        AdminRequest::PicoclawSession { id },
    )
    .await
    {
        Ok(AdminReply::PicoclawSession { detail }) => success(detail),
        Ok(_) => PicoclawError::new(RUNTIME_UNAVAILABLE, "unexpected picoclaw helper answer")
            .into_response(),
        Err(_) => PicoclawError::new(RUNTIME_UNAVAILABLE, "session not found")
            .with_status(StatusCode::NOT_FOUND)
            .into_response(),
    }
}

pub(crate) async fn delete_session(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
) -> Response {
    if let Err(response) = enabled(&state) {
        return *response;
    }
    match helper(
        &state.options.picoclaw_socket,
        AdminRequest::PicoclawSessionDelete { id: id.clone() },
    )
    .await
    {
        Ok(_) => success(json!({"id": id, "deleted": true})),
        Err(_) => PicoclawError::new(RUNTIME_UNAVAILABLE, "session not found")
            .with_status(StatusCode::NOT_FOUND)
            .into_response(),
    }
}

/// `DELETE /api/picoclaw/runtime/session`: the browser gives the lock back.
pub(crate) async fn release_session(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let picoclaw = match enabled(&state) {
        Ok(picoclaw) => picoclaw,
        Err(response) => return *response,
    };
    let Some(session_id) = headers
        .get(SESSION_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
    else {
        return PicoclawError::new(SESSION_ID_MISSING, "missing X-PicoClaw-Session-ID")
            .into_response();
    };
    gateway::close_session(&state, picoclaw, &session_id, 1000, "session released").await;
    picoclaw.release(&session_id);
    success(json!({
        "released": true,
        "current_session": picoclaw.owner().unwrap_or_default(),
    }))
}

/// Called when the switch is turned off: the runtime must not keep running.
#[cfg_attr(not(feature = "restricted-control-surface"), allow(dead_code))]
pub(crate) async fn switched_off(state: &AppState) {
    if state.picoclaw.is_some() && state.options.admin_socket.is_some() {
        if let Err(error) = stop_runtime(state).await {
            eprintln!(
                "picoclaw: stop after switching off failed: {}",
                error.message
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_install_status_is_read_only_as_a_small_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let installed = serde_json::to_vec(&PicoclawInstallStatus {
            stage: "installed".into(),
            progress: 100,
            error: String::new(),
        })
        .unwrap();
        let path = dir.path().join("picoclaw-install.json");
        std::fs::write(&path, &installed).unwrap();
        assert!(install_progress_at(&path).is_some());
        // A link to a status elsewhere is not followed.
        let linked = dir.path().join("linked.json");
        std::os::unix::fs::symlink(&path, &linked).unwrap();
        assert!(install_progress_at(&linked).is_none());
        // Nor is one larger than a status.
        let mut large = installed.clone();
        large.resize(8192, b' ');
        std::fs::write(&path, large).unwrap();
        assert!(install_progress_at(&path).is_none());
        // A FIFO does not hold the reader.
        let fifo = dir.path().join("fifo.json");
        crate::remote_download::mkfifo(&fifo);
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(install_progress_at(&fifo).is_none());
        });
        assert_eq!(
            receiver
                .recv_timeout(std::time::Duration::from_secs(1))
                .ok(),
            Some(true),
            "a FIFO held the install status read"
        );
    }

    #[test]
    fn the_lock_serves_one_session_and_expires() {
        let dir = tempfile::tempdir().unwrap();
        let picoclaw = Picoclaw::new(dir.path(), "http://127.0.0.1:18080/x".into());
        assert_eq!(picoclaw.internal_token().len(), 64);
        let again = Picoclaw::new(dir.path(), "http://127.0.0.1:18080/x".into());
        assert_eq!(again.internal_token(), picoclaw.internal_token());
        assert_eq!(picoclaw.acquire("a"), Ok(true));
        assert_eq!(picoclaw.acquire("a"), Ok(false));
        assert_eq!(picoclaw.acquire("b"), Err("a".into()));
        assert!(picoclaw.blocks_manual_input());
        assert!(picoclaw.renew("a") && !picoclaw.renew("b"));
        picoclaw.release("b");
        assert_eq!(picoclaw.owner().as_deref(), Some("a"));
        picoclaw.release("a");
        assert!(!picoclaw.blocks_manual_input());
        picoclaw.lock.lock().unwrap().owner = Some(("c".into(), Instant::now()));
        assert_eq!(picoclaw.owner(), None);
    }

    #[test]
    fn an_upgrade_that_never_runs_releases_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let picoclaw = Arc::new(Picoclaw::new(dir.path(), "http://127.0.0.1:18080/x".into()));
        assert_eq!(picoclaw.acquire("a"), Ok(true));
        let guard = ReleaseOnDrop {
            picoclaw: picoclaw.clone(),
            session_id: "a".into(),
        };
        // The closure handed to `on_upgrade` owns the guard; dropping it
        // unrun is what a failed upgrade does.
        let closure = move || drop(guard);
        assert!(picoclaw.blocks_manual_input());
        drop(closure);
        assert!(!picoclaw.blocks_manual_input());
    }

    #[test]
    fn errors_keep_the_reference_shape() {
        let response = PicoclawError::new(LOCK_HELD, "another PicoClaw session is running")
            .with_session("a".into())
            .into_response();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(rfc3339_now().ends_with('Z'));
    }
}
