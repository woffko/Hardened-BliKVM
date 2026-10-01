//! The QUIC and TLS front of the Hardened BliKVM QUIC transport: a
//! WebTransport endpoint for the board's own page. It holds no authority of
//! its own: the web server redeems a page's token, serves its requests and
//! ends its session over the link. See `docs/design/quic-transport-v1.md`.

#![forbid(unsafe_code)]

pub mod certificate;
pub mod config;
pub mod input;
pub mod link;
pub mod session;

use std::{
    future::Future,
    net::SocketAddr,
    sync::{Arc, atomic::AtomicUsize},
    time::Duration,
};

use wtransport::{
    Endpoint, Identity, ServerConfig, VarInt, config::Ipv6DualStackConfig,
    endpoint::endpoint_side::Server, quinn,
};

use crate::{config::Config, link::Link, session::Slot};

/// Streams a page may have open at once. HTTP/3 itself takes three
/// unidirectional ones; the page opens the control and the input stream.
const MAX_BIDIRECTIONAL_STREAMS: u32 = 8;
const MAX_UNIDIRECTIONAL_STREAMS: u32 = 8;
/// What a page may send before it is read: its messages are small.
const STREAM_RECEIVE_WINDOW: u32 = 64 * 1024;
const RECEIVE_WINDOW: u32 = 256 * 1024;
/// The page sends no datagrams of its own.
const DATAGRAM_RECEIVE_BUFFER: usize = 16 * 1024;
/// Room for the datagrams of a frame or two on their way out.
const DATAGRAM_SEND_BUFFER: usize = 512 * 1024;

fn server_config(
    config: &Config,
    bind: SocketAddr,
    identity: Identity,
) -> Result<ServerConfig, String> {
    let mut transport = quinn::TransportConfig::default();
    transport
        .max_idle_timeout(Some(
            quinn::IdleTimeout::try_from(config.idle_timeout).map_err(|error| error.to_string())?,
        ))
        .keep_alive_interval(Some(config.keep_alive))
        .max_concurrent_bidi_streams(MAX_BIDIRECTIONAL_STREAMS.into())
        .max_concurrent_uni_streams(MAX_UNIDIRECTIONAL_STREAMS.into())
        .stream_receive_window(STREAM_RECEIVE_WINDOW.into())
        .receive_window(RECEIVE_WINDOW.into())
        .datagram_receive_buffer_size(Some(DATAGRAM_RECEIVE_BUFFER))
        .datagram_send_buffer_size(DATAGRAM_SEND_BUFFER);
    let builder = ServerConfig::builder();
    // The IPv6 wildcard also takes IPv4 peers; any other address is taken
    // as it is written.
    let builder = match bind {
        SocketAddr::V6(address) if address.ip().is_unspecified() => {
            builder.with_bind_address_v6(address, Ipv6DualStackConfig::Allow)
        }
        SocketAddr::V6(address) => builder.with_bind_address_v6(address, Ipv6DualStackConfig::Deny),
        SocketAddr::V4(_) => builder.with_bind_address(bind),
    };
    Ok(builder.with_custom_transport(identity, transport).build())
}

/// A listening gateway.
pub struct Gateway {
    endpoint: Endpoint<Server>,
    /// The address the endpoint was bound to.
    bind: SocketAddr,
    config: Arc<Config>,
    link: Link,
}

impl Gateway {
    /// Binds the endpoint with a new certificate and starts the server link.
    pub fn bind(config: Config) -> Result<Self, String> {
        let (identity, certificate) = certificate::make(config.certificate_days)?;
        let mut bind = config.bind;
        let first = Endpoint::server(server_config(&config, bind, identity.clone_identity())?);
        let endpoint = match first {
            Ok(endpoint) => endpoint,
            // A board without IPv6 still has IPv4.
            Err(_) if bind.is_ipv6() && bind.ip().is_unspecified() => {
                bind = SocketAddr::from(([0, 0, 0, 0], bind.port()));
                eprintln!("quic gateway: no IPv6 here; listening on {bind}");
                Endpoint::server(server_config(&config, bind, identity)?)
                    .map_err(|error| format!("the endpoint could not listen on {bind}: {error}"))?
            }
            Err(error) => {
                return Err(format!("the endpoint could not listen on {bind}: {error}"));
            }
        };
        let port = endpoint
            .local_addr()
            .map_err(|error| error.to_string())?
            .port();
        let link = Link::spawn(config.link_socket.clone(), port, certificate);
        Ok(Self {
            endpoint,
            bind,
            config: Arc::new(config),
            link,
        })
    }

    pub fn local_addr(&self) -> Result<SocketAddr, String> {
        self.endpoint
            .local_addr()
            .map_err(|error| error.to_string())
    }

    /// Serves pages until `shutdown` completes.
    pub async fn serve(self, shutdown: impl Future<Output = ()>) {
        let connections = Arc::new(AtomicUsize::new(0));
        let mut renew = tokio::time::interval_at(
            tokio::time::Instant::now() + self.config.certificate_use,
            self.config.certificate_use,
        );
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                incoming = self.endpoint.accept() => {
                    // No state is kept for an address that has not answered.
                    if !incoming.remote_address_validated() {
                        incoming.retry();
                        continue;
                    }
                    if !self.link.is_up() {
                        incoming.refuse();
                        continue;
                    }
                    let Some(slot) = Slot::take(&connections, self.config.max_connections) else {
                        incoming.refuse();
                        continue;
                    };
                    tokio::spawn(session::serve(
                        incoming,
                        self.link.clone(),
                        Arc::clone(&self.config),
                        slot,
                    ));
                }
                _ = renew.tick() => self.renew_certificate(),
                () = &mut shutdown => break,
            }
        }
        // The server hears first: while the connections are told and drain,
        // it must not hand out a token for an endpoint that is going.
        self.link.leave();
        self.endpoint.close(VarInt::from_u32(0), b"");
        let _ = tokio::time::timeout(Duration::from_secs(2), self.endpoint.wait_idle()).await;
    }

    /// A new certificate for sessions that open from now on; those that are
    /// open go on.
    fn renew_certificate(&self) {
        let renewed =
            certificate::make(self.config.certificate_days).and_then(|(identity, info)| {
                let config = server_config(&self.config, self.bind, identity)?;
                self.endpoint
                    .reload_config(config, false)
                    .map_err(|error| error.to_string())?;
                Ok(info)
            });
        match renewed {
            Ok(info) => {
                self.link.set_certificate(info);
                eprintln!("quic gateway: the certificate was replaced");
            }
            Err(reason) => eprintln!("quic gateway: the certificate was not replaced: {reason}"),
        }
    }
}
