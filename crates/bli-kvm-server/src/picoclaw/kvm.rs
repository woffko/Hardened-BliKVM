//! The runtime's KVM tools on an internal loopback route: MCP (JSON-RPC over
//! HTTP), screenshot, HID actions and the load-image instruction.
//!
//! Only direct loopback calls with the internal token reach these handlers:
//! the TLS proxy always adds `X-Forwarded-For`, so a proxied request is
//! refused, and so is any call while the switch is off. HID reports go
//! through the HID lease of the operator session that opened the assistant.

use std::net::SocketAddr;
use std::pin::Pin;
use std::time::{Duration, Instant};

use axum::{
    Json, Router,
    body::Body,
    extract::{ConnectInfo, Query, State},
    http::{Request, StatusCode, header},
    middleware::{Next, from_fn_with_state},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use bli_kvm_admin_helper::INTERNAL_TOKEN_HEADER;
use bli_kvm_control_protocol::ControlResponse;
use bli_kvm_core::{HidLeaseRequest, HidReport, PlatformControlRequestV2};
use bytes::Bytes;
use http_body::Body as _;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{INVALID_ACTION, PicoclawError, RUNTIME_UNAVAILABLE, keys, success};
use crate::{AppState, auth::session::Session};

const CLICK_HOLD: Duration = Duration::from_millis(40);
const KEY_DELAY: Duration = Duration::from_millis(30);
const SCROLL_STEP: Duration = Duration::from_millis(20);
const DRAG_STEPS: u32 = 10;
const MAX_ACTIONS: usize = 100;
const MAX_TEXT_CHARS: usize = 4000;
const MAX_WAIT_MS: u64 = 10_000;
const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
const FRAME_TIMEOUT: Duration = Duration::from_secs(3);

pub(crate) fn internal_router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/mcp", post(mcp))
        .route("/screenshot", get(screenshot))
        .route("/actions", post(actions))
        .route("/load-image", post(load_image))
        .route("/runtime/session", get(runtime_session))
        .route_layer(from_fn_with_state(state, guard))
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0_u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

async fn guard(State(state): State<AppState>, request: Request<Body>, next: Next) -> Response {
    let loopback = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .is_some_and(|ConnectInfo(peer)| peer.ip().is_loopback());
    let proxied = request.headers().contains_key("x-forwarded-for");
    let token_ok = state.picoclaw.as_deref().is_some_and(|picoclaw| {
        request
            .headers()
            .get(INTERNAL_TOKEN_HEADER)
            .is_some_and(|value| {
                constant_time_eq(value.as_bytes(), picoclaw.internal_token().as_bytes())
            })
    });
    if !loopback || proxied || !token_ok {
        return StatusCode::NOT_FOUND.into_response();
    }
    if !crate::admin_features::switches().picoclaw {
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(request).await
}

/// The relayed session's operator; actions need one.
async fn operator(state: &AppState) -> Result<(String, Session), PicoclawError> {
    let picoclaw = state
        .picoclaw
        .as_deref()
        .ok_or_else(|| PicoclawError::new(RUNTIME_UNAVAILABLE, "picoclaw is not configured"))?;
    let active = picoclaw.active.lock().await;
    let active = active.as_ref().ok_or_else(|| {
        PicoclawError::new(RUNTIME_UNAVAILABLE, "picoclaw session is not connected")
    })?;
    if !active.operator.is_active() {
        return Err(PicoclawError::new(
            RUNTIME_UNAVAILABLE,
            "the operator session has ended",
        ));
    }
    Ok((active.session_id.clone(), active.operator.clone()))
}

fn operation_id() -> String {
    use rand_core::{OsRng, RngCore};
    let mut bytes = [0_u8; 16];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// One HID report through the operator's live lease.
async fn send(state: &AppState, operator: &Session, report: HidReport) -> Result<(), String> {
    let control = state
        .control
        .as_ref()
        .ok_or("HID control channel is not configured")?;
    let live = state
        .hid_ws_leases
        .lock()
        .await
        .live(&operator.token, Instant::now());
    let (lease_id, bindings) =
        live.ok_or("remote control is off on the operator's KVM page; turn it on to let the assistant use the keyboard and mouse")?;
    let request = PlatformControlRequestV2::HidLease {
        lease: HidLeaseRequest::Report {
            lease_id,
            session_binding: bindings.session_binding,
            target_binding: bindings.target_binding,
            report,
        },
    };
    match control.execute_v2(&operation_id(), request).await {
        Ok(ControlResponse::Applied) => Ok(()),
        Ok(ControlResponse::Rejected { code }) => Err(format!(
            "HID write rejected: {}",
            crate::hid::rejection_name(code)
        )),
        _ => Err("HID write failed".into()),
    }
}

/// Releases every key and button through the operator's lease.
pub(crate) async fn release_all(state: &AppState, operator: &Session) {
    let _ = send(state, operator, HidReport::Keyboard { data: [0; 8] }).await;
    let _ = send(
        state,
        operator,
        HidReport::AbsoluteMouse {
            data: [0, 0, 0, 0, 0, 0],
        },
    )
    .await;
}

fn absolute(value: f64) -> u16 {
    (value.clamp(0.0, 1.0) * f64::from(0x7fff_u16)).round() as u16
}

fn mouse(x: f64, y: f64, buttons: u8, wheel: i8) -> HidReport {
    let (x, y) = (absolute(x), absolute(y));
    HidReport::AbsoluteMouse {
        data: [
            buttons,
            (x & 0xff) as u8,
            (x >> 8) as u8,
            (y & 0xff) as u8,
            (y >> 8) as u8,
            wheel as u8,
        ],
    }
}

#[derive(Debug, Clone, Deserialize)]
struct Point {
    x: Option<f64>,
    y: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum Keys {
    List(Vec<String>),
    Csv(String),
}

impl Keys {
    fn names(&self) -> Vec<String> {
        match self {
            Keys::List(list) => list.clone(),
            Keys::Csv(csv) => csv.split(',').map(str::to_string).collect(),
        }
        .into_iter()
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty())
        .collect()
    }
}

#[derive(Debug, Clone, Deserialize)]
struct Action {
    action: String,
    x: Option<f64>,
    y: Option<f64>,
    from: Option<Point>,
    to: Option<Point>,
    #[serde(default)]
    button: String,
    #[serde(default)]
    text: String,
    keys: Option<Keys>,
    #[serde(default)]
    direction: String,
    #[serde(default)]
    amount: i64,
    #[serde(default)]
    duration_ms: i64,
}

fn point(x: Option<f64>, y: Option<f64>) -> Result<(f64, f64), String> {
    match (x, y) {
        (Some(x), Some(y)) if (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y) => Ok((x, y)),
        (Some(_), Some(_)) => Err("coordinates must be within [0,1]".into()),
        _ => Err("action requires x and y".into()),
    }
}

fn button(name: &str) -> Result<u8, String> {
    Ok(match name.trim().to_ascii_lowercase().as_str() {
        "" | "left" => 1 << 0,
        "right" => 1 << 1,
        "middle" => 1 << 2,
        "back" => 1 << 3,
        "forward" => 1 << 4,
        _ => return Err("invalid mouse button".into()),
    })
}

/// Runs one action; returns the number of HID reports written.
async fn execute(state: &AppState, operator: &Session, action: &Action) -> Result<u32, String> {
    let mut writes = 0;
    macro_rules! out {
        ($report:expr) => {{
            send(state, operator, $report).await?;
            writes += 1;
        }};
    }
    match action.action.trim().to_ascii_lowercase().as_str() {
        "click" => {
            let (x, y) = point(action.x, action.y)?;
            let pressed = button(&action.button)?;
            out!(mouse(x, y, 0, 0));
            out!(mouse(x, y, pressed, 0));
            tokio::time::sleep(CLICK_HOLD).await;
            out!(mouse(x, y, 0, 0));
        }
        "move" => {
            let (x, y) = point(action.x, action.y)?;
            out!(mouse(x, y, 0, 0));
        }
        "wait" => {
            if action.duration_ms < 0 {
                return Err("wait duration must be >= 0".into());
            }
            let wait = (action.duration_ms as u64).min(MAX_WAIT_MS);
            tokio::time::sleep(Duration::from_millis(wait)).await;
        }
        "drag" => {
            let from = action
                .from
                .as_ref()
                .ok_or("action requires point coordinates")?;
            let to = action
                .to
                .as_ref()
                .ok_or("action requires point coordinates")?;
            let (from_x, from_y) = point(from.x, from.y)?;
            let (to_x, to_y) = point(to.x, to.y)?;
            let pressed = button(&action.button)?;
            out!(mouse(from_x, from_y, 0, 0));
            out!(mouse(from_x, from_y, pressed, 0));
            for step in 1..=DRAG_STEPS {
                let ratio = f64::from(step) / f64::from(DRAG_STEPS);
                out!(mouse(
                    from_x + (to_x - from_x) * ratio,
                    from_y + (to_y - from_y) * ratio,
                    pressed,
                    0
                ));
            }
            out!(mouse(to_x, to_y, 0, 0));
        }
        "scroll" => {
            let (x, y) = if action.x.is_some() || action.y.is_some() {
                point(action.x, action.y)?
            } else {
                (0.5, 0.5)
            };
            let amount = match action.amount {
                0 => 1,
                amount if amount < 0 => return Err("scroll amount must be > 0".into()),
                amount => amount.min(50),
            };
            let wheel: i8 = match action.direction.trim().to_ascii_lowercase().as_str() {
                "" | "up" => 1,
                "down" => -1,
                _ => return Err("invalid scroll direction".into()),
            };
            for _ in 0..amount {
                out!(mouse(x, y, 0, wheel));
                out!(mouse(x, y, 0, 0));
                tokio::time::sleep(SCROLL_STEP).await;
            }
        }
        "type" => {
            if action.text.is_empty() {
                return Err("type requires text".into());
            }
            if action.text.chars().count() > MAX_TEXT_CHARS {
                return Err("type text is too long".into());
            }
            let mut reports = Vec::new();
            for ch in action.text.chars() {
                let (modifiers, code) =
                    keys::character(ch).ok_or("unsupported character in type action")?;
                reports.push([modifiers, 0, code, 0, 0, 0, 0, 0]);
            }
            for report in reports {
                out!(HidReport::Keyboard { data: report });
                out!(HidReport::Keyboard { data: [0; 8] });
                tokio::time::sleep(KEY_DELAY).await;
            }
        }
        "hotkey" => {
            let names = action.keys.as_ref().map(Keys::names).unwrap_or_default();
            let report = keys::hotkey_report(&names)?;
            out!(HidReport::Keyboard { data: report });
            tokio::time::sleep(CLICK_HOLD).await;
            out!(HidReport::Keyboard { data: [0; 8] });
        }
        _ => return Err("unknown or invalid action".into()),
    }
    Ok(writes)
}

/// Runs a batch; on failure every key and button is released.
async fn execute_all(state: &AppState, actions: &[Action]) -> Result<Value, (String, usize)> {
    if actions.is_empty() {
        return Err(("empty actions".into(), 0));
    }
    if actions.len() > MAX_ACTIONS {
        return Err(("too many actions".into(), 0));
    }
    let (session_id, operator) = operator(state)
        .await
        .map_err(|error| (error_text(error), 0))?;
    let picoclaw = state
        .picoclaw
        .as_deref()
        .expect("operator implies picoclaw");
    let started = Instant::now();
    let mut writes = 0;
    for (index, action) in actions.iter().enumerate() {
        if !picoclaw.renew(&session_id) {
            release_all(state, &operator).await;
            return Err(("another PicoClaw session is running".into(), index));
        }
        match execute(state, &operator, action).await {
            Ok(count) => writes += count,
            Err(error) => {
                release_all(state, &operator).await;
                return Err((error, index));
            }
        }
    }
    Ok(json!({
        "action": if actions.len() > 1 { "batch" } else { actions[0].action.as_str() },
        "duration_ms": started.elapsed().as_millis() as u64,
        "hid_writes": writes,
        "executed_actions": actions.len(),
    }))
}

fn error_text(error: PicoclawError) -> String {
    error.message
}

fn parse_actions(value: &Value) -> Result<Vec<Action>, String> {
    if let Some(list) = value.get("actions") {
        return serde_json::from_value(list.clone()).map_err(|_| "invalid actions payload".into());
    }
    serde_json::from_value::<Action>(value.clone())
        .map(|action| vec![action])
        .map_err(|_| "invalid action payload".into())
}

async fn actions(State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    let actions = match parse_actions(&body) {
        Ok(actions) => actions,
        Err(message) => return PicoclawError::new(INVALID_ACTION, message).into_response(),
    };
    match execute_all(&state, &actions).await {
        Ok(result) => success(result),
        Err((message, index)) => {
            let mut response = Json(json!({
                "code": INVALID_ACTION,
                "message": message,
                "msg": message,
                "index": index,
            }))
            .into_response();
            *response.status_mut() = StatusCode::OK;
            response
        }
    }
}

/// Reads the next complete JPEG from the MJPEG stream.
async fn frame(state: &AppState) -> Result<Vec<u8>, String> {
    let stream = state
        .video
        .mjpeg()
        .await
        .map_err(|_| "failed to capture screenshot")?;
    let mut body = Box::pin(stream.body);
    let mut buffer: Vec<u8> = Vec::new();
    let deadline = tokio::time::Instant::now() + FRAME_TIMEOUT;
    loop {
        if let Some(jpeg) = complete_jpeg(&buffer) {
            return Ok(jpeg.to_vec());
        }
        if buffer.len() > MAX_FRAME_BYTES {
            return Err("failed to capture screenshot".into());
        }
        let next = tokio::time::timeout_at(
            deadline,
            std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)),
        )
        .await
        .map_err(|_| "no HDMI signal or frame unavailable")?;
        match next {
            Some(Ok(frame)) => {
                if let Ok(data) = frame.into_data() {
                    let data: Bytes = data;
                    buffer.extend_from_slice(&data);
                }
            }
            _ => return Err("failed to capture screenshot".into()),
        }
    }
}

/// The first whole JPEG in `data`: SOI, marker segments, entropy-coded
/// data up to EOI.
pub(crate) fn complete_jpeg(data: &[u8]) -> Option<&[u8]> {
    let start = data
        .windows(3)
        .position(|window| window == [0xff, 0xd8, 0xff])?;
    let jpeg = &data[start..];
    let mut index = 2;
    loop {
        while jpeg.get(index) == Some(&0xff) && jpeg.get(index + 1) == Some(&0xff) {
            index += 1;
        }
        if jpeg.get(index)? != &0xff {
            return None;
        }
        let marker = *jpeg.get(index + 1)?;
        match marker {
            0xd9 => return Some(&jpeg[..index + 2]),
            0x01 | 0xd0..=0xd7 => index += 2,
            _ => {
                let length = usize::from(u16::from_be_bytes([
                    *jpeg.get(index + 2)?,
                    *jpeg.get(index + 3)?,
                ]));
                if length < 2 {
                    return None;
                }
                index += 2 + length;
                if marker == 0xda {
                    // Entropy-coded data: stop at a marker that is not a
                    // stuffed zero or a restart marker.
                    loop {
                        let byte = *jpeg.get(index)?;
                        let next = *jpeg.get(index + 1)?;
                        if byte == 0xff && next != 0x00 && !(0xd0..=0xd7).contains(&next) {
                            break;
                        }
                        index += 1;
                    }
                }
            }
        }
    }
}

/// Width and height from the first SOF segment.
pub(crate) fn jpeg_size(jpeg: &[u8]) -> Option<(u16, u16)> {
    let mut index = 2;
    while index + 9 < jpeg.len() {
        if jpeg[index] != 0xff {
            return None;
        }
        let marker = jpeg[index + 1];
        let length = usize::from(u16::from_be_bytes([jpeg[index + 2], jpeg[index + 3]]));
        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
            let height = u16::from_be_bytes([jpeg[index + 5], jpeg[index + 6]]);
            let width = u16::from_be_bytes([jpeg[index + 7], jpeg[index + 8]]);
            return Some((width, height));
        }
        index += 2 + length;
    }
    None
}

#[derive(Debug, Deserialize)]
struct ScreenshotQuery {
    #[serde(default)]
    format: String,
}

async fn screenshot(
    State(state): State<AppState>,
    Query(query): Query<ScreenshotQuery>,
) -> Response {
    let jpeg = match frame(&state).await {
        Ok(jpeg) => jpeg,
        Err(message) => {
            return PicoclawError::new("SCREENSHOT_FAILED", message).into_response();
        }
    };
    if query.format == "base64" {
        let (width, height) = jpeg_size(&jpeg).unwrap_or((0, 0));
        return success(json!({
            "image_base64": STANDARD.encode(&jpeg),
            "source_width": width,
            "source_height": height,
            "capture_width": width,
            "capture_height": height,
            "format": "jpeg",
        }));
    }
    ([(header::CONTENT_TYPE, "image/jpeg")], jpeg).into_response()
}

fn message_id() -> String {
    let hex = operation_id();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// Shows a captured screenshot in the browser's chat, as the reference does.
async fn observe(state: &AppState, image: &str) {
    let Some(picoclaw) = state.picoclaw.as_deref() else {
        return;
    };
    let active = picoclaw.active.lock().await;
    if let Some(active) = active.as_ref() {
        let message = json!({
            "type": "observation",
            "id": message_id(),
            "session_id": active.session_id,
            "timestamp": now_millis(),
            "payload": {
                "content": "screenshot captured",
                "image_base64": image,
                "mime_type": "image/jpeg",
            },
        });
        let _ = active.downstream.try_send(message.to_string());
    }
}

fn rpc_result(id: &Value, result: Value) -> Response {
    Json(json!({"jsonrpc": "2.0", "id": id, "result": result})).into_response()
}

fn rpc_error(id: &Value, code: i32, message: &str) -> Response {
    Json(json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}))
        .into_response()
}

fn tool_error(id: &Value, message: &str) -> Response {
    rpc_result(
        id,
        json!({"isError": true, "content": [{"type": "text", "text": message}]}),
    )
}

fn tool_definitions() -> Value {
    json!([
        {
            "name": "kvm_screenshot",
            "description": "Capture the current HDMI frame from the downstream remote host as a base64-encoded JPEG image.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "width": {"type": "integer", "description": "Target width in pixels (optional; the full frame is returned on this board)"},
                    "height": {"type": "integer", "description": "Target height in pixels (optional)"},
                    "quality": {"type": "integer", "description": "JPEG quality 1-100 (optional)"}
                }
            }
        },
        {
            "name": "kvm_actions",
            "description": "Send one or more HID actions (click, type, hotkey, scroll, drag, move, wait) to the downstream remote host. Use normalized [0,1] coordinates for mouse actions.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "actions": {
                        "type": "array",
                        "description": "Array of action objects. Each requires an 'action' field (click, move, type, hotkey, scroll, drag, wait).",
                        "items": {
                            "type": "object",
                            "properties": {
                                "action": {"type": "string"},
                                "x": {"type": "number"},
                                "y": {"type": "number"},
                                "button": {"type": "string"},
                                "text": {"type": "string"},
                                "keys": {"type": "array", "items": {"type": "string"}},
                                "direction": {"type": "string"},
                                "amount": {"type": "integer"},
                                "duration_ms": {"type": "integer"},
                                "from": {"type": "object", "properties": {"x": {"type": "number"}, "y": {"type": "number"}}},
                                "to": {"type": "object", "properties": {"x": {"type": "number"}, "y": {"type": "number"}}}
                            },
                            "required": ["action"]
                        }
                    }
                },
                "required": ["actions"]
            }
        }
    ])
}

/// MCP over HTTP (JSON-RPC 2.0), answering exactly as the reference does.
async fn mcp(State(state): State<AppState>, body: Bytes) -> Response {
    let Ok(request) = serde_json::from_slice::<Value>(&body) else {
        let mut response = rpc_error(&Value::Null, -32700, "parse error");
        *response.status_mut() = StatusCode::BAD_REQUEST;
        return response;
    };
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    if request.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return rpc_error(&id, -32600, "invalid request: jsonrpc must be 2.0");
    }
    match request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "initialize" => rpc_result(
            &id,
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "blikvm", "version": "1.0.0"},
            }),
        ),
        "tools/list" => rpc_result(&id, json!({"tools": tool_definitions()})),
        "ping" => rpc_result(&id, json!({})),
        "tools/call" => {
            let params = request.get("params").cloned().unwrap_or(Value::Null);
            let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
            match params.get("name").and_then(Value::as_str) {
                Some("kvm_screenshot") => match frame(&state).await {
                    Ok(jpeg) => {
                        let image = STANDARD.encode(&jpeg);
                        observe(&state, &image).await;
                        let (width, height) = jpeg_size(&jpeg).unwrap_or((0, 0));
                        rpc_result(
                            &id,
                            json!({
                                "content": [
                                    {"type": "text", "text": "screenshot captured"},
                                    {"type": "image", "data": image, "mimeType": "image/jpeg"},
                                ],
                                "meta": {
                                    "source_width": width,
                                    "source_height": height,
                                    "capture_width": width,
                                    "capture_height": height,
                                },
                            }),
                        )
                    }
                    Err(message) => tool_error(&id, &message),
                },
                Some("kvm_actions") => {
                    let list = arguments.get("actions").cloned().unwrap_or(Value::Null);
                    let actions: Vec<Action> = match serde_json::from_value(list) {
                        Ok(actions) => actions,
                        Err(_) => return tool_error(&id, "invalid actions payload"),
                    };
                    if actions.is_empty() {
                        return tool_error(&id, "empty actions array");
                    }
                    match execute_all(&state, &actions).await {
                        Ok(result) => rpc_result(
                            &id,
                            json!({"content": [{"type": "text", "text": result.to_string()}]}),
                        ),
                        Err((message, _)) => tool_error(&id, &message),
                    }
                }
                Some(name) => rpc_error(&id, -32602, &format!("unknown tool: {name}")),
                None => rpc_error(&id, -32602, "invalid params"),
            }
        }
        method => rpc_error(&id, -32601, &format!("method not found: {method}")),
    }
}

#[derive(Debug, Deserialize)]
struct LoadImageBody {
    #[serde(default)]
    path: String,
    #[serde(default)]
    prompt: String,
}

async fn load_image(State(state): State<AppState>, Json(body): Json<LoadImageBody>) -> Response {
    let path = body.path.trim();
    if path.is_empty() || path.len() > 1024 || path.chars().any(char::is_control) {
        return PicoclawError::new(INVALID_ACTION, "image path is required").into_response();
    }
    let prompt = match body.prompt.trim() {
        "" => "Describe the image briefly.",
        prompt => prompt,
    };
    let Some(picoclaw) = state.picoclaw.as_deref() else {
        return PicoclawError::new(RUNTIME_UNAVAILABLE, "picoclaw is not configured")
            .into_response();
    };
    let active = picoclaw.active.lock().await;
    let Some(active) = active.as_ref() else {
        return PicoclawError::new(RUNTIME_UNAVAILABLE, "picoclaw session is not connected")
            .into_response();
    };
    let arguments = json!({ "path": path }).to_string();
    let instruction = format!(
        "Call `load_image` first with this exact argument object:\n```json\n{arguments}\n```\n\nDo not ask the user to re-upload the image. Do not modify the path. Do not use `read_file` or any other tool to inspect this image directly.\n\nAfter `load_image` succeeds, treat the loaded image as the current task input and continue with this request:\n{prompt}"
    );
    let id = message_id();
    let message = json!({
        "type": "message.send",
        "id": id,
        "session_id": active.session_id,
        "timestamp": now_millis(),
        "payload": {"content": instruction},
    });
    if active.upstream.try_send(message.to_string()).is_err() {
        return PicoclawError::new(
            RUNTIME_UNAVAILABLE,
            "failed to deliver image instruction to picoclaw",
        )
        .into_response();
    }
    success(json!({
        "accepted": true,
        "session_id": active.session_id,
        "message_id": id,
        "path": path,
        "user_prompt": prompt,
        "instructed_at": super::rfc3339_now(),
    }))
}

async fn runtime_session(State(state): State<AppState>) -> Response {
    let owner = state
        .picoclaw
        .as_deref()
        .and_then(super::Picoclaw::owner)
        .unwrap_or_default();
    success(json!({"current_session": owner, "checked_at": super::rfc3339_now()}))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal JPEG: SOI, SOF0 (640x480), SOS with stuffed and restart
    /// bytes in the entropy data, EOI.
    fn sample() -> Vec<u8> {
        let mut jpeg = vec![0xff, 0xd8];
        jpeg.extend([
            0xff, 0xc0, 0x00, 0x0b, 0x08, 0x01, 0xe0, 0x02, 0x80, 0x01, 0x01, 0x11, 0x00,
        ]);
        jpeg.extend([0xff, 0xda, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x3f, 0x00]);
        jpeg.extend([0x12, 0xff, 0x00, 0x34, 0xff, 0xd3, 0x56]);
        jpeg.extend([0xff, 0xd9]);
        jpeg
    }

    #[test]
    fn jpegs_are_cut_from_the_stream_and_measured() {
        let jpeg = sample();
        let mut stream = b"--frame\r\nContent-Type: image/jpeg\r\n\r\n".to_vec();
        stream.extend(&jpeg);
        stream.extend(b"\r\n--frame\r\n");
        assert_eq!(complete_jpeg(&stream), Some(jpeg.as_slice()));
        assert_eq!(complete_jpeg(&stream[..stream.len() - 20]), None);
        assert_eq!(jpeg_size(&jpeg), Some((640, 480)));
    }

    #[test]
    fn actions_parse_in_both_reference_shapes() {
        let batch = json!({"actions": [{"action": "hotkey", "keys": "ctrl, alt ,delete"}]});
        let parsed = parse_actions(&batch).unwrap();
        assert_eq!(
            parsed[0].keys.as_ref().unwrap().names(),
            ["ctrl", "alt", "delete"]
        );
        let single = json!({"action": "click", "x": 0.5, "y": 1.0});
        assert_eq!(parse_actions(&single).unwrap().len(), 1);
        assert!(point(Some(1.5), Some(0.0)).is_err());
        assert!(point(None, Some(0.0)).is_err());
        assert_eq!(absolute(1.0), 0x7fff);
        assert_eq!(absolute(0.0), 0);
        assert!(button("side").is_err());
        assert!(constant_time_eq(b"abc", b"abc") && !constant_time_eq(b"abc", b"abd"));
    }
}
