//! The socket nginx reaches the server through (code audit of 2026-10-20,
//! AUD-16.2). Any local process can open the server's TCP port, so a client
//! address forwarded there could be named by any of them; on this socket the
//! server serves nginx's account alone, checked by the peer's credentials,
//! and believes `X-Forwarded-For` there only.

use std::{io, os::unix::fs::PermissionsExt, path::Path};

use axum::{
    extract::connect_info::Connected,
    serve::{IncomingStream, Listener},
};
use tokio::net::{UnixListener, UnixStream, unix::SocketAddr};

/// A connection through the proxy socket.
#[derive(Clone, Copy, Debug)]
pub struct ProxyPeer;

/// Accepts only connections whose peer runs as one user.
pub struct ProxyListener {
    listener: UnixListener,
    uid: u32,
}

impl ProxyListener {
    pub fn bind(path: &Path, uid: u32) -> io::Result<Self> {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let listener = UnixListener::bind(path)?;
        // Who is served is decided per connection, by its credentials.
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o666))?;
        Ok(Self { listener, uid })
    }
}

impl Listener for ProxyListener {
    type Io = UnixStream;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let (stream, address) = Listener::accept(&mut self.listener).await;
            match stream.peer_cred() {
                Ok(credentials) if credentials.uid() == self.uid => return (stream, address),
                Ok(credentials) => {
                    eprintln!(
                        "proxy socket: a peer of uid {} was refused",
                        credentials.uid()
                    );
                }
                Err(error) => eprintln!("proxy socket: a peer was refused: {error}"),
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.listener.local_addr()
    }
}

impl Connected<IncomingStream<'_, ProxyListener>> for ProxyPeer {
    fn connect_info(_stream: IncomingStream<'_, ProxyListener>) -> Self {
        Self
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn only_the_named_user_is_served() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("proxy.sock");
        let own = nix::unistd::geteuid().as_raw();

        let mut listener = ProxyListener::bind(&path, own).unwrap();
        let _client = UnixStream::connect(&path).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), listener.accept())
            .await
            .expect("the named user was not served");

        let mut listener = ProxyListener::bind(&path, own.wrapping_add(1)).unwrap();
        let _client = UnixStream::connect(&path).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(300), listener.accept())
                .await
                .is_err(),
            "another user was served"
        );
    }
}
