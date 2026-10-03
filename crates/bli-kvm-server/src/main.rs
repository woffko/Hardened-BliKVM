use std::{
    env, error::Error, future::IntoFuture, net::SocketAddr, os::fd::AsRawFd,
    os::unix::fs::PermissionsExt, path::PathBuf, time::Duration,
};

use axum::serve::ListenerExt;
use bli_kvm_server::{ServerOptions, app_with_options};
use tokio::net::{TcpListener, UnixListener, UnixStream};

const DEFAULT_BIND: &str = "127.0.0.1:18080";
/// Send buffer of a stream connection (the kernel doubles it).
const STREAM_SEND_BUFFER: libc::c_int = 32 * 1024;

/// A small, fixed send buffer: a slow viewer's frames then wait in the
/// server, where a newer frame replaces them, not in nginx.
fn small_send_buffer(connection: &mut UnixStream) {
    // SAFETY: the descriptor is the live socket; the option value is a
    // plain int that outlives the call.
    unsafe {
        libc::setsockopt(
            connection.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_SNDBUF,
            (&STREAM_SEND_BUFFER as *const libc::c_int).cast(),
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        );
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
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
    // The QUIC gateway's link exists only when its socket and its user are
    // both named and the user exists; anything less leaves QUIC unavailable
    // and the server as it is.
    let (quic_link_socket, quic_link_uid) = match (
        env::var_os("BLI_KVM_QUIC_LINK_SOCKET").map(PathBuf::from),
        env::var("BLI_KVM_QUIC_LINK_USER").ok(),
    ) {
        (Some(socket), Some(user)) => match nix::unistd::User::from_name(&user) {
            Ok(Some(found)) => (Some(socket), Some(found.uid.as_raw())),
            _ => {
                eprintln!(
                    "quic link: user {user} does not exist; the QUIC transport stays unavailable"
                );
                (None, None)
            }
        },
        _ => (None, None),
    };
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
        update_native_admission: env::var("BLI_KVM_UPDATE_NATIVE_ADMISSION").as_deref() == Ok("1"),
        webrtc_janus: env::var("BLI_KVM_JANUS_WS")
            .ok()
            .and_then(|value| value.parse().ok()),
        webrtc_demand_socket: env::var_os("BLI_KVM_WEBRTC_DEMAND_SOCKET").map(PathBuf::from),
        webrtc_h264_demand_socket: env::var_os("BLI_KVM_WEBRTC_H264_DEMAND_SOCKET")
            .map(PathBuf::from),
        control_socket: env::var_os("BLI_KVM_CONTROL_SOCKET").map(PathBuf::from),
        restricted_controls_enabled: env::var("BLI_KVM_RESTRICTED_CONTROLS").as_deref() == Ok("1"),
        media_catalog: env::var_os("BLI_KVM_MEDIA_CATALOG").map(PathBuf::from),
        media_controls_enabled: env::var("BLI_KVM_MEDIA_CONTROLS").as_deref() == Ok("1"),
        system_actions_enabled: env::var("BLI_KVM_SYSTEM_ACTIONS").as_deref() == Ok("1"),
        network_writes_enabled: env::var("BLI_KVM_NETWORK_WRITES").as_deref() == Ok("1"),
        firewall_modes_enabled: env::var("BLI_KVM_FIREWALL_MODES").as_deref() == Ok("1"),
        video_controls_enabled: env::var("BLI_KVM_VIDEO_CONTROLS").as_deref() == Ok("1"),
        engine_socket: env::var_os("BLI_KVM_USTREAMER_SOCKET").map(PathBuf::from),
        h264_encoder_status: env::var_os("BLI_KVM_H264_ENCODER_STATUS").map(PathBuf::from),
        h264_encoder_report: env::var_os("BLI_KVM_H264_ENCODER_REPORT").map(PathBuf::from),
        admin_socket: env::var_os("BLI_KVM_ADMIN_SOCKET").map(PathBuf::from),
        download_socket: env::var_os("BLI_KVM_DOWNLOAD_SOCKET").map(PathBuf::from),
        picoclaw_socket: env::var_os("BLI_KVM_PICOCLAW_SOCKET").map(PathBuf::from),
        picoclaw_mcp_url: Some(format!("http://{address}/internal/picoclaw/mcp")),
        media_directory: env::var_os("BLI_KVM_MEDIA_DIRECTORY").map(PathBuf::from),
        media_store_must_be_mounted: true,
        quic_link_socket,
        quic_link_uid,
        ..ServerOptions::default()
    };

    // nginx takes the MJPEG stream from a unix socket: a loopback TCP
    // connection's receive buffer in nginx grows to megabytes, and a slow
    // viewer's frames waited there. Without the setting the stream goes over
    // TCP like every other route.
    let stream_socket = env::var_os("BLI_KVM_STREAM_SOCKET").map(PathBuf::from);
    let stream_listener = match &stream_socket {
        Some(path) => {
            if !path.is_absolute() {
                return Err("BLI_KVM_STREAM_SOCKET must be absolute".into());
            }
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            let listener = UnixListener::bind(path)?;
            // Any local process could already reach the TCP port; the
            // session check is the same on both.
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o666))?;
            Some(listener)
        }
        None => None,
    };

    let listener = TcpListener::bind(address).await?;
    eprintln!("BLi-KVM development server listening on {address}");
    let shutdown = options.shutdown.clone();
    let graceful = shutdown.clone();
    let app = app_with_options(options);
    let server = axum::serve(
        listener,
        app.clone()
            .into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move { graceful.cancelled().await })
    .into_future();
    let stream_graceful = shutdown.clone();
    let stream_server = async move {
        match stream_listener {
            Some(listener) => {
                axum::serve(listener.tap_io(small_send_buffer), app.into_make_service())
                    .with_graceful_shutdown(async move { stream_graceful.cancelled().await })
                    .await
            }
            None => {
                stream_graceful.cancelled().await;
                Ok(())
            }
        }
    };
    tokio::pin!(server);
    tokio::pin!(stream_server);
    tokio::select! {
        result = &mut server => { result?; return Ok(()); },
        result = &mut stream_server => { result?; return Ok(()); },
        _ = terminate.recv() => {},
        _ = interrupt.recv() => {},
    }
    shutdown.stop();
    tokio::time::timeout(Duration::from_secs(20), async {
        server.await?;
        stream_server.await?;
        shutdown.drained().await;
        Ok::<(), std::io::Error>(())
    })
    .await
    .map_err(|_| "server shutdown deadline exceeded")??;
    Ok(())
}
