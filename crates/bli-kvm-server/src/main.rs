use std::{env, error::Error, net::SocketAddr, path::PathBuf};

use bli_kvm_server::{ServerOptions, app_with_options};
use tokio::net::TcpListener;

const DEFAULT_BIND: &str = "127.0.0.1:18080";

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let address: SocketAddr = env::var("BLI_KVM_BIND")
        .unwrap_or_else(|_| DEFAULT_BIND.to_string())
        .parse()?;
    if !address.ip().is_loopback() {
        return Err("refusing a non-loopback bind; use a hardened reverse proxy".into());
    }

    let web_root =
        PathBuf::from(env::var("BLI_KVM_WEB_ROOT").unwrap_or_else(|_| "web/dist".into()));
    if !web_root.join("index.html").is_file() {
        return Err(format!(
            "web root does not contain index.html: {}",
            web_root.display()
        )
        .into());
    }
    let options = ServerOptions {
        web_root: Some(web_root),
        state_dir: PathBuf::from(env::var("BLI_KVM_STATE_DIR").unwrap_or_else(|_| "state".into())),
        secure_cookies: env::var("BLI_KVM_SECURE_COOKIES").as_deref() == Ok("1"),
        allow_missing_origin: env::var("BLI_KVM_ALLOW_MISSING_ORIGIN").as_deref() == Ok("1"),
        broker_socket: PathBuf::from(
            env::var("BLI_KVM_BROKER_SOCKET")
                .unwrap_or_else(|_| "/run/hardened-blikvm/platform-broker.sock".into()),
        ),
        video_socket: PathBuf::from(
            env::var("BLI_KVM_VIDEO_SOCKET")
                .unwrap_or_else(|_| "/run/hardened-blikvm-video/mjpeg.sock".into()),
        ),
        update_status_socket: env::var_os("BLI_KVM_UPDATE_STATUS_SOCKET").map(PathBuf::from),
        update_upload_socket: env::var_os("BLI_KVM_UPDATE_UPLOAD_SOCKET").map(PathBuf::from),
        update_activation_socket: env::var_os("BLI_KVM_UPDATE_ACTIVATION_SOCKET")
            .map(PathBuf::from),
        update_writes_enabled: env::var("BLI_KVM_UPDATE_WRITES_ENABLED").as_deref() == Ok("1"),
        control_socket: env::var_os("BLI_KVM_CONTROL_SOCKET").map(PathBuf::from),
        restricted_controls_enabled: env::var("BLI_KVM_RESTRICTED_CONTROLS").as_deref() == Ok("1"),
        media_catalog: env::var_os("BLI_KVM_MEDIA_CATALOG").map(PathBuf::from),
        media_controls_enabled: env::var("BLI_KVM_MEDIA_CONTROLS").as_deref() == Ok("1"),
        system_actions_enabled: env::var("BLI_KVM_SYSTEM_ACTIONS").as_deref() == Ok("1"),
        network_writes_enabled: env::var("BLI_KVM_NETWORK_WRITES").as_deref() == Ok("1"),
        video_controls_enabled: env::var("BLI_KVM_VIDEO_CONTROLS").as_deref() == Ok("1"),
        engine_socket: env::var_os("BLI_KVM_USTREAMER_SOCKET").map(PathBuf::from),
        media_directory: env::var_os("BLI_KVM_MEDIA_DIRECTORY").map(PathBuf::from),
        ..ServerOptions::default()
    };

    let listener = TcpListener::bind(address).await?;
    eprintln!("BLi-KVM development server listening on {address}");
    axum::serve(
        listener,
        app_with_options(options).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
