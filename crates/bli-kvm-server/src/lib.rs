use std::{path::PathBuf, sync::Arc};

use axum::{
    Extension, Json, Router,
    extract::{DefaultBodyLimit, Request, State},
    http::{HeaderName, HeaderValue, StatusCode},
    middleware::{Next, from_fn, from_fn_with_state},
    response::Response,
    routing::{delete, get, post},
};
use bli_kvm_broker_client::BrokerReadClient;
use bli_kvm_core::{
    CapabilityManifest, CapabilityProvider, DiagnosticsView, EvidenceLevel, FeatureStatus,
    RuntimeInventory, VideoTransport,
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

mod admin_features;
mod application;
mod atx;
mod auth;
mod autostart;
mod device_settings;
mod download;
mod encoder_report;
mod error;
mod firewall_modes;
mod hid;
mod hostname;
mod leader_key;
mod media;
mod mjpeg_latest;
mod network;
mod network_ipv4;
mod network_write;
mod picoclaw;
pub mod proxy_socket;
mod quality;
mod quic;
mod remote_download;
mod screen_area;
mod scripts;
mod session_lock;
mod shortcuts;
pub mod shutdown;
mod ssh;
mod state_file;
mod system_action;
mod system_log;
mod system_time;
mod system_update;
mod tailscale;
mod target_agent;
mod terminal;
mod update;
mod video;
#[cfg(feature = "restricted-control-surface")]
mod video_engine;
mod video_settings;
mod vm_info;
mod webrtc;
mod wifi;
mod wol_macs;
mod ws;

/// Login and first setup carry two short strings; anything larger is refused
/// before it is parsed.
const AUTH_BODY_LIMIT: usize = 4096;

#[derive(Clone)]
pub(crate) struct AppState {
    pub capabilities: Arc<CapabilityManifest>,
    pub inventory: Arc<BrokerReadClient>,
    pub video: Arc<VideoReadClient>,
    pub update_status: Option<Arc<UpdateStatusClient>>,
    pub update_upload: Option<Arc<UpdateUploadClient>>,
    pub update_http_upload: Arc<tokio::sync::Semaphore>,
    pub update_activation: Option<Arc<UpdateActivationClient>>,
    /// Native profile: activation is a Prepared admission followed by the
    /// coordinator's full composition transition, not a legacy apply.
    pub update_native_admission: bool,
    pub webrtc: Option<Arc<webrtc::WebRtcGateway>>,
    pub accounts: Arc<AccountStore>,
    pub sessions: Arc<SessionStore>,
    pub login_limiter: Arc<Mutex<LoginRateLimiter>>,
    pub setup_lock: Arc<Mutex<()>>,
    pub control: Option<Arc<bli_kvm_control_client::ControlClient>>,
    pub focus: Arc<Mutex<hid::FocusChallengeStore>>,
    pub hid_ws_leases: Arc<Mutex<hid::WsLeaseStore>>,
    pub terminal_tickets: Arc<Mutex<terminal::TicketStore>>,
    pub picoclaw: Option<Arc<picoclaw::Picoclaw>>,
    #[cfg_attr(not(feature = "restricted-control-surface"), allow(dead_code))]
    pub firewall_status: Arc<firewall_modes::StatusCache>,
    /// The QUIC transport's tokens and sessions; absent without a link.
    pub quic: Option<Arc<quic::QuicTransport>>,
    /// The quality monitor's answer of the last second, and what its rates
    /// are made from.
    pub quality: Arc<quality::QualityCache>,
    pub options: Arc<ServerOptions>,
}

#[derive(Debug, Clone)]
pub struct ServerOptions {
    pub shutdown: shutdown::Shutdown,
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
    /// Explicit operator opt-in for both write clients and the GUI capability.
    /// Requires all three separate IPC paths; does not prove worker liveness.
    pub update_writes_enabled: bool,
    /// Native update profile; selected by the reviewed native server unit.
    pub update_native_admission: bool,
    /// Loopback Janus WebSocket for VP8 WebRTC signaling; absent disables it.
    pub webrtc_janus: Option<std::net::SocketAddr>,
    /// The WebRTC encoder's demand socket: held open while a viewer is
    /// connected, so the encoder works only then. Absent: it always runs.
    pub webrtc_demand_socket: Option<PathBuf>,
    /// The H.264 encoder's demand socket; set, it also offers H.264 over
    /// WebRTC (Janus mountpoint 2) beside VP8.
    pub webrtc_h264_demand_socket: Option<PathBuf>,
    /// Privileged control broker used only for safe-target/focus attestation.
    /// No value keeps the HID focus endpoint unavailable.
    pub control_socket: Option<PathBuf>,
    /// Explicit opt-in for advertising the restricted control surface. Only a
    /// `restricted-control-surface` build honours it; shipped builds always
    /// keep HID and ATX `not_exposed`.
    pub restricted_controls_enabled: bool,
    /// Read-only virtual-media catalog path. No value keeps the catalog
    /// endpoint unavailable.
    pub media_catalog: Option<PathBuf>,
    /// Explicit opt-in for advertising and accepting virtual-media writes in a
    /// `restricted-control-surface` build. Requires the restricted surface and
    /// a configured control socket.
    pub media_controls_enabled: bool,
    /// Explicit opt-in for the privileged reboot and shutdown requests. The
    /// capability stays unavailable, and every request is refused, without it.
    pub system_actions_enabled: bool,
    /// The board owner applies the reviewed video-engine settings.
    pub video_controls_enabled: bool,
    /// The engine's read-only control socket (`/state` read-back for the
    /// measured capture rate). No value keeps the extra field absent.
    pub engine_socket: Option<PathBuf>,
    /// Where the H.264 encoder says which engine encodes (the board's video
    /// engine or x264). No value keeps the field absent.
    pub h264_encoder_status: Option<PathBuf>,
    /// The H.264 encoder's report: whether the board's video engine can be
    /// had (docs/design/h264-engine-choice-and-quality-monitor-v1.md), and
    /// with the VP8 encoder's, how the encoding goes for the quality monitor.
    pub h264_encoder_report: Option<PathBuf>,
    pub vp8_encoder_report: Option<PathBuf>,
    /// Where the encoder units' control groups are, for their processor
    /// time.
    pub unit_cgroups: PathBuf,
    /// Root administration helper (terminal, scripts). No value keeps those
    /// features unavailable even when switched on.
    pub admin_socket: Option<PathBuf>,
    /// Unprivileged remote image download helper. No value keeps remote
    /// download unavailable even when switched on.
    pub download_socket: Option<PathBuf>,
    /// PicoClaw configuration helper (runs as the runtime's user). With the
    /// admin and download helpers it makes the assistant available.
    pub picoclaw_socket: Option<PathBuf>,
    /// The loopback address the runtime's MCP client calls.
    pub picoclaw_mcp_url: Option<String>,
    /// Explicit opt-in for the bounded DNS and IPv6 writes. The capability
    /// stays unavailable, and every request is refused, without it.
    pub network_writes_enabled: bool,
    pub firewall_modes_enabled: bool,
    /// Image directory an operator upload writes into. Must be the same
    /// directory the board owner serves, and is required for uploads.
    pub media_directory: Option<PathBuf>,
    /// Reconcile the catalog with the image store only while the store is a
    /// filesystem of its own (the card's third partition). An unmounted store
    /// looks like an empty one, and must not empty the catalog.
    pub media_store_must_be_mounted: bool,
    /// The socket the QUIC gateway connects to. No value keeps the QUIC
    /// transport unavailable.
    pub quic_link_socket: Option<PathBuf>,
    /// The only user whose process may use that socket; no value means this
    /// server's own user.
    pub quic_link_uid: Option<u32>,
    /// The socket nginx reaches the server through, which only nginx's
    /// account and the server may open. Named, `X-Forwarded-For` is
    /// believed there alone, and a loopback TCP peer counts as itself (code
    /// audit of 2026-10-20, AUD-16.2).
    pub proxy_socket: Option<PathBuf>,
}

impl ServerOptions {
    /// Whether the server offers, and serves, the WebRTC quality: the video
    /// controls are enabled and the encoders take the bitrate over their
    /// demand sockets.
    #[cfg_attr(not(feature = "restricted-control-surface"), allow(dead_code))]
    pub(crate) fn offers_webrtc_quality(&self) -> bool {
        self.video_controls_enabled
            && self.webrtc_janus.is_some()
            && (self.webrtc_demand_socket.is_some() || self.webrtc_h264_demand_socket.is_some())
    }

    /// Whether the server offers, and serves, the board's choice of the H.264
    /// engine: the video controls are enabled, the H.264 encoder takes the
    /// choice over its demand socket, and its report is configured. The report
    /// is written by the encoder that knows the choice and its drop-in is
    /// installed with it, so it says that the encoder is there: a slot on a
    /// board whose encoder is older offers nothing that would change nothing.
    #[cfg_attr(not(feature = "restricted-control-surface"), allow(dead_code))]
    pub(crate) fn offers_engine_choice(&self) -> bool {
        self.video_controls_enabled
            && self.webrtc_janus.is_some()
            && self.webrtc_h264_demand_socket.is_some()
            && self.h264_encoder_report.is_some()
    }
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            shutdown: shutdown::Shutdown::default(),
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
            update_writes_enabled: false,
            update_native_admission: false,
            webrtc_janus: None,
            webrtc_demand_socket: None,
            webrtc_h264_demand_socket: None,
            control_socket: None,
            restricted_controls_enabled: false,
            media_catalog: None,
            media_controls_enabled: false,
            system_actions_enabled: false,
            video_controls_enabled: false,
            engine_socket: None,
            h264_encoder_status: None,
            h264_encoder_report: None,
            vp8_encoder_report: None,
            unit_cgroups: PathBuf::from("/sys/fs/cgroup/system.slice"),
            admin_socket: None,
            download_socket: None,
            picoclaw_socket: None,
            picoclaw_mcp_url: None,
            network_writes_enabled: false,
            firewall_modes_enabled: false,
            media_directory: None,
            media_store_must_be_mounted: false,
            quic_link_socket: None,
            quic_link_uid: None,
            proxy_socket: None,
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

/// The assistant needs its configuration helper plus the admin helper (start
/// and stop) and the download helper (install).
fn picoclaw_available(options: &ServerOptions) -> bool {
    options.picoclaw_socket.is_some()
        && options.admin_socket.is_some()
        && options.download_socket.is_some()
}

pub fn app_with_options(options: ServerOptions) -> Router {
    // Partial files an interrupted upload left in the image store.
    if let Some(directory) = options.media_directory.as_deref() {
        download::sweep_stale_uploads(directory);
    }
    assert!(
        !options.update_writes_enabled
            || (options.update_status_socket.is_some()
                && options.update_upload_socket.is_some()
                && options.update_activation_socket.is_some()),
        "write enablement requires configured status, upload and activation sockets"
    );
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
    // VP8 over WebRTC is offered only when the loopback gateway is configured;
    // MJPEG stays the preferred transport.
    if options
        .webrtc_janus
        .is_some_and(|address| address.ip().is_loopback())
    {
        manifest.video.transports.push(VideoTransport::WebRtcJanus);
        if options.webrtc_h264_demand_socket.is_some() {
            manifest
                .video
                .transports
                .push(VideoTransport::WebRtcJanusH264);
        }
    }
    if options.update_status_socket.is_some() {
        manifest.system.signed_application_updates = if options.update_writes_enabled {
            FeatureStatus::read_write(EvidenceLevel::SourceVerified)
        } else {
            FeatureStatus::read_only(EvidenceLevel::SourceVerified)
        };
    }
    #[cfg(feature = "restricted-control-surface")]
    if options.restricted_controls_enabled && options.control_socket.is_some() {
        manifest.hid.status = FeatureStatus::read_write(EvidenceLevel::SourceVerified);
        manifest.power.status = FeatureStatus::read_write(EvidenceLevel::SourceVerified);
        for feature in [
            "device_ssh",
            "device_hostname",
            "device_mdns",
            "device_hid_only",
            "device_mouse_jiggler",
            "device_oled",
            "network_wifi",
            "system_firewall",
        ] {
            manifest.board.features.insert(
                feature.into(),
                FeatureStatus::read_write(EvidenceLevel::SourceVerified),
            );
        }
        // HTTPS is always on: the page shows the redirect and the
        // certificate but offers no switch.
        manifest.board.features.insert(
            "network_tls".into(),
            FeatureStatus::read_only(EvidenceLevel::SourceVerified),
        );
        if options.media_controls_enabled {
            manifest.virtual_media.status =
                FeatureStatus::read_write(EvidenceLevel::SourceVerified);
            // The reference Download menu carries the local upload and the
            // remote download; the latter stays switched off until enabled.
            if options.media_directory.is_some() && options.media_catalog.is_some() {
                manifest.virtual_media.remote_download =
                    FeatureStatus::read_write(EvidenceLevel::SourceVerified);
            }
        }
        if options.system_actions_enabled {
            manifest.system.actions = FeatureStatus::read_write(EvidenceLevel::SourceVerified);
            // The reference's KVM reboot in Device settings.
            manifest.board.features.insert(
                "device_reboot".into(),
                FeatureStatus::read_write(EvidenceLevel::SourceVerified),
            );
        }
        // Administration features exist when the root helper is configured;
        // each stays switched off until the operator enables it.
        if options.admin_socket.is_some() {
            for feature in ["terminal", "scripts"] {
                manifest.board.features.insert(
                    feature.into(),
                    FeatureStatus::read_write(EvidenceLevel::SourceVerified),
                );
            }
            // RAW system updates go through the same helper behind their own
            // switch (`docs/design/system-raw-update-v1.md`).
            manifest.system.signed_system_updates =
                FeatureStatus::read_write(EvidenceLevel::SourceVerified);
        }
        if picoclaw_available(&options) {
            manifest.board.features.insert(
                "picoclaw".into(),
                FeatureStatus::read_write(EvidenceLevel::SourceVerified),
            );
        }
        // Tailscale as in the reference, through the administration helper.
        manifest.board.features.insert(
            "tailscale".into(),
            if options.admin_socket.is_some() {
                FeatureStatus::read_write(EvidenceLevel::SourceVerified)
            } else {
                FeatureStatus::unavailable()
            },
        );
        if options.video_controls_enabled {
            manifest.video.status = FeatureStatus::read_write(EvidenceLevel::SourceVerified);
            // Only the entries the owner can enforce are offered. The engine's
            // hardware encoder ignores the JPEG quality and the capture device
            // delivers MJPEG, so the quality switch cannot change the picture
            // on this board and stays hidden with the frame rate alone offered.
            manifest.video.settings = vec![
                bli_kvm_core::VideoSetting::Fps,
                bli_kvm_core::VideoSetting::Resolution,
                bli_kvm_core::VideoSetting::FrameDetect,
            ];
            // The WebRTC encoders take their bitrate over the demand socket.
            if options.offers_webrtc_quality() {
                manifest
                    .video
                    .settings
                    .push(bli_kvm_core::VideoSetting::WebrtcBitrate);
            }
            // The H.264 encoder takes the board's choice of its engine the
            // same way, where it reports.
            if options.offers_engine_choice() {
                manifest
                    .video
                    .settings
                    .push(bli_kvm_core::VideoSetting::H264Engine);
            }
        }
        if options.network_writes_enabled {
            manifest.network.status = FeatureStatus::read_write(EvidenceLevel::SourceVerified);
            manifest.network.dns = FeatureStatus::read_write(EvidenceLevel::SourceVerified);
            manifest.network.ipv6 = FeatureStatus::read_write(EvidenceLevel::SourceVerified);
            manifest.network.wol = FeatureStatus::read_write(EvidenceLevel::SourceVerified);
        }
        if options.firewall_modes_enabled {
            manifest.board.features.insert(
                "system_firewall_modes".into(),
                FeatureStatus::read_write(EvidenceLevel::SourceVerified),
            );
        }
        // The QUIC transport exists when the gateway's link is configured;
        // it stays switched off until the operator enables it.
        if options.quic_link_socket.is_some() {
            manifest.board.features.insert(
                "network_quic".into(),
                FeatureStatus::read_write(EvidenceLevel::SourceVerified),
            );
        }
    }
    manifest
        .validate()
        .expect("built-in Bli H616 capability manifest must be valid");
    let state = AppState {
        capabilities: Arc::new(manifest),
        update_http_upload: Arc::new(tokio::sync::Semaphore::new(1)),
        update_native_admission: options.update_native_admission,
        webrtc: options.webrtc_janus.and_then(|janus| {
            webrtc::WebRtcGateway::new(
                janus,
                options.webrtc_demand_socket.clone(),
                options.webrtc_h264_demand_socket.clone(),
                webrtc::load_bitrate(&options.state_dir),
                webrtc::load_engine(&options.state_dir),
            )
        }),
        update_activation: options
            .update_activation_socket
            .as_ref()
            .filter(|_| options.update_writes_enabled)
            .map(|path| {
                Arc::new(
                    UpdateActivationClient::new(path)
                        .expect("activation socket path must be a bounded absolute Unix path"),
                )
            }),
        update_upload: options
            .update_upload_socket
            .as_ref()
            .filter(|_| options.update_writes_enabled)
            .map(|path| {
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
        sessions: Arc::new(SessionStore::with_lock_after_seconds(session_lock::load(
            &options.state_dir,
        ))),
        login_limiter: Arc::new(Mutex::new(LoginRateLimiter::new(
            options.login_max_failures,
            options.login_lockout_secs,
        ))),
        setup_lock: Arc::new(Mutex::new(())),
        control: options.control_socket.as_ref().map(|path| {
            Arc::new(
                bli_kvm_control_client::ControlClient::new(path)
                    .expect("control socket path must be a bounded absolute Unix path"),
            )
        }),
        focus: Arc::new(Mutex::new(hid::FocusChallengeStore::new())),
        hid_ws_leases: Arc::new(Mutex::new(hid::WsLeaseStore::new())),
        terminal_tickets: Arc::new(Mutex::new(terminal::TicketStore::new())),
        picoclaw: picoclaw_available(&options).then(|| {
            Arc::new(picoclaw::Picoclaw::new(
                &options.state_dir,
                options
                    .picoclaw_mcp_url
                    .clone()
                    .unwrap_or_else(|| "http://127.0.0.1:18080/internal/picoclaw/mcp".into()),
            ))
        }),
        firewall_status: Arc::new(firewall_modes::StatusCache::default()),
        quality: Arc::new(quality::QualityCache::default()),
        quic: options
            .quic_link_socket
            .as_ref()
            .map(|_| Arc::new(quic::QuicTransport::default())),
        options: Arc::new(options.clone()),
    };
    // The H.264 encoder runs at the share of the chosen bitrate that the
    // QUIC sessions' paths carry.
    if let (Some(webrtc), Some(quic)) = (state.webrtc.as_ref(), state.quic.as_ref()) {
        webrtc.follow_share(quic.share());
    }
    quic::listen(&state);

    let public = Router::new()
        .route("/health", get(health))
        .route("/capabilities", get(capability_manifest))
        .route(
            "/application/current-version",
            get(current_application_version),
        )
        .route(
            "/auth/setup",
            get(auth::routes::setup_state)
                .post(auth::routes::setup_first_account)
                .layer(DefaultBodyLimit::max(AUTH_BODY_LIMIT)),
        )
        .route(
            "/auth/login",
            post(auth::routes::login).layer(DefaultBodyLimit::max(AUTH_BODY_LIMIT)),
        );
    let protected_routes = Router::new()
        .route("/auth/account", get(auth::routes::account))
        .route("/auth/logout", post(auth::routes::logout))
        .route(
            "/auth/password",
            get(auth::routes::password_state).post(auth::routes::change_password),
        )
        .route("/application/version", get(application::version))
        .route("/application/preview", get(application::preview))
        .route("/inventory", get(runtime_inventory))
        .route("/hostname", get(hostname))
        .route("/vm/mdns", get(device_settings::mdns_state))
        .route("/vm/mdns/enable", post(device_settings::mdns_enable))
        .route("/vm/mdns/disable", post(device_settings::mdns_disable))
        .route(
            "/vm/hostname",
            get(hostname::get_hostname).post(hostname::set_hostname),
        )
        .route("/vm/ssh", get(ssh::state))
        .route("/vm/ssh/enable", post(ssh::enable))
        .route("/vm/ssh/disable", post(ssh::disable))
        .route("/transport", get(quic::state))
        .route("/transport/quic/enable", post(quic::enable))
        .route("/transport/quic/disable", post(quic::disable))
        .route("/transport/quic/session", post(quic::session))
        .route(
            "/vm/tls",
            get(device_settings::tls_state).post(device_settings::tls_update),
        )
        .route(
            "/vm/hid-only",
            get(device_settings::hid_only_state).post(device_settings::hid_only_update),
        )
        .route(
            "/vm/oled",
            get(device_settings::display_sleep_state).post(device_settings::display_sleep_update),
        )
        .route(
            "/vm/mouse-jiggler",
            get(device_settings::mouse_jiggler_state).post(device_settings::mouse_jiggler_update),
        )
        .route("/system/time", get(system_time::get_config))
        .route(
            "/network/dns",
            get(network_ipv4::get_dns).post(network_ipv4::set_dns),
        )
        .route("/network/dns/confirm", post(network_ipv4::confirm))
        .route(
            "/network/ipv6",
            get(network_ipv4::ipv6::get).post(network_ipv4::ipv6::set),
        )
        .route("/network/wol", post(network_write::wol))
        .route(
            "/network/wol/mac",
            get(wol_macs::list).delete(wol_macs::delete),
        )
        .route("/network/wol/mac/name", post(wol_macs::rename))
        .route("/extensions/tailscale/status", get(tailscale::status))
        .route("/extensions/tailscale/install", post(tailscale::install))
        .route(
            "/extensions/tailscale/uninstall",
            post(tailscale::uninstall),
        )
        .route("/extensions/tailscale/start", post(tailscale::start))
        .route("/extensions/tailscale/restart", post(tailscale::restart))
        .route("/extensions/tailscale/stop", post(tailscale::stop))
        .route("/extensions/tailscale/up", post(tailscale::up))
        .route("/extensions/tailscale/down", post(tailscale::down))
        .route("/extensions/tailscale/login", post(tailscale::login))
        .route("/extensions/tailscale/logout", post(tailscale::logout))
        .route(
            "/system/firewall/mode",
            get(firewall_modes::status).post(firewall_modes::change),
        )
        .route(
            "/system/firewall/mode/confirm",
            post(firewall_modes::confirm),
        )
        .route(
            "/system/firewall",
            get(device_settings::firewall_state).post(device_settings::firewall_set),
        )
        .route(
            "/system/firewall/confirm",
            post(device_settings::firewall_confirm),
        )
        .route(
            "/network/wifi",
            get(wifi::network_wifi).post(wifi::network_wifi_ap),
        )
        .route(
            "/network/wifi/scan",
            get(wifi::network_wifi_scan).post(wifi::network_wifi_rescan),
        )
        .route(
            "/network/wifi/saved",
            get(wifi::network_wifi_saved).post(wifi::network_wifi_profile),
        )
        .route("/network/wifi/connect", post(wifi::network_wifi_connect))
        .route(
            "/network/wifi/disconnect",
            post(wifi::network_wifi_disconnect),
        )
        .route("/network/wifi/radio", post(wifi::network_wifi_radio))
        .route(
            "/network/wifi/preferred",
            post(wifi::network_wifi_preferred),
        )
        .route(
            "/network/wifi/preferred/confirm",
            post(wifi::network_wifi_preferred_confirm),
        )
        .route("/network/interfaces", get(network::network_interfaces))
        .route("/vm/info", get(vm_info::vm_info))
        .route("/vm/autostart", get(autostart::list))
        .route("/vm/autostart/{name}", get(autostart::content))
        .route("/download/image/enabled", get(download::image_enabled))
        .route("/download/file/check", post(download::check_upload))
        .route("/download/image", post(remote_download::start))
        .route("/download/image/status", get(remote_download::status))
        .route(
            "/download/image/remote/enabled",
            get(admin_features::remote_download_enabled)
                .post(admin_features::set_remote_download_enabled),
        )
        .route(
            "/vm/terminal/enabled",
            get(admin_features::terminal_enabled).post(admin_features::set_terminal_enabled),
        )
        .route(
            "/admin/features",
            get(admin_features::list).post(admin_features::update),
        )
        .route("/vm/hardware", get(vm_info::vm_hardware))
        .route("/system-log/config", get(system_log::get_config))
        .route("/system-log/messages", get(system_log::get_messages))
        .route("/system-log/test", post(system_log::send_test))
        .route("/diagnostics", get(diagnostics_view))
        .route("/application-update/status", get(update::status))
        .route("/application-update/upload", post(update::upload))
        .route("/application-update/activate", post(update::activate))
        .route(
            "/system-update/raw-enabled",
            get(system_update::raw_enabled).post(system_update::set_raw_enabled),
        )
        .route("/system-update/status", get(system_update::status))
        .route("/system-update/upload", post(system_update::upload))
        .route("/system-update/install", post(system_update::install))
        .route("/system-update/confirm", post(system_update::confirm))
        .route("/system-update/rollback", post(system_update::rollback))
        .route("/system-update/cancel", post(system_update::cancel))
        .route(
            "/hid/focus",
            get(hid::issue_focus_challenge).post(hid::confirm_focus),
        )
        .route("/hid/lease", post(hid::hid_lease))
        .route(
            "/hid/screen-area",
            get(screen_area::get_screen_area).post(screen_area::set_screen_area),
        )
        .route(
            "/target-agent",
            get(target_agent::state).post(target_agent::set),
        )
        .route("/target-agent/call", post(target_agent::call))
        .route("/hid/shortcuts", get(shortcuts::list))
        .route(
            "/hid/shortcut",
            post(shortcuts::add).delete(shortcuts::delete),
        )
        .route(
            "/hid/shortcut/leader-key",
            get(leader_key::get_leader_key).post(leader_key::set_leader_key),
        )
        .route("/ws", get(ws::hid_socket))
        .route("/virtual-media", post(media::virtual_media))
        .route("/virtual-media/catalog", get(media::media_catalog))
        .route("/storage/image", get(media::storage_images))
        .route("/storage/image/mounted", get(media::storage_mounted))
        .route("/storage/image/mount", post(media::storage_mount))
        .route("/storage/image/delete", post(media::storage_delete))
        .route("/storage/cdrom", get(media::storage_cdrom))
        .route("/storage/usb/reconnect", post(media::storage_reconnect))
        .route(
            "/vm/session-lock",
            get(session_lock::get_session_lock).post(session_lock::set_session_lock),
        )
        .route("/application-power", post(atx::application_power))
        .route("/vm/system/reboot", post(system_action::reboot))
        .route("/vm/system/shutdown", post(system_action::shutdown))
        .route("/stream/mjpeg", get(video::mjpeg))
        .route(
            "/stream/mjpeg/detect",
            post(video_settings::update_frame_detect),
        )
        .route(
            "/stream/mjpeg/detect/stop",
            post(video_settings::stop_frame_detect),
        )
        .route("/stream/webrtc", get(webrtc::webrtc_socket))
        .route("/vm/terminal", get(terminal::connect))
        .route(
            "/vm/terminal/unlock",
            post(terminal::unlock).layer(DefaultBodyLimit::max(AUTH_BODY_LIMIT)),
        )
        .route("/vm/script", get(scripts::list).delete(scripts::delete))
        .route("/vm/script/upload", post(scripts::upload))
        .route("/vm/script/run", post(scripts::run))
        .route(
            "/vm/screen",
            get(video_settings::screen_state).post(video_settings::update_screen),
        )
        .route("/vm/quality", get(quality::quality))
        .route("/picoclaw/model/config", post(picoclaw::model_config))
        .route("/picoclaw/agent/profile", post(picoclaw::agent_profile))
        .route("/picoclaw/sessions", get(picoclaw::list_sessions))
        .route(
            "/picoclaw/sessions/{id}",
            get(picoclaw::get_session).delete(picoclaw::delete_session),
        )
        .route(
            "/picoclaw/runtime/status",
            get(picoclaw::get_runtime_status),
        )
        .route(
            "/picoclaw/runtime/session",
            delete(picoclaw::release_session),
        )
        .route("/picoclaw/runtime/install", post(picoclaw::install))
        .route("/picoclaw/runtime/uninstall", post(picoclaw::uninstall))
        .route("/picoclaw/runtime/start", post(picoclaw::start))
        .route("/picoclaw/runtime/stop", post(picoclaw::stop))
        .route("/picoclaw/gateway/ws", get(picoclaw::connect_gateway))
        .route_layer(from_fn_with_state(state.clone(), protected));
    // The media upload takes a large body, so it keeps its own bound, but it
    // must still pass the session, CSRF and origin checks before the body is
    // read; route_layer is the outer wrapper here.
    let upload = Router::new()
        .route("/download/file", post(download::upload_image))
        .layer(DefaultBodyLimit::max(
            usize::try_from(download::MAX_MEDIA_UPLOAD_BYTES + 1024 * 1024)
                .expect("upload cap fits usize"),
        ))
        .route_layer(from_fn_with_state(state.clone(), protected));
    let protected_routes = protected_routes.merge(upload);
    let api = public.merge(protected_routes).fallback(api_not_found);
    let router = Router::new()
        .nest("/api", api)
        .nest(
            "/internal/picoclaw",
            picoclaw::internal_router(state.clone()),
        )
        .with_state(state.clone());
    let router = if let Some(root) = options.web_root {
        let index = root.join("index.html");
        router.fallback_service(ServeDir::new(root).fallback(ServeFile::new(index)))
    } else {
        router
    };

    router
        .layer(from_fn_with_state(
            state.options.shutdown.clone(),
            shutdown::admit,
        ))
        .layer(from_fn(security_headers))
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

/// Resolve the runtime inventory the same way for every view: the vendor
/// broker when it answers, otherwise (board profile) the facts our own
/// arrangement observes instead of failing the whole view.
async fn resolve_inventory(state: &AppState, session_token: &str) -> AppResult<RuntimeInventory> {
    match state.inventory.inventory().await {
        Ok(inventory) => Ok(inventory),
        Err(_) if state.options.restricted_controls_enabled && state.control.is_some() => {
            Ok(native_inventory(state, session_token).await)
        }
        Err(_) => Err(AppError::ServiceUnavailable(
            "platform broker unavailable".into(),
        )),
    }
}

async fn runtime_inventory(
    State(state): State<AppState>,
    Extension(auth::middleware::CurrentSession(session)): Extension<
        auth::middleware::CurrentSession,
    >,
) -> AppResult<Json<ApiResponse<RuntimeInventory>>> {
    let inventory = resolve_inventory(&state, &session.token).await?;
    Ok(Json(ApiResponse::ok(inventory)))
}

/// The USB device controller: bound (online) and enumerated by the target
/// host (`configured`).
fn usb_device_controller() -> (Option<bool>, Option<bool>) {
    let Ok(mut entries) = std::fs::read_dir("/sys/class/udc") else {
        return (None, None);
    };
    let Some(Ok(entry)) = entries.next() else {
        return (Some(false), Some(false));
    };
    match std::fs::read_to_string(entry.path().join("state")) {
        Ok(state) => (Some(true), Some(state.trim() == "configured")),
        Err(_) => (Some(true), None),
    }
}

/// Assemble the native board inventory: thermal and RTC from the kernel views,
/// the host power LED and the component states from the board owner and the
/// USB controller, and nothing that we do not observe.
async fn native_inventory(state: &AppState, session_token: &str) -> RuntimeInventory {
    let thermal = bli_kvm_platform_bli::read_h616_temperature_millidegrees().ok();
    let rtc = bli_kvm_platform_bli::read_h616_external_rtc().ok();
    let power_led = match state.control.as_ref() {
        Some(control) => {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_millis() as u64)
                .unwrap_or(0);
            let operation_id = format!("{now_ms:032x}");
            match control
                .execute_v2(
                    operation_id,
                    bli_kvm_core::PlatformControlRequestV2::BoardStatus {},
                )
                .await
            {
                Ok(bli_kvm_control_protocol::ControlResponse::BoardStatus {
                    power_led_present,
                    power_led_on,
                    ..
                }) => (Some(true), power_led_present.then_some(power_led_on)),
                _ => (Some(false), None),
            }
        }
        None => (None, None),
    };
    let (power_online, power_led) = power_led;
    let (hid_online, hid_connected) = usb_device_controller();
    #[cfg(feature = "restricted-control-surface")]
    let (media_enabled, media_online, media_connected) = if state.options.media_controls_enabled {
        match media::observe(state, session_token).await {
            Some(mounted) => (Some(true), Some(true), Some(mounted)),
            None => (Some(true), Some(false), None),
        }
    } else {
        (Some(false), None, None)
    };
    #[cfg(not(feature = "restricted-control-surface"))]
    let (media_enabled, media_online, media_connected) = {
        let _ = session_token;
        (None, None, None)
    };
    // The capture engine's own state tells whether the source has a signal.
    #[cfg(feature = "restricted-control-surface")]
    let video_online = match state.options.engine_socket.clone() {
        Some(socket) => tokio::task::spawn_blocking(move || video_engine::read_state(&socket))
            .await
            .ok()
            .flatten()
            .and_then(|engine| engine.online),
        None => None,
    };
    #[cfg(not(feature = "restricted-control-surface"))]
    let video_online = None;
    bli_kvm_platform_bli::native_board_inventory(bli_kvm_platform_bli::NativeBoardObservations {
        thermal_millidegrees_celsius: thermal,
        external_rtc_seconds: rtc.map(|readout| readout.unix_seconds),
        external_rtc_offset_seconds: rtc.map(|readout| readout.offset_from_system_seconds),
        power_led,
        video_online,
        hid_online,
        hid_connected,
        power_online,
        media_enabled,
        media_online,
        media_connected,
    })
}

async fn diagnostics_view(
    State(state): State<AppState>,
    Extension(auth::middleware::CurrentSession(session)): Extension<
        auth::middleware::CurrentSession,
    >,
) -> AppResult<Json<ApiResponse<DiagnosticsView>>> {
    let inventory = resolve_inventory(&state, &session.token).await?;
    let diagnostics = DiagnosticsView::from_inventory(&inventory)
        .map_err(|_| AppError::ServiceUnavailable("diagnostics view unavailable".into()))?;
    Ok(Json(ApiResponse::ok(diagnostics)))
}

async fn current_application_version() -> Json<ApiResponse<CurrentApplicationVersion>> {
    Json(ApiResponse::ok(CurrentApplicationVersion {
        current: env!("CARGO_PKG_VERSION"),
    }))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Hostname {
    hostname: String,
}

async fn hostname() -> AppResult<Json<ApiResponse<Hostname>>> {
    Ok(Json(ApiResponse::ok(Hostname {
        hostname: system_hostname()?,
    })))
}

pub(crate) fn system_hostname() -> AppResult<String> {
    let raw = std::fs::read_to_string("/etc/hostname")
        .map_err(|_| AppError::ServiceUnavailable("hostname is unavailable".into()))?;
    validate_hostname(raw.trim())
}

fn validate_hostname(value: &str) -> AppResult<String> {
    if value.is_empty()
        || value.len() > 253
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-')
    {
        return Err(AppError::Internal("hostname is invalid".into()));
    }
    Ok(value.to_string())
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
    mod http_transport;
    mod quic_transport;
    mod target_agent;
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
    async fn configured_write_sockets_without_opt_in_remain_inaccessible() {
        struct NeverBody;
        impl http_body::Body for NeverBody {
            type Data = bytes::Bytes;
            type Error = std::io::Error;
            fn poll_frame(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>>
            {
                panic!("disabled write request must not poll its body");
            }
        }
        let root = tempdir().unwrap();
        let upload = root.path().join("upload.sock");
        let activation = root.path().join("activation.sock");
        let upload_listener = UnixListener::bind(&upload).unwrap();
        let activation_listener = UnixListener::bind(&activation).unwrap();
        let app = app_with_options(ServerOptions {
            state_dir: root.path().join("account"),
            update_status_socket: Some(root.path().join("status.sock")),
            update_upload_socket: Some(upload),
            update_activation_socket: Some(activation),
            ..ServerOptions::default()
        });
        let (cookie, csrf) = setup_and_login(&app).await;
        for route in ["upload", "activate"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(format!("/api/application-update/{route}"))
                        .header(header::HOST, "kvm.test")
                        .header(header::ORIGIN, "https://kvm.test")
                        .header(header::COOKIE, &cookie)
                        .header("x-csrf-token", &csrf)
                        .body(Body::new(NeverBody))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        }
        for listener in [upload_listener, activation_listener] {
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(20), listener.accept())
                    .await
                    .is_err()
            );
        }
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
            manifest["data"]["system"]["signedApplicationUpdates"]["evidence"],
            "source_verified"
        );
    }

    #[tokio::test]
    async fn write_opt_in_rejects_missing_or_shared_paths() {
        for mask in 0..7 {
            let root = tempdir().unwrap();
            let options = ServerOptions {
                state_dir: root.path().join("account"),
                update_status_socket: (mask & 1 != 0).then(|| root.path().join("status.sock")),
                update_upload_socket: (mask & 2 != 0).then(|| root.path().join("upload.sock")),
                update_activation_socket: (mask & 4 != 0)
                    .then(|| root.path().join("activation.sock")),
                update_writes_enabled: true,
                ..ServerOptions::default()
            };
            assert!(std::panic::catch_unwind(|| app_with_options(options)).is_err());
        }
        for pair in [(0, 1), (0, 2), (1, 2)] {
            let root = tempdir().unwrap();
            let mut paths = [
                root.path().join("status.sock"),
                root.path().join("upload.sock"),
                root.path().join("activation.sock"),
            ];
            paths[pair.1] = paths[pair.0].clone();
            let options = ServerOptions {
                state_dir: root.path().join("account"),
                update_status_socket: Some(paths[0].clone()),
                update_upload_socket: Some(paths[1].clone()),
                update_activation_socket: Some(paths[2].clone()),
                update_writes_enabled: true,
                ..ServerOptions::default()
            };
            assert!(std::panic::catch_unwind(|| app_with_options(options)).is_err());
        }
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
            update_writes_enabled: true,
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
            "read_write"
        );
    }

    async fn next_text(
        browser: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    ) -> Value {
        use futures_util::StreamExt;
        let frame = tokio::time::timeout(std::time::Duration::from_secs(5), browser.next())
            .await
            .unwrap();
        let Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) = frame else {
            panic!("expected a text frame, got {frame:?}");
        };
        serde_json::from_str(&text).unwrap()
    }

    #[tokio::test]
    #[allow(clippy::result_large_err)] // The tungstenite handshake callback type is fixed.
    async fn webrtc_signaling_relays_only_the_closed_protocol_and_destroys_the_session() {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::{
            Message as WsMessage,
            client::IntoClientRequest,
            handshake::server::{Request as HsRequest, Response as HsResponse},
            http::HeaderValue,
        };

        let janus_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let janus_address = janus_listener.local_addr().unwrap();
        let janus = tokio::spawn(async move {
            let (stream, _) = janus_listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_hdr_async(
                stream,
                |request: &HsRequest, mut response: HsResponse| {
                    assert_eq!(
                        request.headers().get("sec-websocket-protocol").unwrap(),
                        "janus-protocol"
                    );
                    response.headers_mut().insert(
                        "sec-websocket-protocol",
                        HeaderValue::from_static("janus-protocol"),
                    );
                    Ok(response)
                },
            )
            .await
            .unwrap();
            let mut seen = Vec::new();
            loop {
                let Some(Ok(WsMessage::Text(text))) = socket.next().await else {
                    break;
                };
                let request: Value = serde_json::from_str(&text).unwrap();
                let transaction = request["transaction"].clone();
                let kind = request["janus"].as_str().unwrap().to_string();
                seen.push(request.clone());
                let reply = match kind.as_str() {
                    "create" => Some(
                        serde_json::json!({"janus": "success", "transaction": transaction, "data": {"id": 11}}),
                    ),
                    "attach" => {
                        assert_eq!(request["plugin"], "janus.plugin.streaming");
                        assert_eq!(request["session_id"], 11);
                        Some(
                            serde_json::json!({"janus": "success", "transaction": transaction, "data": {"id": 22}}),
                        )
                    }
                    "message" if request["body"]["request"] == "watch" => {
                        assert_eq!(request["body"]["id"], 1);
                        assert_eq!(request["handle_id"], 22);
                        socket
                            .send(WsMessage::Text(
                                serde_json::json!({"janus": "ack", "transaction": transaction})
                                    .to_string()
                                    .into(),
                            ))
                            .await
                            .unwrap();
                        Some(
                            serde_json::json!({"janus": "event", "transaction": transaction, "jsep": {"type": "offer", "sdp": "v=0 offer"}}),
                        )
                    }
                    "message" => {
                        assert_eq!(request["body"]["request"], "start");
                        assert_eq!(request["jsep"]["sdp"], "v=0 answer");
                        None
                    }
                    "trickle" if request["candidate"]["completed"] == true => {
                        Some(serde_json::json!({"janus": "webrtcup", "session_id": 11}))
                    }
                    "destroy" => break,
                    _ => None,
                };
                if let Some(reply) = reply {
                    socket
                        .send(WsMessage::Text(reply.to_string().into()))
                        .await
                        .unwrap();
                }
            }
            seen
        });

        let root = tempdir().unwrap();
        // The encoder's demand socket: held for exactly the viewer's session.
        let demand_path = root.path().join("demand.sock");
        let demand_listener = tokio::net::UnixListener::bind(&demand_path).unwrap();
        let demand = tokio::spawn(async move {
            let (mut connection, _) = demand_listener.accept().await.unwrap();
            let mut received = Vec::new();
            connection.read_to_end(&mut received).await.unwrap();
            received
        });
        let app = app_with_options(ServerOptions {
            state_dir: root.path().join("account"),
            webrtc_janus: Some(janus_address),
            webrtc_demand_socket: Some(demand_path),
            ..ServerOptions::default()
        });
        let (cookie, _) = setup_and_login(&app).await;
        let manifest = json_body(
            app.clone()
                .oneshot(json_request(Method::GET, "/api/capabilities", Value::Null))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            manifest["data"]["video"]["transports"],
            serde_json::json!(["mjpeg", "web_rtc_janus"])
        );
        assert_eq!(manifest["data"]["video"]["preferredTransport"], "mjpeg");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });
        let mut request = format!("ws://{address}/api/stream/webrtc")
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert("cookie", cookie.parse().unwrap());
        request
            .headers_mut()
            .insert("origin", HeaderValue::from_static("http://kvm.test"));
        request
            .headers_mut()
            .insert("host", HeaderValue::from_static("kvm.test"));
        let stream = tokio::net::TcpStream::connect(address).await.unwrap();
        let (mut browser, _) = tokio_tungstenite::client_async(request, stream)
            .await
            .unwrap();
        let offer = next_text(&mut browser).await;
        assert_eq!(
            offer,
            serde_json::json!({"type": "offer", "sdp": "v=0 offer"})
        );
        for message in [
            serde_json::json!({"type": "answer", "sdp": "v=0 answer"}),
            serde_json::json!({"type": "candidate", "candidate": {"candidate": "candidate:1 1 udp 1 192.0.2.1 5000 typ host", "sdpMid": "0", "sdpMLineIndex": 0}}),
            serde_json::json!({"type": "candidate", "candidate": null}),
        ] {
            browser
                .send(WsMessage::Text(message.to_string().into()))
                .await
                .unwrap();
        }
        assert_eq!(next_text(&mut browser).await["type"], "started");
        // Anything outside the closed protocol ends the session with an error.
        browser
            .send(WsMessage::Text(
                serde_json::json!({"type": "janus", "janus": "detach"})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        assert_eq!(
            next_text(&mut browser).await,
            serde_json::json!({"type": "error", "code": "invalid_message"})
        );
        let seen = tokio::time::timeout(std::time::Duration::from_secs(5), janus)
            .await
            .unwrap()
            .unwrap();
        let kinds: Vec<_> = seen
            .iter()
            .map(|request| request["janus"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            kinds,
            [
                "create", "attach", "message", "message", "trickle", "trickle", "destroy"
            ]
        );
        assert!(
            seen.iter()
                .all(|request| request["janus"] == "create" || request["session_id"] == 11)
        );
        // The viewer left, so the demand connection closed after naming VP8
        // and the bitrate (the default, nothing was chosen).
        let asked = tokio::time::timeout(std::time::Duration::from_secs(5), demand)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(asked, b"vp8\nkbps 2500\n");
        server.abort();
    }

    #[tokio::test]
    async fn h264_over_webrtc_is_offered_only_with_its_encoder() {
        use tokio_tungstenite::tungstenite::{client::IntoClientRequest, http::HeaderValue};

        let root = tempdir().unwrap();
        let janus = SocketAddr::from(([127, 0, 0, 1], 9));
        let with_h264 = app_with_options(ServerOptions {
            state_dir: root.path().join("with"),
            webrtc_janus: Some(janus),
            webrtc_h264_demand_socket: Some(root.path().join("h264.sock")),
            ..ServerOptions::default()
        });
        let without = app_with_options(ServerOptions {
            state_dir: root.path().join("without"),
            webrtc_janus: Some(janus),
            ..ServerOptions::default()
        });
        for (app, transports) in [
            (
                &with_h264,
                serde_json::json!(["mjpeg", "web_rtc_janus", "web_rtc_janus_h264"]),
            ),
            (&without, serde_json::json!(["mjpeg", "web_rtc_janus"])),
        ] {
            let manifest = json_body(
                app.clone()
                    .oneshot(json_request(Method::GET, "/api/capabilities", Value::Null))
                    .await
                    .unwrap(),
            )
            .await;
            assert_eq!(manifest["data"]["video"]["transports"], transports);
        }

        // Asked of a server without it, H.264 is refused before the upgrade.
        let (cookie, _) = setup_and_login(&without).await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                without.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });
        for (query, status) in [("h264", 503), ("av1", 400)] {
            let mut request = format!("ws://{address}/api/stream/webrtc?codec={query}")
                .into_client_request()
                .unwrap();
            let headers = request.headers_mut();
            headers.insert("cookie", cookie.parse().unwrap());
            headers.insert("origin", HeaderValue::from_static("http://kvm.test"));
            headers.insert("host", HeaderValue::from_static("kvm.test"));
            let stream = tokio::net::TcpStream::connect(address).await.unwrap();
            match tokio_tungstenite::client_async(request, stream).await {
                Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
                    assert_eq!(response.status().as_u16(), status, "{query}");
                }
                other => panic!("{query}: {:?}", other.map(|_| ())),
            }
        }
        server.abort();
    }

    #[tokio::test]
    async fn native_profile_activation_sends_admission_and_reports_prepared() {
        use bli_kvm_update_protocol::{self as protocol, activation as ipc};
        let root = tempdir().unwrap();
        let socket = root.path().join("activate.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let app = app_with_options(ServerOptions {
            state_dir: root.path().join("account"),
            update_status_socket: Some(root.path().join("status.sock")),
            update_upload_socket: Some(root.path().join("upload.sock")),
            update_activation_socket: Some(socket),
            update_writes_enabled: true,
            update_native_admission: true,
            ..ServerOptions::default()
        });
        let (cookie, csrf) = setup_and_login(&app).await;
        let slot = format!("app-00000000000000000002-{}", "a".repeat(64));
        let worker = {
            let slot = slot.clone();
            tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                stream.read_to_end(&mut bytes).await.unwrap();
                let request = ipc::decode_request(&bytes[4..]).unwrap();
                assert_eq!(
                    request.request,
                    ipc::ActivationRequest::Admit {
                        slot_id: slot.clone(),
                        expected_generation: 7
                    }
                );
                let response = ipc::ActivationResponseEnvelope {
                    schema_version: 1,
                    request_id: request.request_id,
                    response: ipc::ActivationResponse::Prepared {
                        receipt: ipc::PreparedActivationReceipt {
                            operation_id: "2".repeat(32),
                            target_slot_id: slot,
                            prepared_generation: 8,
                            deadline_monotonic_ms: 90_000,
                        },
                    },
                };
                stream
                    .write_all(
                        &protocol::encode_frame(&ipc::encode_response(&response).unwrap()).unwrap(),
                    )
                    .await
                    .unwrap();
            })
        };
        let mut request = json_request(
            Method::POST,
            "/api/application-update/activate",
            serde_json::json!({"slotId": slot, "expectedGeneration": "7"}),
        );
        request
            .headers_mut()
            .insert(header::COOKIE, cookie.parse().unwrap());
        request
            .headers_mut()
            .insert("x-csrf-token", csrf.parse().unwrap());
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let result = json_body(response).await;
        assert_eq!(result["data"]["phase"], "prepared");
        assert_eq!(result["data"]["receipt"]["operationId"], "2".repeat(32));
        assert_eq!(result["data"]["receipt"]["targetSlotId"], slot);
        worker.await.unwrap();
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
            update_activation_socket: Some(root.path().join("activation.sock")),
            update_writes_enabled: true,
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

    async fn session_lock_state(
        app: &Router,
        cookie: &str,
        csrf: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method(Method::GET)
            .uri("/api/vm/session-lock")
            .header(header::HOST, "kvm.test")
            .header(header::COOKIE, cookie);
        if let Some(csrf) = csrf {
            request = request.header("x-csrf-token", csrf);
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        (status, json_body(response).await)
    }

    async fn set_session_lock(
        app: &Router,
        cookie: &str,
        csrf: Option<&str>,
        duration: u64,
        lock_now: bool,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method(Method::POST)
            .uri("/api/vm/session-lock")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::HOST, "kvm.test")
            .header(header::ORIGIN, "https://kvm.test")
            .header(header::COOKIE, cookie);
        if let Some(csrf) = csrf {
            request = request.header("x-csrf-token", csrf);
        }
        let body = serde_json::json!({"duration": duration, "lockNow": lock_now});
        let response = app
            .clone()
            .oneshot(request.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = response.status();
        (status, json_body(response).await)
    }

    #[tokio::test]
    async fn administration_features_are_refused_while_switched_off() {
        // No switch file exists on the host, so every feature is off.
        let app = isolated_app();
        let (cookie, csrf) = setup_and_login(&app).await;
        let mut request = Request::builder()
            .method(Method::POST)
            .uri("/api/vm/terminal/unlock")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::HOST, "kvm.test")
            .header(header::ORIGIN, "https://kvm.test")
            .header(header::COOKIE, &cookie)
            .header("x-csrf-token", &csrf)
            .body(Body::from(
                serde_json::json!({"username": "admin", "password": "irrelevant"}).to_string(),
            ))
            .unwrap();
        request
            .extensions_mut()
            .insert(axum::extract::ConnectInfo(SocketAddr::from((
                [127, 0, 0, 1],
                1,
            ))));
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let request = Request::builder()
            .method(Method::GET)
            .uri("/api/vm/terminal?ticket=anything")
            .header(header::HOST, "kvm.test")
            .header(header::ORIGIN, "https://kvm.test")
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        // Never upgraded: the upgrade extractor or the switch refuses it.
        assert!(matches!(
            response.status(),
            StatusCode::BAD_REQUEST | StatusCode::FORBIDDEN
        ));

        let request = Request::builder()
            .method(Method::GET)
            .uri("/api/vm/script")
            .header(header::HOST, "kvm.test")
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let request = Request::builder()
            .method(Method::POST)
            .uri("/api/download/image")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::HOST, "kvm.test")
            .header(header::ORIGIN, "https://kvm.test")
            .header(header::COOKIE, &cookie)
            .header("x-csrf-token", &csrf)
            .body(Body::from(
                serde_json::json!({"file": "https://mirror.example/a.iso"}).to_string(),
            ))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let request = Request::builder()
            .method(Method::GET)
            .uri("/api/picoclaw/runtime/status")
            .header(header::HOST, "kvm.test")
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        // The runtime's internal route answers nothing without its token,
        // and nothing that came through the TLS proxy.
        for forwarded in [false, true] {
            let mut request = Request::builder()
                .method(Method::POST)
                .uri("/internal/picoclaw/mcp")
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-blikvm-internal-token", "0".repeat(64));
            if forwarded {
                request = request.header("x-forwarded-for", "192.0.2.1");
            }
            let mut request = request
                .body(Body::from(
                    r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
                ))
                .unwrap();
            request
                .extensions_mut()
                .insert(axum::extract::ConnectInfo(SocketAddr::from((
                    [127, 0, 0, 1],
                    1,
                ))));
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }

        let request = Request::builder()
            .method(Method::GET)
            .uri("/api/admin/features")
            .header(header::HOST, "kvm.test")
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        let body = json_body(response).await;
        for feature in ["terminal", "scripts", "remoteDownload", "picoclaw"] {
            assert_eq!(body["data"][feature], false, "{feature}");
        }
    }

    fn isolated_app() -> Router {
        let root = tempdir().unwrap();
        app_with_options(ServerOptions {
            state_dir: root.path().join("account"),
            ..ServerOptions::default()
        })
    }

    async fn login_again(app: &Router) -> String {
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
        login.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned()
    }

    #[tokio::test]
    async fn login_refuses_oversized_bodies_and_names_without_remembering_them() {
        let app = isolated_app();
        setup_and_login(&app).await;
        let big = json_request(
            Method::POST,
            "/api/auth/login",
            serde_json::json!({"username": "x".repeat(64 * 1024), "password": "p"}),
        );
        let response = app.clone().oneshot(big).await.unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        for length in [65, 200] {
            let response = app
                .clone()
                .oneshot(json_request(
                    Method::POST,
                    "/api/auth/login",
                    serde_json::json!({"username": "x".repeat(length), "password": "p"}),
                ))
                .await
                .unwrap();
            // -1 is a bad request; a wrong name would answer -2.
            assert_eq!(json_body(response).await["code"], -1, "{length}");
        }
        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/api/auth/login",
                serde_json::json!({"username": "operator", "password": "p".repeat(257)}),
            ))
            .await
            .unwrap();
        assert_eq!(json_body(response).await["code"], -1);
        // The real account still logs in.
        assert!(!login_again(&app).await.is_empty());
    }

    fn forwarded_login(from: &str, password: &str) -> Request<Body> {
        let mut request = json_request(
            Method::POST,
            "/api/auth/login",
            serde_json::json!({"username": "operator", "password": password}),
        );
        request
            .headers_mut()
            .insert("x-forwarded-for", from.parse().unwrap());
        request
            .extensions_mut()
            .insert(ConnectInfo("127.0.0.1:4242".parse::<SocketAddr>().unwrap()));
        request
    }

    #[tokio::test]
    async fn a_lockout_belongs_to_the_forwarded_client_not_to_nginx() {
        let root = tempdir().unwrap();
        let app = auth_app(root.path().join("account"), 2);
        setup_and_login(&app).await;
        for _ in 0..2 {
            let response = app
                .clone()
                .oneshot(forwarded_login("198.51.100.1", "wrong password"))
                .await
                .unwrap();
            assert_eq!(json_body(response).await["code"], -2);
        }
        // The first client is locked, even with the right password.
        let response = app
            .clone()
            .oneshot(forwarded_login("198.51.100.1", "initial password"))
            .await
            .unwrap();
        assert_ne!(json_body(response).await["code"], 0);
        // A second client behind the same nginx is not.
        let response = app
            .clone()
            .oneshot(forwarded_login("198.51.100.2", "initial password"))
            .await
            .unwrap();
        assert_eq!(json_body(response).await["code"], 0);
    }

    #[tokio::test]
    async fn with_the_proxy_socket_a_loopback_tcp_peer_counts_as_itself() {
        let root = tempdir().unwrap();
        let app = app_with_options(ServerOptions {
            state_dir: root.path().join("account"),
            secure_cookies: true,
            login_max_failures: 2,
            proxy_socket: Some(root.path().join("proxy.sock")),
            ..ServerOptions::default()
        });
        setup_and_login(&app).await;
        // A local process names a fresh client for every guess over TCP.
        for from in ["198.51.100.1", "198.51.100.2"] {
            let response = app
                .clone()
                .oneshot(forwarded_login(from, "wrong password"))
                .await
                .unwrap();
            assert_eq!(json_body(response).await["code"], -2);
        }
        // Both counted under the TCP peer, which is now locked.
        let response = app
            .clone()
            .oneshot(forwarded_login("198.51.100.3", "initial password"))
            .await
            .unwrap();
        assert_ne!(json_body(response).await["code"], 0);
        // Through nginx's socket the forwarded client counts, and is not.
        let mut request = forwarded_login("198.51.100.4", "initial password");
        request.extensions_mut().remove::<ConnectInfo<SocketAddr>>();
        request
            .extensions_mut()
            .insert(ConnectInfo(crate::proxy_socket::ProxyPeer));
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(json_body(response).await["code"], 0);
    }

    #[tokio::test]
    async fn a_session_lock_that_cannot_be_saved_keeps_the_old_value() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempdir().unwrap();
        let state = root.path().join("account");
        let app = auth_app(state.clone(), 5);
        let (cookie, csrf) = setup_and_login(&app).await;
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o500)).unwrap();
        let (status, _) = set_session_lock(&app, &cookie, Some(&csrf), 300, false).await;
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        let (_, body) = session_lock_state(&app, &cookie, None).await;
        assert_eq!(body["data"]["duration"], 3600);
    }

    #[cfg(feature = "restricted-control-surface")]
    #[tokio::test]
    async fn the_webrtc_quality_is_offered_checked_and_kept() {
        let root = tempdir().unwrap();
        let state_dir = root.path().join("account");
        let options = ServerOptions {
            state_dir: state_dir.clone(),
            restricted_controls_enabled: true,
            video_controls_enabled: true,
            control_socket: Some(root.path().join("control.sock")),
            webrtc_janus: Some("127.0.0.1:8188".parse().unwrap()),
            webrtc_h264_demand_socket: Some(root.path().join("h264.sock")),
            ..ServerOptions::default()
        };
        let app = app_with_options(options);
        let (cookie, csrf) = setup_and_login(&app).await;
        let response = app
            .clone()
            .oneshot(json_request(Method::GET, "/api/capabilities", Value::Null))
            .await
            .unwrap();
        let settings = json_body(response).await["data"]["video"]["settings"].clone();
        assert!(
            settings
                .as_array()
                .unwrap()
                .contains(&Value::from("webrtc_bitrate")),
            "{settings}"
        );
        let post = |value: i64| {
            let mut request = json_request(
                Method::POST,
                "/api/vm/screen",
                serde_json::json!({"type": "webrtc_kbps", "value": value}),
            );
            request
                .headers_mut()
                .insert(header::COOKIE, cookie.parse().unwrap());
            request
                .headers_mut()
                .insert("x-csrf-token", csrf.parse().unwrap());
            request
        };
        let body = json_body(app.clone().oneshot(post(5000)).await.unwrap()).await;
        assert_eq!(body["data"]["applied"], true, "{body}");
        assert_eq!(body["data"]["webrtcKbps"], 5000);
        let body = json_body(app.clone().oneshot(post(3000)).await.unwrap()).await;
        assert_eq!(body["data"]["applied"], false);
        assert_eq!(body["data"]["rejection"], "value_out_of_range");
        // Kept for the next start of the server.
        assert_eq!(webrtc::load_bitrate(&state_dir), 5000);
    }

    /// The WebRTC quality and the H.264 engine's choice are the server's own
    /// settings: offered, and served, only where the video controls are
    /// enabled; the choice also only where the H.264 encoder's report is
    /// configured, which the encoder that knows the choice writes, so that a
    /// slot on a board with an older encoder does not offer a choice that
    /// changes nothing.
    #[cfg(feature = "restricted-control-surface")]
    #[tokio::test]
    async fn the_servers_own_video_settings_are_offered_and_served_only_where_configured() {
        let root = tempdir().unwrap();
        for (controls, report) in [(true, true), (true, false), (false, true), (false, false)] {
            let tag = format!("controls {controls}, report {report}");
            let app = app_with_options(ServerOptions {
                state_dir: root.path().join(format!("account-{controls}-{report}")),
                restricted_controls_enabled: true,
                video_controls_enabled: controls,
                control_socket: Some(root.path().join("control.sock")),
                webrtc_janus: Some("127.0.0.1:8188".parse().unwrap()),
                webrtc_h264_demand_socket: Some(root.path().join("h264.sock")),
                h264_encoder_report: report.then(|| root.path().join("report.json")),
                ..ServerOptions::default()
            });
            let (cookie, csrf) = setup_and_login(&app).await;
            let response = app
                .clone()
                .oneshot(json_request(Method::GET, "/api/capabilities", Value::Null))
                .await
                .unwrap();
            let settings = json_body(response).await["data"]["video"]["settings"].clone();
            let offered = |name: &str| settings.as_array().unwrap().contains(&Value::from(name));
            let post = |kind: &str, value: i64| {
                let mut request = json_request(
                    Method::POST,
                    "/api/vm/screen",
                    serde_json::json!({"type": kind, "value": value}),
                );
                request
                    .headers_mut()
                    .insert(header::COOKIE, cookie.parse().unwrap());
                request
                    .headers_mut()
                    .insert("x-csrf-token", csrf.parse().unwrap());
                request
            };
            for (name, kind, value, is_offered) in [
                ("h264_engine", "h264_engine", 0, controls && report),
                ("webrtc_bitrate", "webrtc_kbps", 5000, controls),
            ] {
                assert_eq!(offered(name), is_offered, "{tag}: {settings}");
                let body = json_body(app.clone().oneshot(post(kind, value)).await.unwrap()).await;
                assert_eq!(body["data"]["applied"], is_offered, "{tag}, {kind}: {body}");
                if !is_offered {
                    assert_eq!(
                        body["data"]["rejection"], "unsupported_setting",
                        "{tag}, {kind}: {body}"
                    );
                }
            }
        }
    }

    /// The owner's side for the screen's read-backs: the settings it runs
    /// the engine with, as often as it is asked.
    #[cfg(feature = "restricted-control-surface")]
    fn owner_reading_back(listener: UnixListener) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut prefix = [0_u8; 4];
                stream.read_exact(&mut prefix).await.unwrap();
                let mut payload = vec![0_u8; u32::from_be_bytes(prefix) as usize];
                stream.read_exact(&mut payload).await.unwrap();
                let request = bli_kvm_control_protocol::decode_request_v2(&payload).unwrap();
                assert!(matches!(
                    request.request,
                    bli_kvm_core::PlatformControlRequestV2::VideoSettingsState {}
                ));
                let envelope = bli_kvm_control_protocol::ControlResponseEnvelope {
                    schema_version: bli_kvm_control_protocol::CONTROL_SCHEMA_VERSION_V2,
                    request_id: request.request_id,
                    operation_id: request.operation_id,
                    response: bli_kvm_control_protocol::ControlResponse::VideoState {
                        resolution: Some("1920x1080".into()),
                        fps: 60,
                        quality: 100,
                    },
                };
                let payload = bli_kvm_control_protocol::encode_response_v2(&envelope).unwrap();
                stream
                    .write_all(&bli_kvm_control_protocol::encode_frame(&payload).unwrap())
                    .await
                    .unwrap();
            }
        })
    }

    /// The board's choice of the H.264 engine: offered where the H.264
    /// encoder's demand socket is, chosen by its two values only, kept for
    /// the next start, and read back with what the encoder's report says of
    /// the engine.
    #[cfg(feature = "restricted-control-surface")]
    #[tokio::test]
    async fn the_h264_engine_is_offered_chosen_kept_and_reported() {
        let root = tempdir().unwrap();
        let state_dir = root.path().join("account");
        let control_socket = root.path().join("control.sock");
        let owner = owner_reading_back(UnixListener::bind(&control_socket).unwrap());
        let report = root.path().join("report.json");
        let options = ServerOptions {
            state_dir: state_dir.clone(),
            restricted_controls_enabled: true,
            video_controls_enabled: true,
            control_socket: Some(control_socket.clone()),
            webrtc_janus: Some("127.0.0.1:8188".parse().unwrap()),
            webrtc_h264_demand_socket: Some(root.path().join("h264.sock")),
            h264_encoder_report: Some(report.clone()),
            ..ServerOptions::default()
        };
        let app = app_with_options(options.clone());
        let operator = setup_and_login(&app).await;
        // Another server on the same state: the account is there.
        let login = |app: Router| async move {
            let login = app
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
            let csrf = json_body(login).await["data"]["csrfToken"]
                .as_str()
                .unwrap()
                .to_owned();
            (cookie, csrf)
        };
        let capabilities = |app: Router| async move {
            let response = app
                .oneshot(json_request(Method::GET, "/api/capabilities", Value::Null))
                .await
                .unwrap();
            json_body(response).await["data"]["video"]["settings"].clone()
        };
        let choose = |app: Router, (cookie, csrf): (String, String), value: i64| async move {
            let mut request = json_request(
                Method::POST,
                "/api/vm/screen",
                serde_json::json!({"type": "h264_engine", "value": value}),
            );
            request
                .headers_mut()
                .insert(header::COOKIE, cookie.parse().unwrap());
            request
                .headers_mut()
                .insert("x-csrf-token", csrf.parse().unwrap());
            json_body(app.oneshot(request).await.unwrap()).await
        };
        let read_back = |app: Router, (cookie, _): (String, String)| async move {
            let request = Request::builder()
                .uri("/api/vm/screen")
                .header(header::HOST, "kvm.test")
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap();
            json_body(app.oneshot(request).await.unwrap()).await["data"].clone()
        };
        let settings = capabilities(app.clone()).await;
        assert!(
            settings
                .as_array()
                .unwrap()
                .contains(&Value::from("h264_engine")),
            "{settings}"
        );
        // Nothing chosen and no report yet: the engine, availability unknown.
        let screen = read_back(app.clone(), operator.clone()).await;
        assert_eq!(screen["h264Engine"], "hardware", "{screen}");
        assert!(screen.get("h264HardwareAvailable").is_none(), "{screen}");
        assert!(screen.get("h264HardwareReason").is_none(), "{screen}");
        // The encoder reports an engine it cannot have.
        let report_of = |available: &str, reason: &str| {
            format!(
                r#"{{"schema":1,"codec":"h264","streaming":true,"engine":"x264","engineWanted":"hardware","hardwareAvailable":{available},"hardwareReason":{reason}}}"#
            )
        };
        std::fs::write(&report, report_of("false", r#""/dev/x: busy""#)).unwrap();
        let screen = read_back(app.clone(), operator.clone()).await;
        assert_eq!(screen["h264HardwareAvailable"], false, "{screen}");
        assert_eq!(screen["h264HardwareReason"], "/dev/x: busy");

        let body = choose(app.clone(), operator.clone(), 0).await;
        assert_eq!(body["data"]["applied"], true, "{body}");
        assert_eq!(body["data"]["h264Engine"], "software");
        for refused in [2, -1, 1 << 40] {
            let body = choose(app.clone(), operator.clone(), refused).await;
            assert_eq!(body["data"]["applied"], false, "{body}");
            assert_eq!(body["data"]["rejection"], "value_out_of_range");
        }
        std::fs::write(&report, report_of("true", "null")).unwrap();
        let screen = read_back(app.clone(), operator.clone()).await;
        assert_eq!(screen["h264Engine"], "software", "{screen}");
        assert_eq!(screen["h264HardwareAvailable"], true);
        assert!(screen.get("h264HardwareReason").is_none(), "{screen}");
        // Kept for the next start of the server.
        assert_eq!(
            webrtc::load_engine(&state_dir),
            webrtc::H264Engine::Software
        );
        let restarted = app_with_options(options.clone());
        let again = login(restarted.clone()).await;
        assert_eq!(read_back(restarted, again).await["h264Engine"], "software");
        // A damaged report is no report.
        std::fs::write(&report, "{").unwrap();
        let screen = read_back(app.clone(), operator.clone()).await;
        assert!(screen.get("h264HardwareAvailable").is_none(), "{screen}");
        let body = choose(app.clone(), operator.clone(), 1).await;
        assert_eq!(body["data"]["h264Engine"], "hardware", "{body}");

        // Without the H.264 encoder's demand socket nothing is offered,
        // chosen or reported.
        let without = app_with_options(ServerOptions {
            webrtc_h264_demand_socket: None,
            webrtc_demand_socket: Some(root.path().join("vp8.sock")),
            ..options
        });
        let settings = capabilities(without.clone()).await;
        assert!(
            !settings
                .as_array()
                .unwrap()
                .contains(&Value::from("h264_engine")),
            "{settings}"
        );
        let credentials = login(without.clone()).await;
        let body = choose(without.clone(), credentials.clone(), 0).await;
        assert_eq!(body["data"]["rejection"], "unsupported_setting", "{body}");
        assert!(
            read_back(without, credentials)
                .await
                .get("h264Engine")
                .is_none()
        );
        assert_eq!(
            webrtc::load_engine(&state_dir),
            webrtc::H264Engine::Hardware
        );
        owner.abort();
    }

    /// The quality monitor's answer: for a logged-in session only, with
    /// every section; built once a second, so that many readers within it
    /// cost one build and see the same answer; rates and shares once there
    /// are two reads.
    #[tokio::test]
    async fn the_quality_answer_is_built_once_a_second_from_the_reports() {
        let root = tempdir().unwrap();
        let report = root.path().join("h264-report.json");
        let cgroups = root.path().join("cgroups");
        let unit = cgroups.join("hardened-blikvm-video-encoder-h264.service");
        std::fs::create_dir_all(&unit).unwrap();
        std::fs::write(unit.join("cpu.stat"), "usage_usec 1000000\n").unwrap();
        let mut written = crate::encoder_report::tests::whole_report("h264");
        // The reports' times are set and not taken from the clock: a time ahead
        // of now is a report written just now, however long a login takes on a
        // loaded host, and two of them a second apart give a rate over exactly
        // one second. The answer shows no report older than three seconds.
        let ahead = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
        let put = |value: &serde_json::Value, second: u64| {
            std::fs::write(&report, value.to_string()).unwrap();
            let file = std::fs::File::options().write(true).open(&report).unwrap();
            file.set_modified(ahead + std::time::Duration::from_secs(second))
                .unwrap();
        };
        let app = app_with_options(ServerOptions {
            state_dir: root.path().join("account"),
            h264_encoder_report: Some(report.clone()),
            vp8_encoder_report: Some(root.path().join("no-vp8-report.json")),
            unit_cgroups: cgroups,
            ..ServerOptions::default()
        });
        let anonymous = app
            .clone()
            .oneshot(json_request(Method::GET, "/api/vm/quality", Value::Null))
            .await
            .unwrap();
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
        let (cookie, _) = setup_and_login(&app).await;
        let read = |app: Router| {
            let cookie = cookie.clone();
            async move {
                let request = Request::builder()
                    .uri("/api/vm/quality")
                    .header(header::HOST, "kvm.test")
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap();
                let response = app.oneshot(request).await.unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                json_body(response).await["data"].clone()
            }
        };
        put(&written, 0);
        let first = read(app.clone()).await;
        let first_read = std::time::Instant::now();
        for key in [
            "capture",
            "h264",
            "vp8",
            "units",
            "temperatureMillidegrees",
            "version",
        ] {
            assert!(first.get(key).is_some(), "{key} missing: {first}");
        }
        for key in ["width", "height", "fps", "queuedFps", "quality"] {
            assert!(
                first["capture"].get(key).is_some(),
                "capture {key}: {first}"
            );
        }
        assert_eq!(first["h264"]["engine"], "hardware");
        assert_eq!(first["h264"]["encodedFrames"], 1000);
        assert!(first["h264"]["fps"].is_null(), "one report: no rate yet");
        assert!(first["vp8"].is_null());
        assert!(first["units"]["h264CpuPercent"].is_null());
        assert!(first["units"]["vp8CpuPercent"].is_null());
        assert!(first["version"].is_null());
        // A new report and more processor time within the second: the same
        // answer, built once for every reader.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        written["encodedFrames"] = 1030.into();
        put(&written, 1);
        std::fs::write(unit.join("cpu.stat"), "usage_usec 1500000\n").unwrap();
        for _ in 0..20 {
            // Within the second of the first build; a host so loaded that
            // the reads outlast it is no failure of the cache.
            let within = first_read.elapsed() < std::time::Duration::from_millis(900);
            let answer = read(app.clone()).await;
            if within {
                assert_eq!(answer, first);
            }
        }
        // After the second: the rates of the two reports, thirty frames in
        // the second between their times, and the unit's share.
        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
        let later = read(app.clone()).await;
        assert_eq!(later["h264"]["encodedFrames"], 1030);
        assert_eq!(later["h264"]["fps"].as_f64(), Some(30.0), "{later}");
        assert!(later["h264"]["hardware"].get("passesPerSecond").is_some());
        assert!(later["units"]["h264CpuPercent"].as_f64().unwrap() > 0.0);
        // A damaged report is null, and nothing else changes.
        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
        std::fs::write(&report, "{").unwrap();
        let damaged = read(app.clone()).await;
        assert!(damaged["h264"].is_null());
        assert!(damaged.get("units").is_some());
    }

    #[tokio::test]
    async fn session_lock_round_trips_and_requires_csrf() {
        let app = isolated_app();
        let (cookie, csrf) = setup_and_login(&app).await;
        let (status, body) = session_lock_state(&app, &cookie, None).await;
        assert_eq!(status, StatusCode::OK);
        // The reference default: one hour.
        assert_eq!(body["data"]["duration"], 3600);

        let (status, body) = set_session_lock(&app, &cookie, Some(&csrf), 300, false).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["duration"], 300);

        let (status, _) = set_session_lock(&app, &cookie, None, 60, false).await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        let (status, body) = session_lock_state(&app, &cookie, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body["data"]["duration"], 300,
            "a rejected write must not change the policy"
        );
    }

    async fn screen_area_call(
        app: &Router,
        cookie: &str,
        csrf: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method(if body.is_some() {
                Method::POST
            } else {
                Method::GET
            })
            .uri("/api/hid/screen-area")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::HOST, "kvm.test")
            .header(header::ORIGIN, "https://kvm.test")
            .header(header::COOKIE, cookie);
        if let Some(csrf) = csrf {
            request = request.header("x-csrf-token", csrf);
        }
        let response = app
            .clone()
            .oneshot(
                request
                    .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        (status, json_body(response).await)
    }

    #[tokio::test]
    async fn the_screen_area_round_trips_and_a_screen_outside_its_desktop_is_refused() {
        let app = isolated_app();
        let (cookie, csrf) = setup_and_login(&app).await;
        let (status, body) = screen_area_call(&app, &cookie, None, None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            body["data"]["area"].is_null(),
            "the whole desktop by default"
        );

        let left = serde_json::json!({
            "desktop": {"width": 5120, "height": 1440},
            "screen": {"x": 0, "y": 0, "width": 2560, "height": 1440}
        });
        let (status, body) = screen_area_call(
            &app,
            &cookie,
            Some(&csrf),
            Some(serde_json::json!({"area": left})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["area"], left);

        // Without the CSRF token, or a screen beyond its desktop: refused,
        // and the layout stays.
        let (status, _) =
            screen_area_call(&app, &cookie, None, Some(serde_json::json!({"area": null}))).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let outside = serde_json::json!({"area": {
            "desktop": {"width": 5120, "height": 1440},
            "screen": {"x": 2600, "y": 0, "width": 2560, "height": 1440}
        }});
        // The reference's API answers a refusal with its own code.
        let (_, body) = screen_area_call(&app, &cookie, Some(&csrf), Some(outside)).await;
        assert_ne!(body["code"], 0);
        let (_, body) = screen_area_call(&app, &cookie, None, None).await;
        assert_eq!(body["data"]["area"], left);

        // The whole desktop again.
        let (status, body) = screen_area_call(
            &app,
            &cookie,
            Some(&csrf),
            Some(serde_json::json!({"area": null})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["data"]["area"].is_null());
    }

    #[tokio::test]
    async fn sessions_live_as_long_as_the_session_lock_says() {
        let now = || {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
        };
        let app = isolated_app();
        let (cookie, csrf) = setup_and_login(&app).await;
        let (_, account) = {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/auth/account")
                        .header(header::HOST, "kvm.test")
                        .header(header::COOKIE, &cookie)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            (response.status(), json_body(response).await)
        };
        // Login used the reference default of one hour, not 15 minutes.
        let expires = account["data"]["expiresAt"].as_u64().unwrap();
        assert!(
            (now() + 3590..=now() + 3600).contains(&expires),
            "{account}"
        );

        // A change retimes the caller's own session and its cookie.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/vm/session-lock")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::HOST, "kvm.test")
                    .header(header::ORIGIN, "https://kvm.test")
                    .header(header::COOKIE, &cookie)
                    .header("x-csrf-token", &csrf)
                    .body(Body::from(r#"{"duration":1800}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let set_cookie = response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .to_owned();
        assert!(set_cookie.contains("Max-Age=1800"), "{set_cookie}");
        let body = json_body(response).await;
        let expires = body["data"]["expiresAt"].as_u64().unwrap();
        assert!((now() + 1790..=now() + 1800).contains(&expires), "{body}");
    }

    #[tokio::test]
    async fn session_lock_expires_an_idle_session() {
        let app = isolated_app();
        let (cookie, csrf) = setup_and_login(&app).await;
        let (status, _) = set_session_lock(&app, &cookie, Some(&csrf), 1, false).await;
        assert_eq!(status, StatusCode::OK);

        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/vm/info")
                    .header(header::HOST, "kvm.test")
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        // Turning the policy off restores normal sessions.
        let app = isolated_app();
        let (cookie, csrf) = setup_and_login(&app).await;
        let (status, _) = set_session_lock(&app, &cookie, Some(&csrf), 0, false).await;
        assert_eq!(status, StatusCode::OK);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/vm/info")
                    .header(header::HOST, "kvm.test")
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn session_lock_now_revokes_the_other_session() {
        let app = isolated_app();
        let (cookie, csrf) = setup_and_login(&app).await;
        let other = login_again(&app).await;
        let (status, _) = set_session_lock(&app, &cookie, Some(&csrf), 0, true).await;
        assert_eq!(status, StatusCode::OK);

        for (cookie, expected) in [
            (&other, StatusCode::UNAUTHORIZED),
            (&cookie, StatusCode::OK),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/vm/info")
                        .header(header::HOST, "kvm.test")
                        .header(header::COOKIE, cookie)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
        }
    }

    #[cfg(feature = "restricted-control-surface")]
    #[tokio::test]
    async fn system_action_requires_authentication_and_the_explicit_opt_in() {
        // No session.
        let response = app()
            .oneshot(json_request(
                Method::POST,
                "/api/vm/system/reboot",
                serde_json::json!({"confirm": true}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        // Session, but the capability is not armed: refused before any IPC.
        let root = tempdir().unwrap();
        let app = app_with_options(ServerOptions {
            state_dir: root.path().join("account"),
            control_socket: Some(root.path().join("control.sock")),
            ..ServerOptions::default()
        });
        let (cookie, csrf) = setup_and_login(&app).await;
        let mut request = json_request(
            Method::POST,
            "/api/vm/system/reboot",
            serde_json::json!({"confirm": true}),
        );
        request
            .headers_mut()
            .insert(header::COOKIE, cookie.parse().unwrap());
        request
            .headers_mut()
            .insert("x-csrf-token", csrf.parse().unwrap());
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

        let capabilities = json_body(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/capabilities")
                        .header(header::HOST, "kvm.test")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            capabilities["data"]["system"]["actions"]["availability"],
            "unavailable"
        );
    }

    #[cfg(feature = "restricted-control-surface")]
    #[tokio::test]
    async fn the_board_profile_inventory_reports_the_host_power_led() {
        let root = tempdir().unwrap();
        let control_socket = root.path().join("control.sock");
        let listener = UnixListener::bind(&control_socket).unwrap();
        let broker = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut prefix = [0_u8; 4];
            stream.read_exact(&mut prefix).await.unwrap();
            let mut payload = vec![0_u8; u32::from_be_bytes(prefix) as usize];
            stream.read_exact(&mut payload).await.unwrap();
            let request = bli_kvm_control_protocol::decode_request_v2(&payload).unwrap();
            assert!(matches!(
                request.request,
                bli_kvm_core::PlatformControlRequestV2::BoardStatus {}
            ));
            let envelope = bli_kvm_control_protocol::ControlResponseEnvelope {
                schema_version: bli_kvm_control_protocol::CONTROL_SCHEMA_VERSION_V2,
                request_id: request.request_id,
                operation_id: request.operation_id,
                response: bli_kvm_control_protocol::ControlResponse::BoardStatus {
                    power_led_present: true,
                    power_led_on: true,
                    hdd_led_present: true,
                    hdd_led_on: false,
                },
            };
            let payload = bli_kvm_control_protocol::encode_response_v2(&envelope).unwrap();
            stream
                .write_all(&bli_kvm_control_protocol::encode_frame(&payload).unwrap())
                .await
                .unwrap();
        });
        let app = app_with_options(ServerOptions {
            state_dir: root.path().join("account"),
            control_socket: Some(control_socket),
            restricted_controls_enabled: true,
            ..ServerOptions::default()
        });
        let (cookie, _csrf) = setup_and_login(&app).await;
        let request = Request::builder()
            .uri("/api/inventory")
            .header(header::HOST, "kvm.test")
            .header(header::COOKIE, cookie)
            .body(Body::empty())
            .unwrap();
        let body = json_body(app.clone().oneshot(request).await.unwrap()).await;
        assert_eq!(body["code"], 0);
        assert_eq!(body["data"]["backend"], "native");
        assert_eq!(body["data"]["powerLed"], true);
        broker.await.unwrap();
    }

    #[cfg(feature = "restricted-control-surface")]
    #[tokio::test]
    async fn the_board_profile_diagnostics_use_the_native_inventory() {
        let root = tempdir().unwrap();
        let control_socket = root.path().join("control.sock");
        let listener = UnixListener::bind(&control_socket).unwrap();
        let broker = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut prefix = [0_u8; 4];
            stream.read_exact(&mut prefix).await.unwrap();
            let mut payload = vec![0_u8; u32::from_be_bytes(prefix) as usize];
            stream.read_exact(&mut payload).await.unwrap();
            let request = bli_kvm_control_protocol::decode_request_v2(&payload).unwrap();
            assert!(matches!(
                request.request,
                bli_kvm_core::PlatformControlRequestV2::BoardStatus {}
            ));
            let envelope = bli_kvm_control_protocol::ControlResponseEnvelope {
                schema_version: bli_kvm_control_protocol::CONTROL_SCHEMA_VERSION_V2,
                request_id: request.request_id,
                operation_id: request.operation_id,
                response: bli_kvm_control_protocol::ControlResponse::BoardStatus {
                    power_led_present: true,
                    power_led_on: true,
                    hdd_led_present: true,
                    hdd_led_on: false,
                },
            };
            let payload = bli_kvm_control_protocol::encode_response_v2(&envelope).unwrap();
            stream
                .write_all(&bli_kvm_control_protocol::encode_frame(&payload).unwrap())
                .await
                .unwrap();
        });
        let app = app_with_options(ServerOptions {
            state_dir: root.path().join("account"),
            control_socket: Some(control_socket),
            restricted_controls_enabled: true,
            ..ServerOptions::default()
        });
        let (cookie, _csrf) = setup_and_login(&app).await;
        let request = Request::builder()
            .uri("/api/diagnostics")
            .header(header::HOST, "kvm.test")
            .header(header::COOKIE, cookie)
            .body(Body::empty())
            .unwrap();
        let body = json_body(app.clone().oneshot(request).await.unwrap()).await;
        assert_eq!(body["code"], 0);
        assert_eq!(body["data"]["availability"], "current_only");
        assert_eq!(body["data"]["current"]["components"]["powerLed"], "active");
        broker.await.unwrap();
    }

    #[cfg(feature = "restricted-control-surface")]
    #[tokio::test]
    async fn the_display_sleep_is_forwarded_read_back_and_other_times_are_refused() {
        let root = tempdir().unwrap();
        let control_socket = root.path().join("control.sock");
        let listener = UnixListener::bind(&control_socket).unwrap();
        let broker = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut prefix = [0_u8; 4];
                stream.read_exact(&mut prefix).await.unwrap();
                let mut payload = vec![0_u8; u32::from_be_bytes(prefix) as usize];
                stream.read_exact(&mut payload).await.unwrap();
                let request = bli_kvm_control_protocol::decode_request_v2(&payload).unwrap();
                let response = match request.request {
                    bli_kvm_core::PlatformControlRequestV2::DisplaySleepState {} => {
                        bli_kvm_control_protocol::ControlResponse::DisplaySleepState {
                            seconds: 300,
                        }
                    }
                    bli_kvm_core::PlatformControlRequestV2::DisplaySleep { seconds } => {
                        assert_eq!(seconds, 60);
                        bli_kvm_control_protocol::ControlResponse::Applied
                    }
                    _ => panic!("unexpected request"),
                };
                let envelope = bli_kvm_control_protocol::ControlResponseEnvelope {
                    schema_version: bli_kvm_control_protocol::CONTROL_SCHEMA_VERSION_V2,
                    request_id: request.request_id,
                    operation_id: request.operation_id,
                    response,
                };
                let payload = bli_kvm_control_protocol::encode_response_v2(&envelope).unwrap();
                stream
                    .write_all(&bli_kvm_control_protocol::encode_frame(&payload).unwrap())
                    .await
                    .unwrap();
            }
        });
        let app = app_with_options(ServerOptions {
            state_dir: root.path().join("account"),
            control_socket: Some(control_socket),
            restricted_controls_enabled: true,
            ..ServerOptions::default()
        });
        let (cookie, csrf) = setup_and_login(&app).await;

        let state_request = Request::builder()
            .uri("/api/vm/oled")
            .header(header::HOST, "kvm.test")
            .header(header::COOKIE, cookie.clone())
            .body(Body::empty())
            .unwrap();
        let body = json_body(app.clone().oneshot(state_request).await.unwrap()).await;
        assert_eq!(body["code"], 0);
        assert_eq!(body["data"]["exist"], true);
        assert_eq!(body["data"]["sleep"], 300);

        // A time that is not one of the choices never reaches the owner.
        for (sleep, applied) in [(20, false), (60, true)] {
            let mut update = json_request(
                Method::POST,
                "/api/vm/oled",
                serde_json::json!({ "sleep": sleep }),
            );
            update
                .headers_mut()
                .insert(header::COOKIE, cookie.parse().unwrap());
            update
                .headers_mut()
                .insert("x-csrf-token", csrf.parse().unwrap());
            let body = json_body(app.clone().oneshot(update).await.unwrap()).await;
            assert_eq!(body["code"], 0);
            assert_eq!(body["data"]["applied"], applied, "{sleep}");
            if !applied {
                assert_eq!(body["data"]["rejection"], "unsupported_time");
            }
        }
        broker.await.unwrap();
    }

    #[cfg(feature = "restricted-control-surface")]
    #[tokio::test]
    async fn mouse_jiggler_is_forwarded_read_back_and_absolute_is_refused() {
        let root = tempdir().unwrap();
        let control_socket = root.path().join("control.sock");
        let listener = UnixListener::bind(&control_socket).unwrap();
        let broker = tokio::spawn(async move {
            let mut served = 0_u32;
            while served < 2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut prefix = [0_u8; 4];
                stream.read_exact(&mut prefix).await.unwrap();
                let mut payload = vec![0_u8; u32::from_be_bytes(prefix) as usize];
                stream.read_exact(&mut payload).await.unwrap();
                let request = bli_kvm_control_protocol::decode_request_v2(&payload).unwrap();
                let response = match request.request {
                    bli_kvm_core::PlatformControlRequestV2::MouseJigglerState {} => {
                        bli_kvm_control_protocol::ControlResponse::MouseJigglerState {
                            enabled: true,
                            mode: bli_kvm_core::MouseJigglerMode::Relative,
                        }
                    }
                    bli_kvm_core::PlatformControlRequestV2::MouseJiggler { change } => {
                        assert!(change.enabled);
                        assert_eq!(change.mode, bli_kvm_core::MouseJigglerMode::Relative);
                        served = 9;
                        bli_kvm_control_protocol::ControlResponse::Applied
                    }
                    _ => panic!("unexpected request"),
                };
                served = if served == 9 { 2 } else { served + 1 };
                let envelope = bli_kvm_control_protocol::ControlResponseEnvelope {
                    schema_version: bli_kvm_control_protocol::CONTROL_SCHEMA_VERSION_V2,
                    request_id: request.request_id,
                    operation_id: request.operation_id,
                    response,
                };
                let payload = bli_kvm_control_protocol::encode_response_v2(&envelope).unwrap();
                stream
                    .write_all(&bli_kvm_control_protocol::encode_frame(&payload).unwrap())
                    .await
                    .unwrap();
            }
        });
        let app = app_with_options(ServerOptions {
            state_dir: root.path().join("account"),
            control_socket: Some(control_socket),
            restricted_controls_enabled: true,
            ..ServerOptions::default()
        });
        let (cookie, csrf) = setup_and_login(&app).await;

        let state_request = Request::builder()
            .uri("/api/vm/mouse-jiggler")
            .header(header::HOST, "kvm.test")
            .header(header::COOKIE, cookie.clone())
            .body(Body::empty())
            .unwrap();
        let body = json_body(app.clone().oneshot(state_request).await.unwrap()).await;
        assert_eq!(body["code"], 0);
        assert_eq!(body["data"]["enabled"], true);
        assert_eq!(body["data"]["mode"], "relative");

        // The absolute mode needs a pointer position the target never reports.
        let mut absolute = json_request(
            Method::POST,
            "/api/vm/mouse-jiggler",
            serde_json::json!({"enabled": true, "mode": "absolute"}),
        );
        absolute
            .headers_mut()
            .insert(header::COOKIE, cookie.parse().unwrap());
        absolute
            .headers_mut()
            .insert("x-csrf-token", csrf.parse().unwrap());
        let body = json_body(app.clone().oneshot(absolute).await.unwrap()).await;
        assert_eq!(body["code"], 0);
        assert_eq!(body["data"]["rejection"], "unsupported_mode");

        let mut relative = json_request(
            Method::POST,
            "/api/vm/mouse-jiggler",
            serde_json::json!({"enabled": true, "mode": "relative"}),
        );
        relative
            .headers_mut()
            .insert(header::COOKIE, cookie.parse().unwrap());
        relative
            .headers_mut()
            .insert("x-csrf-token", csrf.parse().unwrap());
        let body = json_body(app.clone().oneshot(relative).await.unwrap()).await;
        assert_eq!(body["code"], 0);
        assert_eq!(body["data"]["applied"], true);
        broker.await.unwrap();
    }

    #[cfg(feature = "restricted-control-surface")]
    #[tokio::test]
    async fn mdns_toggles_are_forwarded_and_read_back() {
        let root = tempdir().unwrap();
        let control_socket = root.path().join("control.sock");
        let listener = UnixListener::bind(&control_socket).unwrap();
        let broker = tokio::spawn(async move {
            let mut served = 0_u32;
            while served < 3 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut prefix = [0_u8; 4];
                stream.read_exact(&mut prefix).await.unwrap();
                let mut payload = vec![0_u8; u32::from_be_bytes(prefix) as usize];
                stream.read_exact(&mut payload).await.unwrap();
                let request = bli_kvm_control_protocol::decode_request_v2(&payload).unwrap();
                let response = match request.request {
                    bli_kvm_core::PlatformControlRequestV2::MdnsState {} => {
                        bli_kvm_control_protocol::ControlResponse::MdnsState { enabled: false }
                    }
                    bli_kvm_core::PlatformControlRequestV2::Mdns { enabled } => {
                        assert!(enabled, "the enable route asks for the enabled state");
                        served = 9;
                        bli_kvm_control_protocol::ControlResponse::Applied
                    }
                    _ => panic!("unexpected request"),
                };
                served = if served == 9 { 3 } else { served + 1 };
                let envelope = bli_kvm_control_protocol::ControlResponseEnvelope {
                    schema_version: bli_kvm_control_protocol::CONTROL_SCHEMA_VERSION_V2,
                    request_id: request.request_id,
                    operation_id: request.operation_id,
                    response,
                };
                let payload = bli_kvm_control_protocol::encode_response_v2(&envelope).unwrap();
                stream
                    .write_all(&bli_kvm_control_protocol::encode_frame(&payload).unwrap())
                    .await
                    .unwrap();
            }
        });
        let app = app_with_options(ServerOptions {
            state_dir: root.path().join("account"),
            control_socket: Some(control_socket),
            restricted_controls_enabled: true,
            ..ServerOptions::default()
        });
        let (cookie, csrf) = setup_and_login(&app).await;

        let state_request = Request::builder()
            .uri("/api/vm/mdns")
            .header(header::HOST, "kvm.test")
            .header(header::COOKIE, cookie.clone())
            .body(Body::empty())
            .unwrap();
        let body = json_body(app.clone().oneshot(state_request).await.unwrap()).await;
        assert_eq!(body["code"], 0);
        assert_eq!(body["data"]["enabled"], false);

        let mut enable = Request::builder()
            .method(Method::POST)
            .uri("/api/vm/mdns/enable")
            .header(header::HOST, "kvm.test")
            .header(header::ORIGIN, "https://kvm.test")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, cookie)
            .header("x-csrf-token", csrf)
            .body(Body::from("{}"))
            .unwrap();
        enable
            .headers_mut()
            .insert(header::CONTENT_LENGTH, "2".parse().unwrap());
        let body = json_body(app.clone().oneshot(enable).await.unwrap()).await;
        assert_eq!(body["code"], 0);
        assert_eq!(body["data"]["applied"], true);
        broker.await.unwrap();
    }

    #[cfg(feature = "restricted-control-surface")]
    #[tokio::test]
    async fn video_settings_are_forwarded_and_read_back() {
        let root = tempdir().unwrap();
        let control_socket = root.path().join("control.sock");
        let listener = UnixListener::bind(&control_socket).unwrap();
        let broker = tokio::spawn(async move {
            let mut served = 0_u32;
            while served < 2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut prefix = [0_u8; 4];
                stream.read_exact(&mut prefix).await.unwrap();
                let mut payload = vec![0_u8; u32::from_be_bytes(prefix) as usize];
                stream.read_exact(&mut payload).await.unwrap();
                let request = bli_kvm_control_protocol::decode_request_v2(&payload).unwrap();
                let response = match request.request {
                    bli_kvm_core::PlatformControlRequestV2::VideoSettings { change } => {
                        assert_eq!(change.fps, Some(24));
                        assert_eq!(change.quality, None);
                        bli_kvm_control_protocol::ControlResponse::Applied
                    }
                    bli_kvm_core::PlatformControlRequestV2::VideoSettingsState {} => {
                        bli_kvm_control_protocol::ControlResponse::VideoState {
                            resolution: Some("1280x720".into()),
                            fps: 24,
                            quality: 80,
                        }
                    }
                    _ => panic!("unexpected request"),
                };
                served += 1;
                let envelope = bli_kvm_control_protocol::ControlResponseEnvelope {
                    schema_version: bli_kvm_control_protocol::CONTROL_SCHEMA_VERSION_V2,
                    request_id: request.request_id,
                    operation_id: request.operation_id,
                    response,
                };
                let payload = bli_kvm_control_protocol::encode_response_v2(&envelope).unwrap();
                stream
                    .write_all(&bli_kvm_control_protocol::encode_frame(&payload).unwrap())
                    .await
                    .unwrap();
            }
        });
        let app = app_with_options(ServerOptions {
            state_dir: root.path().join("account"),
            control_socket: Some(control_socket),
            restricted_controls_enabled: true,
            video_controls_enabled: true,
            ..ServerOptions::default()
        });
        let (cookie, csrf) = setup_and_login(&app).await;

        let mut update = json_request(
            Method::POST,
            "/api/vm/screen",
            serde_json::json!({"type": "fps", "value": 24}),
        );
        update
            .headers_mut()
            .insert(header::COOKIE, cookie.parse().unwrap());
        update
            .headers_mut()
            .insert("x-csrf-token", csrf.parse().unwrap());
        let body = json_body(app.clone().oneshot(update).await.unwrap()).await;
        assert_eq!(body["code"], 0);
        assert_eq!(body["data"]["applied"], true);

        // An unsupported menu entry is refused without touching the owner.
        let mut unsupported = json_request(
            Method::POST,
            "/api/vm/screen",
            serde_json::json!({"type": "gop", "value": 30}),
        );
        unsupported
            .headers_mut()
            .insert(header::COOKIE, cookie.parse().unwrap());
        unsupported
            .headers_mut()
            .insert("x-csrf-token", csrf.parse().unwrap());
        let body = json_body(app.clone().oneshot(unsupported).await.unwrap()).await;
        assert_eq!(body["code"], 0);
        assert_eq!(body["data"]["applied"], false);
        assert_eq!(body["data"]["rejection"], "unsupported_setting");

        // The read-back reports the settings the owner will run the engine with.
        let read_back = Request::builder()
            .uri("/api/vm/screen")
            .header(header::HOST, "kvm.test")
            .header(header::COOKIE, cookie)
            .body(Body::empty())
            .unwrap();
        let body = json_body(app.clone().oneshot(read_back).await.unwrap()).await;
        assert_eq!(body["code"], 0);
        assert_eq!(body["data"]["fps"], 24);
        assert_eq!(body["data"]["quality"], 80);

        // The capability is advertised only with the reviewed opt-in.
        let capabilities = json_body(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/capabilities")
                        .header(header::HOST, "kvm.test")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            capabilities["data"]["video"]["status"]["availability"],
            "read_write"
        );
        broker.await.unwrap();
    }

    #[cfg(feature = "restricted-control-surface")]
    #[tokio::test]
    async fn system_action_forwards_the_confirmation_and_maps_refusals() {
        let root = tempdir().unwrap();
        let control_socket = root.path().join("control.sock");
        let listener = UnixListener::bind(&control_socket).unwrap();
        let broker = tokio::spawn(async move {
            let mut served = 0_u32;
            while served < 2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut prefix = [0_u8; 4];
                stream.read_exact(&mut prefix).await.unwrap();
                let mut payload = vec![0_u8; u32::from_be_bytes(prefix) as usize];
                stream.read_exact(&mut payload).await.unwrap();
                let request = bli_kvm_control_protocol::decode_request_v2(&payload).unwrap();
                let response = match request.request {
                    bli_kvm_core::PlatformControlRequestV2::SystemAction { request } => {
                        assert_eq!(request.action, bli_kvm_core::SystemActionKind::Reboot);
                        assert!(request.session_binding != 0);
                        if served == 0 {
                            bli_kvm_control_protocol::ControlResponse::Applied
                        } else {
                            bli_kvm_control_protocol::ControlResponse::Rejected {
                                code: bli_kvm_control_protocol::ControlRejectionCode::NotArmed,
                            }
                        }
                    }
                    _ => panic!("unexpected request"),
                };
                served += 1;
                let envelope = bli_kvm_control_protocol::ControlResponseEnvelope {
                    schema_version: bli_kvm_control_protocol::CONTROL_SCHEMA_VERSION_V2,
                    request_id: request.request_id,
                    operation_id: request.operation_id,
                    response,
                };
                let payload = bli_kvm_control_protocol::encode_response_v2(&envelope).unwrap();
                stream
                    .write_all(&bli_kvm_control_protocol::encode_frame(&payload).unwrap())
                    .await
                    .unwrap();
            }
        });
        let app = app_with_options(ServerOptions {
            state_dir: root.path().join("account"),
            secure_cookies: true,
            control_socket: Some(control_socket),
            restricted_controls_enabled: true,
            system_actions_enabled: true,
            ..ServerOptions::default()
        });
        let (cookie, csrf) = setup_and_login(&app).await;

        // A request without the explicit confirmation never reaches the owner.
        let mut unconfirmed = json_request(
            Method::POST,
            "/api/vm/system/reboot",
            serde_json::json!({"confirm": false}),
        );
        unconfirmed
            .headers_mut()
            .insert(header::COOKIE, cookie.parse().unwrap());
        unconfirmed
            .headers_mut()
            .insert("x-csrf-token", csrf.parse().unwrap());
        // The reference API reports application errors as HTTP 200 with a
        // negative code, like every other write on this surface.
        let response = app.clone().oneshot(unconfirmed).await.unwrap();
        let status = response.status();
        let body = json_body(response).await;
        assert_eq!(status, StatusCode::OK, "unexpected body: {body}");
        assert_eq!(body["code"], -1);

        // CSRF is required like every other write.
        let mut no_csrf = json_request(
            Method::POST,
            "/api/vm/system/reboot",
            serde_json::json!({"confirm": true}),
        );
        no_csrf
            .headers_mut()
            .insert(header::COOKIE, cookie.parse().unwrap());
        let response = app.clone().oneshot(no_csrf).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        for (expected_applied, expected_rejection) in [(true, None), (false, Some("not_armed"))] {
            let mut request = json_request(
                Method::POST,
                "/api/vm/system/reboot",
                serde_json::json!({"confirm": true}),
            );
            request
                .headers_mut()
                .insert(header::COOKIE, cookie.parse().unwrap());
            request
                .headers_mut()
                .insert("x-csrf-token", csrf.parse().unwrap());
            let body = json_body(app.clone().oneshot(request).await.unwrap()).await;
            assert_eq!(body["data"]["applied"], expected_applied);
            match expected_rejection {
                Some(rejection) => assert_eq!(body["data"]["rejection"], rejection),
                None => assert!(body["data"].get("rejection").is_none()),
            }
        }
        broker.await.unwrap();
    }

    #[tokio::test]
    async fn network_writes_require_authentication_and_the_explicit_opt_in() {
        // No session.
        let response = app()
            .oneshot(json_request(
                Method::POST,
                "/api/network/dns",
                serde_json::json!({"servers": ["1.1.1.1"], "confirm": true}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        // Session, but no opt-in: refused before any IPC.
        let root = tempdir().unwrap();
        let app = app_with_options(ServerOptions {
            state_dir: root.path().join("account"),
            control_socket: Some(root.path().join("control.sock")),
            ..ServerOptions::default()
        });
        let (cookie, csrf) = setup_and_login(&app).await;
        let mut request = json_request(
            Method::POST,
            "/api/network/dns",
            serde_json::json!({"servers": ["1.1.1.1"], "confirm": true}),
        );
        request
            .headers_mut()
            .insert(header::COOKIE, cookie.parse().unwrap());
        request
            .headers_mut()
            .insert("x-csrf-token", csrf.parse().unwrap());
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

        // Enabled, but the request carries no confirmation: refused as invalid.
        let root = tempdir().unwrap();
        let app = app_with_options(ServerOptions {
            state_dir: root.path().join("account"),
            control_socket: Some(root.path().join("control.sock")),
            restricted_controls_enabled: true,
            network_writes_enabled: true,
            ..ServerOptions::default()
        });
        let (cookie, csrf) = setup_and_login(&app).await;
        let mut request = json_request(
            Method::POST,
            "/api/network/ipv6",
            serde_json::json!({"interface": "wlan0", "mode": "bogus"}),
        );
        request
            .headers_mut()
            .insert(header::COOKIE, cookie.parse().unwrap());
        request
            .headers_mut()
            .insert("x-csrf-token", csrf.parse().unwrap());
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let body = json_body(response).await;
        // The enabled handler refuses before any IPC and reports the
        // application-level rejection the reference style uses; without the
        // restricted surface the route is an unavailable stub.
        if cfg!(feature = "restricted-control-surface") {
            assert_eq!(status, StatusCode::OK);
            assert!(
                body["code"].as_i64().unwrap_or(0) < 0,
                "unexpected body: {body}"
            );
        } else {
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        }

        // The Wake-on-LAN route repeats the confirmation requirement.
        let mut request = json_request(
            Method::POST,
            "/api/network/wol",
            serde_json::json!({"mac": "00:11:22:aa:bb:cc"}),
        );
        request
            .headers_mut()
            .insert(header::COOKIE, cookie.parse().unwrap());
        request
            .headers_mut()
            .insert("x-csrf-token", csrf.parse().unwrap());
        let response = app.clone().oneshot(request).await.unwrap();
        if cfg!(feature = "restricted-control-surface") {
            assert_eq!(response.status(), StatusCode::OK);
        } else {
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        }

        let capabilities = json_body(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/capabilities")
                        .header(header::HOST, "kvm.test")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        for component in ["dns", "ipv6", "wol"] {
            let availability = if cfg!(feature = "restricted-control-surface") {
                "read_write"
            } else if component == "wol" {
                "unavailable"
            } else {
                // Native DNS/IPv6 observations remain available without the
                // separately compiled and opted-in mutation surface.
                "read_only"
            };
            assert_eq!(
                capabilities["data"]["network"][component]["availability"],
                availability
            );
        }
        for route in ["/api/network/dns", "/api/network/ipv6"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(route)
                        .header(header::HOST, "kvm.test")
                        .header(header::COOKIE, &cookie)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{route}");
        }
    }

    #[tokio::test]
    async fn media_upload_requires_session_csrf_and_same_origin_before_the_body() {
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
        let app = app_with_options(ServerOptions {
            state_dir: root.path().join("account"),
            ..ServerOptions::default()
        });
        let (cookie, csrf) = setup_and_login(&app).await;
        for (mode, expected) in [
            ("no-login", StatusCode::UNAUTHORIZED),
            ("no-csrf", StatusCode::FORBIDDEN),
            ("cross-origin", StatusCode::FORBIDDEN),
            ("missing-origin", StatusCode::FORBIDDEN),
        ] {
            let mut request = Request::builder()
                .method(Method::POST)
                .uri("/api/download/file")
                .header(header::HOST, "localhost")
                .header(
                    header::CONTENT_TYPE,
                    "multipart/form-data; boundary=hardened",
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
            assert_eq!(response.status(), expected, "{mode}");
        }
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
            update_activation_socket: Some(root.path().join("activation.sock")),
            update_writes_enabled: true,
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
            "read_write"
        );
    }

    #[tokio::test]
    async fn logout_upload_returns_unauthorized_over_real_http1() {
        use tokio::net::{TcpListener, TcpStream};
        use tokio::time::{Duration, timeout};

        timeout(Duration::from_secs(10), async {
            let root = tempdir().unwrap();
            let socket = root.path().join("upload.sock");
            let worker = UnixListener::bind(&socket).unwrap();
            let app = app_with_options(ServerOptions {
                state_dir: root.path().join("account"),
                update_status_socket: Some(root.path().join("status.sock")),
                update_upload_socket: Some(socket),
                update_activation_socket: Some(root.path().join("activation.sock")),
                update_writes_enabled: true,
                ..ServerOptions::default()
            });
            let (cookie, csrf) = setup_and_login(&app).await;
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                axum::serve(
                    listener,
                    app.into_make_service_with_connect_info::<SocketAddr>(),
                )
                .await
                .unwrap();
            });
            let mut upload = TcpStream::connect(address).await.unwrap();
            upload.write_all(format!(
                "POST /api/application-update/upload HTTP/1.1\r\nHost: localhost\r\nOrigin: https://localhost\r\nCookie: {cookie}\r\nx-csrf-token: {csrf}\r\nContent-Type: application/octet-stream\r\nContent-Length: 32768\r\nConnection: close\r\n\r\na"
            ).as_bytes()).await.unwrap();
            let (mut ipc, _) = worker.accept().await.unwrap();
            let length = ipc.read_u32().await.unwrap() as usize;
            assert!(length < 65536);
            ipc.read_exact(&mut vec![0; length]).await.unwrap();
            let mut first = [0; 1];
            ipc.read_exact(&mut first).await.unwrap();
            assert_eq!(first, [b'a']);

            let mut logout = TcpStream::connect(address).await.unwrap();
            logout.write_all(format!(
                "POST /api/auth/logout HTTP/1.1\r\nHost: localhost\r\nOrigin: https://localhost\r\nCookie: {cookie}\r\nx-csrf-token: {csrf}\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
            ).as_bytes()).await.unwrap();
            let mut logout_response = Vec::new();
            logout.read_to_end(&mut logout_response).await.unwrap();
            assert!(logout_response.starts_with(b"HTTP/1.1 200"));
            // Bytes sent after logout must not be delivered to the worker.
            let _ = upload.write_all(b"not-for-the-worker").await;
            let mut upload_response = Vec::new();
            let read = upload.read_to_end(&mut upload_response).await;
            server.abort();
            assert!(
                upload_response.starts_with(b"HTTP/1.1 401"),
                "missing HTTP 401, transport={:?}, response_bytes={}",
                read.err().map(|error| error.kind()),
                upload_response.len()
            );
            let mut remaining = Vec::new();
            ipc.read_to_end(&mut remaining).await.unwrap();
            assert!(remaining.is_empty());
        })
        .await
        .expect("bounded real HTTP logout test timed out");
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
            update_activation_socket: Some(root.path().join("activation.sock")),
            update_writes_enabled: true,
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
            json_body(app.clone().oneshot(logout).await.unwrap()).await["code"],
            0
        );
        // Native cancellation must not wait for the HTTP discard deadline.
        let mut bytes = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            stream.read_to_end(&mut bytes),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(bytes.is_empty());
        assert!(
            !transfer.is_finished(),
            "stalled HTTP discard should still hold admission"
        );
        let login = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/api/auth/login",
                serde_json::json!({"username": "operator", "password": "initial password"}),
            ))
            .await
            .unwrap();
        let next_cookie = login.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let next_csrf = json_body(login).await["data"]["csrfToken"]
            .as_str()
            .unwrap()
            .to_owned();
        let next_upload = || {
            Request::builder()
                .method(Method::POST)
                .uri("/api/application-update/upload")
                .header(header::HOST, "localhost")
                .header(header::ORIGIN, "https://localhost")
                .header(header::COOKIE, &next_cookie)
                .header("x-csrf-token", &next_csrf)
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .header(header::CONTENT_LENGTH, "3")
                .body(Body::from("abc"))
                .unwrap()
        };
        assert_eq!(
            json_body(app.clone().oneshot(next_upload()).await.unwrap()).await["code"],
            -409
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), listener.accept())
                .await
                .is_err()
        );
        let response = tokio::time::timeout(std::time::Duration::from_secs(6), transfer)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let retry = tokio::spawn(app.oneshot(next_upload()));
        let admitted =
            tokio::time::timeout(std::time::Duration::from_secs(1), listener.accept()).await;
        retry.abort();
        assert!(
            admitted.is_ok(),
            "HTTP admission must be released after discard"
        );
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

    #[cfg(feature = "restricted-control-surface")]
    #[tokio::test]
    async fn firewall_modes_bind_identity_and_preserve_refusals() {
        let root = tempdir().unwrap();
        let socket = root.path().join("firewall.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let broker = tokio::spawn(async move {
            let mut identity = None;
            for index in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut prefix = [0; 4];
                stream.read_exact(&mut prefix).await.unwrap();
                let mut bytes = vec![0; u32::from_be_bytes(prefix) as usize];
                stream.read_exact(&mut bytes).await.unwrap();
                let envelope = bli_kvm_control_protocol::decode_request_v2(&bytes).unwrap();
                match envelope.request {
                    bli_kvm_core::PlatformControlRequestV2::FirewallModeChange { request } => {
                        assert_eq!(index, 0);
                        assert_eq!(request.mode, bli_kvm_core::FirewallMode::Restricted);
                        assert_ne!(request.operation_id, [0; 16]);
                        assert_ne!(request.session_binding, [0; 32]);
                        identity = Some((request.operation_id, request.session_binding));
                    }
                    bli_kvm_core::PlatformControlRequestV2::FirewallModeConfirm { request } => {
                        assert_eq!(index, 1);
                        assert_eq!(
                            Some((request.operation_id, request.session_binding)),
                            identity
                        );
                    }
                    _ => panic!("unexpected firewall request"),
                }
                let response = bli_kvm_control_protocol::ControlResponseEnvelope {
                    schema_version: 2,
                    request_id: envelope.request_id,
                    operation_id: envelope.operation_id,
                    response: if index == 0 {
                        bli_kvm_control_protocol::ControlResponse::Applied
                    } else {
                        bli_kvm_control_protocol::ControlResponse::Rejected {
                            code:
                                bli_kvm_control_protocol::ControlRejectionCode::PendingUnreconciled,
                        }
                    },
                };
                let bytes = bli_kvm_control_protocol::encode_response_v2(&response).unwrap();
                stream
                    .write_all(&bli_kvm_control_protocol::encode_frame(&bytes).unwrap())
                    .await
                    .unwrap();
            }
        });
        let app = app_with_options(ServerOptions {
            state_dir: root.path().join("account"),
            control_socket: Some(socket),
            firewall_modes_enabled: true,
            restricted_controls_enabled: true,
            ..ServerOptions::default()
        });
        let anonymous = app
            .clone()
            .oneshot(json_request(
                Method::GET,
                "/api/system/firewall/mode",
                Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
        let manifest = json_body(
            app.clone()
                .oneshot(json_request(Method::GET, "/api/capabilities", Value::Null))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            manifest["data"]["board"]["features"]["system_firewall_modes"]["availability"],
            "read_write"
        );
        let (cookie, csrf) = setup_and_login(&app).await;
        let post = |path, body| {
            let mut request = json_request(Method::POST, path, body);
            request
                .headers_mut()
                .insert(header::COOKIE, cookie.parse().unwrap());
            request
                .headers_mut()
                .insert("x-csrf-token", csrf.parse().unwrap());
            request
        };
        let denied = app
            .clone()
            .oneshot(post(
                "/api/system/firewall/mode",
                serde_json::json!({"mode":"restricted"}),
            ))
            .await
            .unwrap();
        assert_ne!(json_body(denied).await["code"], 0);
        let mut missing_csrf = post(
            "/api/system/firewall/mode",
            serde_json::json!({"mode":"restricted","confirm":true}),
        );
        missing_csrf.headers_mut().remove("x-csrf-token");
        assert_eq!(
            app.clone().oneshot(missing_csrf).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
        let changed = json_body(
            app.clone()
                .oneshot(post(
                    "/api/system/firewall/mode",
                    serde_json::json!({"mode":"restricted","confirm":true}),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(changed["data"]["applied"], true);
        let confirmed = json_body(
            app.oneshot(post(
                "/api/system/firewall/mode/confirm",
                serde_json::json!({"operationId":changed["data"]["operationId"]}),
            ))
            .await
            .unwrap(),
        )
        .await;
        assert_eq!(confirmed["data"]["applied"], false);
        assert!(confirmed["data"]["rejection"].is_string());
        tokio::time::timeout(std::time::Duration::from_secs(2), broker)
            .await
            .unwrap()
            .unwrap();
    }

    #[cfg(feature = "restricted-control-surface")]
    #[tokio::test]
    async fn wifi_routes_reach_the_owner_with_typed_requests_only() {
        use bli_kvm_control_protocol::{ControlResponse, WifiNetwork, WifiProfileView};
        use bli_kvm_core::{PlatformControlRequestV2, WifiProfileAction};
        const PROFILE: &str = "0123abcd-0123-4567-89ab-0123456789ab";
        let root = tempdir().unwrap();
        let socket = root.path().join("wifi.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let broker = tokio::spawn(async move {
            let mut seen = Vec::new();
            for _ in 0..7 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut prefix = [0; 4];
                stream.read_exact(&mut prefix).await.unwrap();
                let mut bytes = vec![0; u32::from_be_bytes(prefix) as usize];
                stream.read_exact(&mut bytes).await.unwrap();
                let envelope = bli_kvm_control_protocol::decode_request_v2(&bytes).unwrap();
                let response = match &envelope.request {
                    PlatformControlRequestV2::WifiScan { rescan: true } => {
                        ControlResponse::WifiNetworks {
                            networks: vec![WifiNetwork {
                                ssid: "HomeNet".into(),
                                signal: 81,
                                security: "wpa2".into(),
                                channel: 36,
                                in_use: true,
                            }],
                        }
                    }
                    PlatformControlRequestV2::WifiSaved {} => ControlResponse::WifiProfiles {
                        profiles: vec![WifiProfileView {
                            id: PROFILE.into(),
                            ssid: "HomeNet".into(),
                            autoconnect: true,
                            active: true,
                        }],
                    },
                    PlatformControlRequestV2::WifiConnect { .. } => ControlResponse::Rejected {
                        code: bli_kvm_control_protocol::ControlRejectionCode::BackendRejected,
                    },
                    _ => ControlResponse::Applied,
                };
                seen.push(envelope.request);
                let response = bli_kvm_control_protocol::ControlResponseEnvelope {
                    schema_version: 2,
                    request_id: envelope.request_id,
                    operation_id: envelope.operation_id,
                    response,
                };
                let bytes = bli_kvm_control_protocol::encode_response_v2(&response).unwrap();
                stream
                    .write_all(&bli_kvm_control_protocol::encode_frame(&bytes).unwrap())
                    .await
                    .unwrap();
            }
            seen
        });
        let app = app_with_options(ServerOptions {
            state_dir: root.path().join("account"),
            control_socket: Some(socket),
            restricted_controls_enabled: true,
            ..ServerOptions::default()
        });
        let (cookie, csrf) = setup_and_login(&app).await;
        let call = |method: Method, path: &str, body: Value| {
            let mut request = json_request(method, path, body);
            request
                .headers_mut()
                .insert(header::COOKIE, cookie.parse().unwrap());
            request
                .headers_mut()
                .insert("x-csrf-token", csrf.parse().unwrap());
            request
        };
        let scan = json_body(
            app.clone()
                .oneshot(call(Method::POST, "/api/network/wifi/scan", Value::Null))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            scan["data"]["networks"][0],
            serde_json::json!({"ssid":"HomeNet","signal":81,"security":"wpa2","channel":36,"inUse":true})
        );
        let saved = json_body(
            app.clone()
                .oneshot(call(Method::GET, "/api/network/wifi/saved", Value::Null))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(saved["data"]["profiles"][0]["id"], PROFILE);
        // Invalid input never reaches the owner.
        for (path, body) in [
            (
                "/api/network/wifi/connect",
                serde_json::json!({"ssid":"HomeNet","password":"short"}),
            ),
            (
                "/api/network/wifi/connect",
                serde_json::json!({"ssid":"","password":""}),
            ),
            (
                "/api/network/wifi/saved",
                serde_json::json!({"id":"../x","action":"forget"}),
            ),
        ] {
            let response = app
                .clone()
                .oneshot(call(Method::POST, path, body))
                .await
                .unwrap();
            assert_ne!(json_body(response).await["code"], 0, "{path}");
        }
        let connect = json_body(
            app.clone()
                .oneshot(call(
                    Method::POST,
                    "/api/network/wifi/connect",
                    serde_json::json!({"ssid":"Hidden","password":"eight888","hidden":true}),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            connect["data"],
            serde_json::json!({"applied":false,"rejection":"backend_rejected"})
        );
        for (path, body) in [
            (
                "/api/network/wifi/saved",
                serde_json::json!({"id":PROFILE,"action":"autoconnect_off"}),
            ),
            (
                "/api/network/wifi/radio",
                serde_json::json!({"enabled":false}),
            ),
            (
                "/api/network/wifi/preferred",
                serde_json::json!({"enabled":true}),
            ),
            ("/api/network/wifi/preferred/confirm", Value::Null),
        ] {
            let response = app
                .clone()
                .oneshot(call(Method::POST, path, body))
                .await
                .unwrap();
            assert_eq!(json_body(response).await["data"]["applied"], true, "{path}");
        }
        let seen = tokio::time::timeout(std::time::Duration::from_secs(2), broker)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            seen,
            [
                PlatformControlRequestV2::WifiScan { rescan: true },
                PlatformControlRequestV2::WifiSaved {},
                PlatformControlRequestV2::WifiConnect {
                    ssid: "Hidden".into(),
                    password: "eight888".into(),
                    hidden: true,
                },
                PlatformControlRequestV2::WifiProfile {
                    id: PROFILE.into(),
                    action: WifiProfileAction::AutoconnectOff,
                },
                PlatformControlRequestV2::WifiRadio { enabled: false },
                PlatformControlRequestV2::WifiPreferred { enabled: true },
                PlatformControlRequestV2::WifiPreferredConfirm {},
            ]
        );

        // Without the operator's opt-in the routes answer unavailable.
        let closed = app_with_options(ServerOptions {
            state_dir: root.path().join("closed"),
            control_socket: Some(root.path().join("absent.sock")),
            ..ServerOptions::default()
        });
        let (cookie, _) = setup_and_login(&closed).await;
        let mut request = json_request(Method::GET, "/api/network/wifi/scan", Value::Null);
        request
            .headers_mut()
            .insert(header::COOKIE, cookie.parse().unwrap());
        let response = closed.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    /// An admin helper that answers root password requests with `replies`
    /// in turn and hands back the passwords it was asked to set.
    fn recording_admin_helper(
        listener: tokio::net::UnixListener,
        replies: Vec<&'static str>,
    ) -> tokio::task::JoinHandle<Vec<String>> {
        use tokio::io::AsyncBufReadExt;
        tokio::spawn(async move {
            let mut passwords = Vec::new();
            for reply in replies {
                let (stream, _) = listener.accept().await.unwrap();
                let mut reader = tokio::io::BufReader::new(stream);
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                let request: bli_kvm_admin_helper::AdminRequest =
                    serde_json::from_str(&line).unwrap();
                let bli_kvm_admin_helper::AdminRequest::RootPassword { password } = request else {
                    panic!("unexpected helper request");
                };
                passwords.push(password);
                let mut stream = reader.into_inner();
                stream
                    .write_all(format!("{reply}\n").as_bytes())
                    .await
                    .unwrap();
            }
            passwords
        })
    }

    fn password_change(cookie: &str, csrf: &str, body: Value) -> Request<Body> {
        let mut change = json_request(Method::POST, "/api/auth/password", body);
        change
            .headers_mut()
            .insert(header::COOKIE, cookie.parse().unwrap());
        change
            .headers_mut()
            .insert("x-csrf-token", csrf.parse().unwrap());
        change
    }

    async fn logs_in_with(app: &Router, password: &str) -> bool {
        let login = json_body(
            app.clone()
                .oneshot(json_request(
                    Method::POST,
                    "/api/auth/login",
                    serde_json::json!({ "username": "operator", "password": password }),
                ))
                .await
                .unwrap(),
        )
        .await;
        login["code"] == 0
    }

    #[tokio::test]
    async fn the_password_changes_only_with_the_current_password() {
        let root = tempdir().unwrap();
        let socket = root.path().join("admin.sock");
        let helper = recording_admin_helper(
            tokio::net::UnixListener::bind(&socket).unwrap(),
            vec![r#"{"status":"done"}"#, r#"{"status":"done"}"#],
        );
        let app = app_with_options(ServerOptions {
            state_dir: root.path().join("state"),
            admin_socket: Some(socket),
            ..ServerOptions::default()
        });
        let (cookie, csrf) = setup_and_login(&app).await;
        for body in [
            serde_json::json!({ "username": "operator", "password": "second password" }),
            serde_json::json!({
                "username": "operator",
                "password": "second password",
                "currentPassword": "not the password"
            }),
        ] {
            let response = app
                .clone()
                .oneshot(password_change(&cookie, &csrf, body))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        }
        assert!(logs_in_with(&app, "initial password").await);
        let changed = json_body(
            app.clone()
                .oneshot(password_change(
                    &cookie,
                    &csrf,
                    serde_json::json!({
                        "username": "operator",
                        "password": "second password",
                        "currentPassword": "initial password"
                    }),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(changed["code"], 0);
        assert!(logs_in_with(&app, "second password").await);
        let passwords = tokio::time::timeout(std::time::Duration::from_secs(5), helper)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(passwords, ["initial password", "second password"]);
    }

    #[tokio::test]
    async fn a_root_password_the_helper_refuses_puts_the_account_back() {
        let root = tempdir().unwrap();
        let socket = root.path().join("admin.sock");
        let helper = recording_admin_helper(
            tokio::net::UnixListener::bind(&socket).unwrap(),
            vec![
                r#"{"status":"done"}"#,
                r#"{"status":"refused","reason":"fixture"}"#,
            ],
        );
        let app = app_with_options(ServerOptions {
            state_dir: root.path().join("state"),
            admin_socket: Some(socket),
            ..ServerOptions::default()
        });
        let (cookie, csrf) = setup_and_login(&app).await;
        let refused = json_body(
            app.clone()
                .oneshot(password_change(
                    &cookie,
                    &csrf,
                    serde_json::json!({
                        "username": "operator",
                        "password": "second password",
                        "currentPassword": "initial password"
                    }),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_ne!(refused["code"], 0);
        let passwords = tokio::time::timeout(std::time::Duration::from_secs(5), helper)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(passwords, ["initial password", "second password"]);
        assert!(logs_in_with(&app, "initial password").await);
        assert!(!logs_in_with(&app, "second password").await);
    }

    #[tokio::test]
    async fn an_oversized_hid_message_closes_the_socket() {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::{
            Message as WsMessage, client::IntoClientRequest, http::HeaderValue,
        };
        let root = tempdir().unwrap();
        let app = app_with_options(ServerOptions {
            state_dir: root.path().join("state"),
            control_socket: Some(root.path().join("absent.sock")),
            ..ServerOptions::default()
        });
        let (cookie, _) = setup_and_login(&app).await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });
        let mut request = format!("ws://{address}/api/ws")
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert("cookie", cookie.parse().unwrap());
        request
            .headers_mut()
            .insert("origin", HeaderValue::from_static("http://kvm.test"));
        request
            .headers_mut()
            .insert("host", HeaderValue::from_static("kvm.test"));
        let stream = tokio::net::TcpStream::connect(address).await.unwrap();
        let (mut browser, _) = tokio_tungstenite::client_async(request, stream)
            .await
            .unwrap();
        // A HID frame is at most 16 bytes; a kilobyte is not buffered and
        // answered, it ends the socket.
        browser
            .send(WsMessage::Binary(vec![1_u8; 1024].into()))
            .await
            .unwrap();
        let next = tokio::time::timeout(std::time::Duration::from_secs(5), browser.next())
            .await
            .unwrap();
        assert!(
            !matches!(next, Some(Ok(WsMessage::Text(_)))),
            "the socket answered instead of closing: {next:?}"
        );
    }

    #[tokio::test]
    async fn first_setup_and_password_changes_set_root_through_the_helper() {
        use tokio::io::AsyncBufReadExt;
        let root = tempdir().unwrap();
        let socket = root.path().join("admin.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        // The helper refuses the first request and accepts the others.
        let helper = tokio::spawn(async move {
            let mut passwords = Vec::new();
            for index in 0..3 {
                let (stream, _) = listener.accept().await.unwrap();
                let mut reader = tokio::io::BufReader::new(stream);
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                let request: bli_kvm_admin_helper::AdminRequest =
                    serde_json::from_str(&line).unwrap();
                let bli_kvm_admin_helper::AdminRequest::RootPassword { password } = request else {
                    panic!("unexpected helper request");
                };
                passwords.push(password);
                let reply = if index == 0 {
                    r#"{"status":"refused","reason":"fixture"}"#
                } else {
                    r#"{"status":"done"}"#
                };
                let mut stream = reader.into_inner();
                stream
                    .write_all(format!("{reply}\n").as_bytes())
                    .await
                    .unwrap();
            }
            passwords
        });
        let app = app_with_options(ServerOptions {
            state_dir: root.path().join("state"),
            admin_socket: Some(socket),
            ..ServerOptions::default()
        });
        let setup = |password: &str| {
            json_request(
                Method::POST,
                "/api/auth/setup",
                serde_json::json!({ "username": "operator", "password": password }),
            )
        };
        // Refused by the helper: no account is stored.
        let refused = json_body(
            app.clone()
                .oneshot(setup("initial password"))
                .await
                .unwrap(),
        )
        .await;
        assert_ne!(refused["code"], 0);
        let state = json_body(
            app.clone()
                .oneshot(json_request(Method::GET, "/api/auth/setup", Value::Null))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(state["data"]["required"], true);
        // A password root cannot take is refused before the helper is asked.
        let broken = json_body(app.clone().oneshot(setup("two\nlines!")).await.unwrap()).await;
        assert_ne!(broken["code"], 0);

        let (cookie, csrf) = setup_and_login(&app).await;
        let mut change = json_request(
            Method::POST,
            "/api/auth/password",
            serde_json::json!({
                "username": "operator",
                "password": "second password",
                "currentPassword": "initial password"
            }),
        );
        change
            .headers_mut()
            .insert(header::COOKIE, cookie.parse().unwrap());
        change
            .headers_mut()
            .insert("x-csrf-token", csrf.parse().unwrap());
        let changed = json_body(app.clone().oneshot(change).await.unwrap()).await;
        assert_eq!(changed["code"], 0);
        let passwords = tokio::time::timeout(std::time::Duration::from_secs(5), helper)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            passwords,
            ["initial password", "initial password", "second password"]
        );
        let login = json_body(
            app.oneshot(json_request(
                Method::POST,
                "/api/auth/login",
                serde_json::json!({ "username": "operator", "password": "second password" }),
            ))
            .await
            .unwrap(),
        )
        .await;
        assert_eq!(login["code"], 0);
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
    async fn hostname_requires_an_authenticated_session() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/api/hostname")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn system_time_requires_an_authenticated_session() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/api/system/time")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    /// A board owner that reports `file` as the attached image to every
    /// request, for the media routes that ask it first.
    fn serve_media_state(
        socket: &std::path::Path,
        file: Option<&str>,
    ) -> tokio::task::JoinHandle<()> {
        let listener = UnixListener::bind(socket).unwrap();
        let file = file.map(str::to_string);
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let mut prefix = [0; 4];
                if stream.read_exact(&mut prefix).await.is_err() {
                    continue;
                }
                let mut bytes = vec![0; u32::from_be_bytes(prefix) as usize];
                if stream.read_exact(&mut bytes).await.is_err() {
                    continue;
                }
                let envelope = bli_kvm_control_protocol::decode_request_v2(&bytes).unwrap();
                let response = bli_kvm_control_protocol::ControlResponseEnvelope {
                    schema_version: 2,
                    request_id: envelope.request_id,
                    operation_id: envelope.operation_id,
                    response: bli_kvm_control_protocol::ControlResponse::MediaState {
                        file: file.clone(),
                        connected: file.is_some(),
                        cdrom: false,
                        writable: false,
                    },
                };
                let bytes = bli_kvm_control_protocol::encode_response_v2(&response).unwrap();
                let _ = stream
                    .write_all(&bli_kvm_control_protocol::encode_frame(&bytes).unwrap())
                    .await;
            }
        })
    }

    #[tokio::test]
    async fn an_empty_upload_leaves_the_image_of_its_name() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().unwrap();
        let images = directory.path().join("images");
        fs::create_dir_all(&images).unwrap();
        fs::set_permissions(&images, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(images.join("kubuntu.img"), b"the operator's image").unwrap();
        let catalog = directory.path().join("catalog.json");
        fs::write(&catalog, r#"{"schemaVersion":1,"entries":[]}"#).unwrap();
        fs::set_permissions(&catalog, fs::Permissions::from_mode(0o644)).unwrap();
        let owner = serve_media_state(&directory.path().join("control.sock"), None);
        let app = app_with_options(ServerOptions {
            state_dir: directory.path().join("state"),
            restricted_controls_enabled: true,
            media_controls_enabled: true,
            media_catalog: Some(catalog.clone()),
            media_directory: Some(images.clone()),
            control_socket: Some(directory.path().join("control.sock")),
            ..ServerOptions::default()
        });
        let (cookie, csrf) = setup_and_login(&app).await;
        let boundary = "----blikvm-upload";
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"kubuntu.img\"\r\nContent-Type: application/octet-stream\r\n\r\n\r\n--{boundary}--\r\n"
        )
        .into_bytes();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/download/file")
                    .header(header::HOST, "kvm.test")
                    .header(header::ORIGIN, "https://kvm.test")
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .header(header::CONTENT_LENGTH, body.len())
                    .header(header::COOKIE, cookie)
                    .header("x-csrf-token", csrf)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(json_body(response).await["code"], 0);
        assert_eq!(
            fs::read(images.join("kubuntu.img")).unwrap(),
            b"the operator's image"
        );
        assert_eq!(
            fs::read_dir(&images).unwrap().count(),
            1,
            "a temporary file was left"
        );
        owner.abort();
    }

    #[tokio::test]
    async fn an_upload_refused_after_its_file_leaves_the_image_of_its_name() {
        // A second file part is refused after the first has arrived; the
        // first must not have replaced the image already (code audit of
        // 2026-10-20, AUD-19.2).
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().unwrap();
        let images = directory.path().join("images");
        fs::create_dir_all(&images).unwrap();
        fs::set_permissions(&images, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(images.join("kubuntu.img"), b"the operator's image").unwrap();
        let catalog = directory.path().join("catalog.json");
        fs::write(&catalog, r#"{"schemaVersion":1,"entries":[]}"#).unwrap();
        fs::set_permissions(&catalog, fs::Permissions::from_mode(0o644)).unwrap();
        let owner = serve_media_state(&directory.path().join("control.sock"), None);
        let app = app_with_options(ServerOptions {
            state_dir: directory.path().join("state"),
            restricted_controls_enabled: true,
            media_controls_enabled: true,
            media_catalog: Some(catalog.clone()),
            media_directory: Some(images.clone()),
            control_socket: Some(directory.path().join("control.sock")),
            ..ServerOptions::default()
        });
        let (cookie, csrf) = setup_and_login(&app).await;
        let boundary = "----blikvm-upload";
        let part = |name: &str, content: &str| {
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n{content}\r\n"
            )
        };
        let body = format!(
            "{}{}--{boundary}--\r\n",
            part("kubuntu.img", "a replacement"),
            part("other.img", "a second file")
        )
        .into_bytes();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/download/file")
                    .header(header::HOST, "kvm.test")
                    .header(header::ORIGIN, "https://kvm.test")
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .header(header::CONTENT_LENGTH, body.len())
                    .header(header::COOKIE, cookie)
                    .header("x-csrf-token", csrf)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(json_body(response).await["code"], 0);
        assert_eq!(
            fs::read(images.join("kubuntu.img")).unwrap(),
            b"the operator's image"
        );
        assert_eq!(
            fs::read_dir(&images).unwrap().count(),
            1,
            "a temporary file was left"
        );
        owner.abort();
    }

    #[tokio::test]
    async fn authenticated_image_upload_writes_the_file_and_the_catalog() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().unwrap();
        let images = directory.path().join("images");
        fs::create_dir_all(&images).unwrap();
        fs::set_permissions(&images, fs::Permissions::from_mode(0o755)).unwrap();
        let catalog = directory.path().join("catalog.json");
        fs::write(&catalog, r#"{"schemaVersion":1,"entries":[]}"#).unwrap();
        fs::set_permissions(&catalog, fs::Permissions::from_mode(0o644)).unwrap();
        let owner = serve_media_state(&directory.path().join("control.sock"), None);
        let app = app_with_options(ServerOptions {
            state_dir: directory.path().join("state"),
            restricted_controls_enabled: true,
            media_controls_enabled: true,
            media_catalog: Some(catalog.clone()),
            media_directory: Some(images.clone()),
            control_socket: Some(directory.path().join("control.sock")),
            ..ServerOptions::default()
        });
        let (cookie, csrf) = setup_and_login(&app).await;

        let boundary = "----blikvm-upload";
        let mut body = Vec::new();
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"kubuntu.img\"\r\nContent-Type: application/octet-stream\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(b"disk-image-bytes");
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/download/file")
                    .header(header::HOST, "kvm.test")
                    .header(header::ORIGIN, "https://kvm.test")
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .header(header::CONTENT_LENGTH, body.len())
                    .header(header::COOKIE, cookie)
                    .header("x-csrf-token", csrf)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let json = json_body(response).await;
        assert_eq!(json["data"]["imageName"], "kubuntu.img");
        assert_eq!(json["data"]["size"], 16);
        assert_eq!(
            fs::read(images.join("kubuntu.img")).unwrap(),
            b"disk-image-bytes"
        );
        assert_eq!(
            fs::metadata(images.join("kubuntu.img"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o644
        );
        let summaries = bli_kvm_media_catalog::MediaCatalog::open(&catalog)
            .unwrap()
            .summaries();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].image_name, "kubuntu.img");
        assert!(summaries[0].removable_allowed);
        assert!(!summaries[0].cdrom_allowed);
        owner.abort();
        #[cfg(feature = "restricted-control-surface")]
        attached_image_is_kept_and_stale_entries_dropped(&directory, &images, &catalog, boundary)
            .await;
    }

    #[tokio::test]
    async fn a_refused_upload_is_read_to_its_end_before_the_answer() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let directory = tempdir().unwrap();
        let images = directory.path().join("images");
        fs::create_dir_all(&images).unwrap();
        let catalog = directory.path().join("catalog.json");
        fs::write(&catalog, r#"{"schemaVersion":1,"entries":[]}"#).unwrap();
        let owner = serve_media_state(&directory.path().join("control.sock"), None);
        let app = app_with_options(ServerOptions {
            state_dir: directory.path().join("state"),
            restricted_controls_enabled: true,
            media_controls_enabled: true,
            media_catalog: Some(catalog),
            media_directory: Some(images.clone()),
            control_socket: Some(directory.path().join("control.sock")),
            ..ServerOptions::default()
        });
        let (cookie, csrf) = setup_and_login(&app).await;

        let boundary = "----blikvm-upload";
        let mut chunks = vec![bytes::Bytes::from(format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"name with space.iso\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        ))];
        chunks.extend(std::iter::repeat_n(
            bytes::Bytes::from(vec![0_u8; 64 * 1024]),
            64,
        ));
        chunks.push(bytes::Bytes::from(format!("\r\n--{boundary}--\r\n")));
        let length: usize = chunks.iter().map(bytes::Bytes::len).sum();
        let (total, read) = (chunks.len(), Arc::new(AtomicUsize::new(0)));
        let counter = read.clone();
        // Each chunk arrives after a pause, as over a connection; a stream
        // that is always ready would be buffered whole by the parser.
        let stream = futures_util::stream::unfold(chunks.into_iter(), move |mut chunks| {
            let counter = counter.clone();
            async move {
                tokio::task::yield_now().await;
                let chunk = chunks.next()?;
                counter.fetch_add(1, Ordering::SeqCst);
                Some((Ok::<_, std::io::Error>(chunk), chunks))
            }
        });
        let answer = json_body(
            app.oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/download/file")
                    .header(header::HOST, "kvm.test")
                    .header(header::ORIGIN, "https://kvm.test")
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .header(header::CONTENT_LENGTH, length)
                    .header(header::COOKIE, cookie)
                    .header("x-csrf-token", csrf)
                    .body(Body::from_stream(stream))
                    .unwrap(),
            )
            .await
            .unwrap(),
        )
        .await;
        assert_eq!(answer["msg"], "image name is invalid");
        // Behind nginx the answer is read only after the whole body is sent.
        assert_eq!(read.load(Ordering::SeqCst), total);
        assert_eq!(fs::read_dir(&images).unwrap().count(), 0);
        owner.abort();
    }

    #[cfg(feature = "restricted-control-surface")]
    async fn attached_image_is_kept_and_stale_entries_dropped(
        directory: &tempfile::TempDir,
        images: &std::path::Path,
        catalog: &std::path::Path,
        boundary: &str,
    ) {
        let (images, catalog) = (images.to_path_buf(), catalog.to_path_buf());
        // With the same image attached, replacing it is refused.
        let attached_socket = directory.path().join("attached.sock");
        let attached_owner = serve_media_state(&attached_socket, Some("kubuntu.img"));
        let attached = app_with_options(ServerOptions {
            state_dir: directory.path().join("state-attached"),
            restricted_controls_enabled: true,
            media_controls_enabled: true,
            media_catalog: Some(catalog.clone()),
            media_directory: Some(images.clone()),
            control_socket: Some(attached_socket),
            ..ServerOptions::default()
        });
        let (cookie, csrf) = setup_and_login(&attached).await;
        let mut body = Vec::new();
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"kubuntu.img\"\r\nContent-Type: application/octet-stream\r\n\r\nreplacement\r\n--{boundary}--\r\n"
            )
            .as_bytes(),
        );
        let refused = json_body(
            attached
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/api/download/file")
                        .header(header::HOST, "kvm.test")
                        .header(header::ORIGIN, "https://kvm.test")
                        .header(
                            header::CONTENT_TYPE,
                            format!("multipart/form-data; boundary={boundary}"),
                        )
                        .header(header::CONTENT_LENGTH, body.len())
                        .header(header::COOKIE, &cookie)
                        .header("x-csrf-token", &csrf)
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(refused["code"], -409);
        assert_eq!(
            fs::read(images.join("kubuntu.img")).unwrap(),
            b"disk-image-bytes"
        );
        // The check before sending answers the same refusals.
        for (name, size, code, msg) in [
            (
                "kubuntu.img",
                11_u64,
                -409,
                "detach the drive before replacing its image",
            ),
            ("name with space.iso", 11, -1, "image name is invalid"),
            (
                "big.iso",
                65 * 1024 * 1024 * 1024,
                -1,
                "upload is too large",
            ),
            ("big.iso", 0, -1, "the file is empty"),
            (
                "FreeBSD-15.0-PRERELEASE-amd64-20250822-027be99b1f33-279771-disc1.iso",
                11,
                0,
                "success",
            ),
        ] {
            let answer = json_body(
                attached
                    .clone()
                    .oneshot(
                        Request::builder()
                            .method(Method::POST)
                            .uri("/api/download/file/check")
                            .header(header::HOST, "kvm.test")
                            .header(header::ORIGIN, "https://kvm.test")
                            .header(header::CONTENT_TYPE, "application/json")
                            .header(header::COOKIE, &cookie)
                            .header("x-csrf-token", &csrf)
                            .body(Body::from(
                                serde_json::json!({ "name": name, "size": size }).to_string(),
                            ))
                            .unwrap(),
                    )
                    .await
                    .unwrap(),
            )
            .await;
            assert_eq!(answer["code"], code, "{name} {size}: {answer}");
            assert_eq!(answer["msg"], msg, "{name} {size}: {answer}");
        }
        attached_owner.abort();

        // A catalog entry whose file is gone is dropped when the catalog is read.
        fs::remove_file(images.join("kubuntu.img")).unwrap();
        let mut request = json_request(Method::GET, "/api/virtual-media/catalog", Value::Null);
        request
            .headers_mut()
            .insert(header::COOKIE, cookie.parse().unwrap());
        let listed = json_body(attached.oneshot(request).await.unwrap()).await;
        assert_eq!(listed["data"]["images"], serde_json::json!([]));
        assert!(
            bli_kvm_media_catalog::MediaCatalog::open(&catalog)
                .unwrap()
                .summaries()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn exec_and_extension_features_stay_unavailable_by_default() {
        let response = app()
            .oneshot(json_request(Method::GET, "/api/capabilities", Value::Null))
            .await
            .unwrap();
        let value = json_body(response).await;
        let features = value["data"]["board"]["features"]
            .as_object()
            .expect("board features");
        for name in [
            "terminal",
            "scripts",
            "picoclaw",
            "wake_on_lan",
            "fan",
            "oled",
            "physical_buttons",
            "serial_switch",
            "system_network",
            "system_firewall",
        ] {
            if let Some(status) = features.get(name) {
                assert_ne!(
                    status["availability"], "read_write",
                    "{name} must stay unavailable or read-only by default"
                );
            }
        }
        assert_ne!(
            value["data"]["virtualMedia"]["remoteDownload"]["availability"],
            "read_write"
        );
        assert_ne!(
            value["data"]["system"]["signedSystemUpdates"]["availability"],
            "read_write"
        );
    }

    #[tokio::test]
    async fn download_flags_require_an_authenticated_session() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/api/download/image/enabled")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn autostart_requires_an_authenticated_session() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/api/vm/autostart")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn vm_info_requires_an_authenticated_session() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/api/vm/info")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn application_version_requires_an_authenticated_session() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/api/application/version")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn system_log_requires_an_authenticated_session() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/api/system-log/messages?kind=system&lines=10")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn hostname_validation_is_bounded_and_conservative() {
        assert_eq!(validate_hostname("mangopimcore").unwrap(), "mangopimcore");
        assert_eq!(validate_hostname("kvm-1.local").unwrap(), "kvm-1.local");
        assert!(validate_hostname("").is_err());
        assert!(validate_hostname(&"a".repeat(254)).is_err());
        assert!(validate_hostname("bad host").is_err());
        assert!(validate_hostname("bad$host").is_err());
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
                    b"HTTP/1.1 200 OK\r\nContent-Type: multipart/x-mixed-replace; boundary=frame\r\nX-Private-Backend: forbidden\r\nContent-Length: 62\r\n\r\n--frame\r\nContent-Type: image/jpeg\r\nContent-Length: 4\r\n\r\nJPEG\r\n",
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
        // Whole parts are relayed as they came.
        assert_eq!(
            to_bytes(response.into_body(), 64 * 1024).await.unwrap(),
            "--frame\r\nContent-Type: image/jpeg\r\nContent-Length: 4\r\n\r\nJPEG\r\n"
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

    #[tokio::test]
    async fn hid_focus_confirmation_is_session_bound_and_denied_by_default() {
        let root = tempdir().unwrap();
        let control_socket = root.path().join("control.sock");
        let listener = UnixListener::bind(&control_socket).unwrap();
        let broker = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut prefix = [0_u8; 4];
            stream.read_exact(&mut prefix).await.unwrap();
            let mut payload = vec![0_u8; u32::from_be_bytes(prefix) as usize];
            stream.read_exact(&mut payload).await.unwrap();
            let request = bli_kvm_control_protocol::decode_request_v2(&payload).unwrap();
            let response = bli_kvm_control_protocol::ControlResponseEnvelope {
                schema_version: bli_kvm_control_protocol::CONTROL_SCHEMA_VERSION_V2,
                request_id: request.request_id,
                operation_id: request.operation_id,
                response: bli_kvm_control_protocol::ControlResponse::Rejected {
                    code: bli_kvm_control_protocol::ControlRejectionCode::CapabilityDisabled,
                },
            };
            let payload = bli_kvm_control_protocol::encode_response_v2(&response).unwrap();
            stream
                .write_all(&bli_kvm_control_protocol::encode_frame(&payload).unwrap())
                .await
                .unwrap();
        });
        let app = app_with_options(ServerOptions {
            state_dir: root.path().join("account"),
            secure_cookies: true,
            control_socket: Some(control_socket),
            ..ServerOptions::default()
        });

        let challenge_without_login = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/hid/focus")
                    .header(header::HOST, "kvm.test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(challenge_without_login.status(), StatusCode::UNAUTHORIZED);

        let (cookie, csrf) = setup_and_login(&app).await;
        let mut challenge_request = Request::builder()
            .method(Method::GET)
            .uri("/api/hid/focus")
            .header(header::HOST, "kvm.test")
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap();
        challenge_request.extensions_mut().insert(ConnectInfo(
            "192.0.2.10:4242".parse::<SocketAddr>().unwrap(),
        ));
        let challenge_body = json_body(app.clone().oneshot(challenge_request).await.unwrap()).await;
        let challenge = challenge_body["data"]["challenge"]
            .as_str()
            .unwrap_or_else(|| panic!("unexpected focus challenge body: {challenge_body}"))
            .to_owned();

        let mut confirm = json_request(
            Method::POST,
            "/api/hid/focus",
            serde_json::json!({
                "operationId": "0123456789abcdef0123456789abcdef",
                "challenge": challenge,
                "targetId": "11".repeat(16),
                "operatorConfirmed": true,
            }),
        );
        confirm
            .headers_mut()
            .insert(header::COOKIE, cookie.parse().unwrap());
        confirm
            .headers_mut()
            .insert("x-csrf-token", csrf.parse().unwrap());
        let confirm_body = json_body(app.clone().oneshot(confirm).await.unwrap()).await;
        assert_eq!(confirm_body["code"], 0);
        assert_eq!(confirm_body["data"]["attested"], false);
        assert_eq!(confirm_body["data"]["rejection"], "capability_disabled");
        broker.await.unwrap();
    }

    #[tokio::test]
    async fn hid_lease_acquire_returns_a_broker_lease_id() {
        exercise_hid_lease_teardown(false).await;
    }

    #[tokio::test]
    async fn shutdown_waits_for_live_websocket_hid_release() {
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            exercise_hid_lease_teardown(true),
        )
        .await
        .unwrap();
    }

    async fn exercise_hid_lease_teardown(websocket_shutdown: bool) {
        let root = tempdir().unwrap();
        let control_socket = root.path().join("control.sock");
        let listener = UnixListener::bind(&control_socket).unwrap();
        let report_seen = Arc::new(tokio::sync::Notify::new());
        let release_seen = Arc::new(tokio::sync::Notify::new());
        let allow_release = Arc::new(tokio::sync::Notify::new());
        let broker_report = report_seen.clone();
        let broker_release = release_seen.clone();
        let broker_allow = allow_release.clone();
        let broker = tokio::spawn(async move {
            let release_index = if websocket_shutdown { 3 } else { 2 };
            for index in 0..=release_index {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut prefix = [0_u8; 4];
                stream.read_exact(&mut prefix).await.unwrap();
                let mut payload = vec![0_u8; u32::from_be_bytes(prefix) as usize];
                stream.read_exact(&mut payload).await.unwrap();
                let request = bli_kvm_control_protocol::decode_request_v2(&payload).unwrap();
                if index == release_index {
                    assert!(matches!(
                        request.request,
                        bli_kvm_core::PlatformControlRequestV2::HidLease {
                            lease: bli_kvm_core::HidLeaseRequest::Release {
                                lease_id,
                                ..
                            }
                        } if lease_id == [0x55; 16]
                    ));
                    if websocket_shutdown {
                        broker_release.notify_one();
                        broker_allow.notified().await;
                    }
                }
                if websocket_shutdown && index == 2 {
                    assert!(matches!(request.request,
                        bli_kvm_core::PlatformControlRequestV2::HidLease {
                            lease: bli_kvm_core::HidLeaseRequest::Report {
                                lease_id,
                                report: bli_kvm_core::HidReport::Keyboard { data }, ..
                            }
                        } if lease_id == [0x55; 16] && data == [0, 0, 4, 0, 0, 0, 0, 0]
                    ));
                    broker_report.notify_one();
                }
                let response = match request.request {
                    bli_kvm_core::PlatformControlRequestV2::HidLease {
                        lease: bli_kvm_core::HidLeaseRequest::Acquire { .. },
                    } => bli_kvm_control_protocol::ControlResponse::LeaseGranted {
                        lease_id: [0x55; 16],
                    },
                    _ => bli_kvm_control_protocol::ControlResponse::Applied,
                };
                let envelope = bli_kvm_control_protocol::ControlResponseEnvelope {
                    schema_version: bli_kvm_control_protocol::CONTROL_SCHEMA_VERSION_V2,
                    request_id: request.request_id,
                    operation_id: request.operation_id,
                    response,
                };
                let payload = bli_kvm_control_protocol::encode_response_v2(&envelope).unwrap();
                stream
                    .write_all(&bli_kvm_control_protocol::encode_frame(&payload).unwrap())
                    .await
                    .unwrap();
            }
        });
        let shutdown = shutdown::Shutdown::default();
        let app = app_with_options(ServerOptions {
            shutdown: shutdown.clone(),
            state_dir: root.path().join("account"),
            secure_cookies: true,
            control_socket: Some(control_socket),
            ..ServerOptions::default()
        });
        let (cookie, csrf) = setup_and_login(&app).await;

        let mut challenge_request = Request::builder()
            .method(Method::GET)
            .uri("/api/hid/focus")
            .header(header::HOST, "kvm.test")
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap();
        challenge_request.extensions_mut().insert(ConnectInfo(
            "192.0.2.10:4242".parse::<SocketAddr>().unwrap(),
        ));
        let challenge_body = json_body(app.clone().oneshot(challenge_request).await.unwrap()).await;
        let challenge = challenge_body["data"]["challenge"]
            .as_str()
            .unwrap()
            .to_owned();

        let mut confirm = json_request(
            Method::POST,
            "/api/hid/focus",
            serde_json::json!({
                "operationId": "0123456789abcdef0123456789abcdef",
                "challenge": challenge,
                "targetId": "11".repeat(16),
                "operatorConfirmed": true,
            }),
        );
        confirm
            .headers_mut()
            .insert(header::COOKIE, cookie.parse().unwrap());
        confirm
            .headers_mut()
            .insert("x-csrf-token", csrf.parse().unwrap());
        let confirm_body = json_body(app.clone().oneshot(confirm).await.unwrap()).await;
        assert_eq!(confirm_body["data"]["attested"], true);

        let mut lease = json_request(
            Method::POST,
            "/api/hid/lease",
            serde_json::json!({
                "action": "acquire",
                "operationId": "fedcba9876543210fedcba9876543210",
                "ttlSeconds": 30
            }),
        );
        lease
            .headers_mut()
            .insert(header::COOKIE, cookie.parse().unwrap());
        lease
            .headers_mut()
            .insert("x-csrf-token", csrf.parse().unwrap());
        let lease_body = json_body(app.clone().oneshot(lease).await.unwrap()).await;
        assert_eq!(lease_body["code"], 0);
        assert_eq!(lease_body["data"]["applied"], true);
        assert_eq!(lease_body["data"]["leaseId"], "55".repeat(16));
        assert_eq!(lease_body["data"]["ttlSeconds"], 30);

        if websocket_shutdown {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let signal = shutdown.clone();
            let server = tokio::spawn(async move {
                axum::serve(
                    listener,
                    app.into_make_service_with_connect_info::<SocketAddr>(),
                )
                .with_graceful_shutdown(async move { signal.cancelled().await })
                .await
                .unwrap();
            });
            let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
            socket.write_all(format!(
                "GET /api/ws HTTP/1.1\r\nHost: kvm.test\r\nOrigin: https://kvm.test\r\nCookie: {cookie}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
            ).as_bytes()).await.unwrap();
            let mut headers = Vec::new();
            while !headers.ends_with(b"\r\n\r\n") {
                assert!(headers.len() < 4096);
                headers.push(socket.read_u8().await.unwrap());
            }
            assert!(headers.starts_with(b"HTTP/1.1 101"));
            // One masked binary frame: keyboard tag followed by a held key.
            let mask = [1_u8, 2, 3, 4];
            let mut frame = vec![0x82, 0x89];
            frame.extend_from_slice(&mask);
            frame.extend(
                [1_u8, 0, 0, 4, 0, 0, 0, 0, 0]
                    .iter()
                    .enumerate()
                    .map(|(index, byte)| byte ^ mask[index % 4]),
            );
            socket.write_all(&frame).await.unwrap();
            report_seen.notified().await;
            shutdown.stop();
            release_seen.notified().await;
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(20), shutdown.drained())
                    .await
                    .is_err()
            );
            allow_release.notify_one();
            shutdown.drained().await;
            server.await.unwrap();
            broker.await.unwrap();
            return;
        }

        let mut logout = json_request(Method::POST, "/api/auth/logout", Value::Null);
        logout
            .headers_mut()
            .insert(header::COOKIE, cookie.parse().unwrap());
        logout
            .headers_mut()
            .insert("x-csrf-token", csrf.parse().unwrap());
        assert_eq!(
            json_body(app.clone().oneshot(logout).await.unwrap()).await["code"],
            0
        );
        broker.await.unwrap();
    }

    #[tokio::test]
    async fn application_power_reports_broker_cooldown() {
        let root = tempdir().unwrap();
        let control_socket = root.path().join("control.sock");
        let listener = UnixListener::bind(&control_socket).unwrap();
        let broker = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut prefix = [0_u8; 4];
                stream.read_exact(&mut prefix).await.unwrap();
                let mut payload = vec![0_u8; u32::from_be_bytes(prefix) as usize];
                stream.read_exact(&mut payload).await.unwrap();
                let request = bli_kvm_control_protocol::decode_request_v2(&payload).unwrap();
                let response = match request.request {
                    bli_kvm_core::PlatformControlRequestV2::Atx { .. } => {
                        bli_kvm_control_protocol::ControlResponse::Rejected {
                            code: bli_kvm_control_protocol::ControlRejectionCode::CooldownActive,
                        }
                    }
                    _ => bli_kvm_control_protocol::ControlResponse::Applied,
                };
                let envelope = bli_kvm_control_protocol::ControlResponseEnvelope {
                    schema_version: bli_kvm_control_protocol::CONTROL_SCHEMA_VERSION_V2,
                    request_id: request.request_id,
                    operation_id: request.operation_id,
                    response,
                };
                let payload = bli_kvm_control_protocol::encode_response_v2(&envelope).unwrap();
                stream
                    .write_all(&bli_kvm_control_protocol::encode_frame(&payload).unwrap())
                    .await
                    .unwrap();
            }
        });
        let app = app_with_options(ServerOptions {
            state_dir: root.path().join("account"),
            secure_cookies: true,
            control_socket: Some(control_socket),
            ..ServerOptions::default()
        });
        let (cookie, csrf) = setup_and_login(&app).await;

        let mut challenge_request = Request::builder()
            .method(Method::GET)
            .uri("/api/hid/focus")
            .header(header::HOST, "kvm.test")
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap();
        challenge_request.extensions_mut().insert(ConnectInfo(
            "192.0.2.10:4242".parse::<SocketAddr>().unwrap(),
        ));
        let challenge_body = json_body(app.clone().oneshot(challenge_request).await.unwrap()).await;
        let challenge = challenge_body["data"]["challenge"]
            .as_str()
            .unwrap()
            .to_owned();

        let mut confirm = json_request(
            Method::POST,
            "/api/hid/focus",
            serde_json::json!({
                "operationId": "0123456789abcdef0123456789abcdef",
                "challenge": challenge,
                "targetId": "11".repeat(16),
                "operatorConfirmed": true,
            }),
        );
        confirm
            .headers_mut()
            .insert(header::COOKIE, cookie.parse().unwrap());
        confirm
            .headers_mut()
            .insert("x-csrf-token", csrf.parse().unwrap());
        let confirm_body = json_body(app.clone().oneshot(confirm).await.unwrap()).await;
        assert_eq!(confirm_body["data"]["attested"], true);

        let mut power = json_request(
            Method::POST,
            "/api/application-power",
            serde_json::json!({
                "operationId": "00112233445566778899aabbccddeeff",
                "action": "power_short"
            }),
        );
        power
            .headers_mut()
            .insert(header::COOKIE, cookie.parse().unwrap());
        power
            .headers_mut()
            .insert("x-csrf-token", csrf.parse().unwrap());
        let power_body = json_body(app.clone().oneshot(power).await.unwrap()).await;
        assert_eq!(power_body["code"], 0);
        assert_eq!(power_body["data"]["applied"], false);
        assert_eq!(power_body["data"]["rejection"], "cooldown_active");
        broker.await.unwrap();
    }

    #[cfg(not(feature = "restricted-control-surface"))]
    #[tokio::test]
    async fn restricted_controls_stay_unexposed_in_default_builds() {
        let app = app_with_options(ServerOptions {
            control_socket: Some(PathBuf::from("/run/hardened-blikvm-test/control.sock")),
            restricted_controls_enabled: true,
            ..ServerOptions::default()
        });
        let response = app
            .oneshot(json_request(Method::GET, "/api/capabilities", Value::Null))
            .await
            .unwrap();
        let body = json_body(response).await;
        assert_eq!(body["data"]["hid"]["status"]["availability"], "unavailable");
        assert_eq!(
            body["data"]["power"]["status"]["availability"],
            "unavailable"
        );
        assert_eq!(
            body["data"]["system"]["actions"]["availability"],
            "unavailable"
        );
        assert_ne!(
            body["data"]["board"]["features"]["device_mdns"]["availability"],
            "read_write"
        );
    }

    #[cfg(feature = "restricted-control-surface")]
    #[tokio::test]
    async fn restricted_controls_are_advertised_only_when_opted_in() {
        let app = app_with_options(ServerOptions {
            control_socket: Some(PathBuf::from("/run/hardened-blikvm-test/control.sock")),
            restricted_controls_enabled: true,
            ..ServerOptions::default()
        });
        let response = app
            .oneshot(json_request(Method::GET, "/api/capabilities", Value::Null))
            .await
            .unwrap();
        let body = json_body(response).await;
        assert_eq!(body["data"]["hid"]["status"]["availability"], "read_write");
        assert_eq!(
            body["data"]["power"]["status"]["availability"],
            "read_write"
        );
        // Tailscale needs the administration helper, not configured here.
        assert_eq!(
            body["data"]["board"]["features"]["tailscale"]["availability"],
            "unavailable"
        );
        for feature in [
            "device_ssh",
            "device_mdns",
            "device_hid_only",
            "device_mouse_jiggler",
            "device_oled",
            "network_wifi",
            "system_firewall",
        ] {
            assert_eq!(
                body["data"]["board"]["features"][feature]["availability"], "read_write",
                "{feature}"
            );
        }
        assert_eq!(
            body["data"]["board"]["features"]["network_tls"]["availability"],
            "read_only"
        );
        for feature in ["dns", "ipv6"] {
            assert_eq!(
                body["data"]["network"][feature]["availability"],
                "read_only"
            );
        }

        let disabled = app_with_options(ServerOptions {
            restricted_controls_enabled: true,
            ..ServerOptions::default()
        });
        let response = disabled
            .oneshot(json_request(Method::GET, "/api/capabilities", Value::Null))
            .await
            .unwrap();
        let body = json_body(response).await;
        assert_eq!(body["data"]["hid"]["status"]["availability"], "unavailable");
        assert_eq!(
            body["data"]["power"]["status"]["availability"],
            "unavailable"
        );
        for feature in [
            "device_ssh",
            "device_mdns",
            "device_hid_only",
            "device_mouse_jiggler",
            "device_oled",
            "network_tls",
            "network_wifi",
            "system_firewall",
            "tailscale",
        ] {
            assert_ne!(
                body["data"]["board"]["features"][feature]["availability"], "read_write",
                "{feature}"
            );
        }
    }

    #[cfg(feature = "restricted-control-surface")]
    #[tokio::test]
    async fn new_restricted_endpoints_stay_unavailable_without_opt_in() {
        let directory = tempdir().unwrap();
        let app = app_with_options(ServerOptions {
            state_dir: directory.path().join("state"),
            ..ServerOptions::default()
        });
        let (cookie, csrf) = setup_and_login(&app).await;

        let leader = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/hid/shortcut/leader-key")
                    .header(header::HOST, "kvm.test")
                    .header(header::ORIGIN, "https://kvm.test")
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(leader.status(), StatusCode::SERVICE_UNAVAILABLE);

        let upload = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/download/file")
                    .header(header::HOST, "kvm.test")
                    .header(header::ORIGIN, "https://kvm.test")
                    .header(header::COOKIE, &cookie)
                    .header("x-csrf-token", &csrf)
                    .header(
                        header::CONTENT_TYPE,
                        "multipart/form-data; boundary=----blikvm",
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(upload.status(), StatusCode::SERVICE_UNAVAILABLE);

        // The websocket gate cannot be exercised here: axum needs the hyper
        // upgrade extension, so the extractor rejects a router-level request
        // before the handler runs.
    }

    #[tokio::test]
    async fn virtual_media_requires_an_authenticated_session() {
        let response = app()
            .oneshot(json_request(
                Method::POST,
                "/api/virtual-media",
                serde_json::json!({
                    "operationId": "0123456789abcdef0123456789abcdef",
                    "command": { "action": "set_connected", "connected": true }
                }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn storage_routes_require_an_authenticated_session() {
        for (method, uri) in [
            (Method::GET, "/api/storage/image"),
            (Method::GET, "/api/storage/image/mounted"),
            (Method::GET, "/api/storage/cdrom"),
            (Method::POST, "/api/storage/image/mount"),
            (Method::POST, "/api/storage/image/delete"),
            (Method::POST, "/api/storage/usb/reconnect"),
        ] {
            let request = if method == Method::POST {
                json_request(method.clone(), uri, serde_json::json!({"file": ""}))
            } else {
                Request::builder()
                    .method(method.clone())
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap()
            };
            assert_eq!(
                app().oneshot(request).await.unwrap().status(),
                StatusCode::UNAUTHORIZED,
                "{uri}"
            );
        }
    }

    #[tokio::test]
    async fn media_catalog_requires_an_authenticated_session() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/api/virtual-media/catalog")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[cfg(feature = "restricted-control-surface")]
    #[tokio::test]
    async fn restricted_surface_serves_the_sanitized_catalog() {
        let root = std::env::temp_dir().join(format!("blikvm-catalog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let catalog = root.join("catalog.json");
        std::fs::write(
            &catalog,
            r#"{"schemaVersion":1,"entries":[{"imageId":7,"imageName":"rescue.iso","size":4096,"sha256":"1111111111111111111111111111111111111111111111111111111111111111","cdromAllowed":true,"removableAllowed":false,"writableAllowed":false}]}"#,
        )
        .unwrap();
        let app = app_with_options(ServerOptions {
            control_socket: Some(root.join("control.sock")),
            restricted_controls_enabled: true,
            media_catalog: Some(catalog),
            state_dir: root.join("state"),
            ..ServerOptions::default()
        });
        let (cookie, _csrf) = setup_and_login(&app).await;
        let request = Request::builder()
            .method(Method::GET)
            .uri("/api/virtual-media/catalog")
            .header(header::COOKIE, cookie)
            .header(header::HOST, "kvm.test")
            .header(header::ORIGIN, "https://kvm.test")
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["data"]["images"][0]["imageId"], 7);
        assert_eq!(body["data"]["images"][0]["imageName"], "rescue.iso");
        assert!(body["data"]["images"][0].get("sha256").is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(feature = "restricted-control-surface")]
    #[tokio::test]
    async fn restricted_surface_advertises_virtual_media_writes() {
        let app = app_with_options(ServerOptions {
            control_socket: Some(PathBuf::from("/run/hardened-blikvm-test/control.sock")),
            restricted_controls_enabled: true,
            media_controls_enabled: true,
            ..ServerOptions::default()
        });
        let response = app
            .oneshot(json_request(Method::GET, "/api/capabilities", Value::Null))
            .await
            .unwrap();
        let body = json_body(response).await;
        assert_eq!(
            body["data"]["virtualMedia"]["status"]["availability"],
            "read_write"
        );

        let without_media = app_with_options(ServerOptions {
            control_socket: Some(PathBuf::from("/run/hardened-blikvm-test/control.sock")),
            restricted_controls_enabled: true,
            ..ServerOptions::default()
        });
        let response = without_media
            .oneshot(json_request(Method::GET, "/api/capabilities", Value::Null))
            .await
            .unwrap();
        let body = json_body(response).await;
        assert_eq!(
            body["data"]["virtualMedia"]["status"]["availability"],
            "read_only"
        );
    }
}
