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
