//! Bounded loopback fetching of exact signed web assets.
//! Endpoint/process binding and authenticated API health remain caller gates.

use crate::native_identity::NativeSlotIdentity;
use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use hyper_util::rt::TokioIo;
use std::{
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Instant,
};

const MAX_ASSET_BYTES: u64 = 8 * 1024 * 1024;

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "web asset response rejected")
}

#[derive(Debug, PartialEq, Eq)]
pub struct WebTreeObservation {
    /// Each signed web entry is fetched, including lazy-loaded assets.
    pub manifest_assets: usize,
    /// Signed tree bytes, excluding the extra root-route fetch of index.html.
    pub manifest_bytes: u64,
}

/// Verify the root route and every web file in the held signed inventory using
/// one original deadline. Bodies are collected one at a time. This establishes
/// content delivery and core browser asset MIME types, not browser execution or
/// authenticated API health.
pub async fn verify_tree(
    context: &NativeSlotIdentity<'_>,
    endpoint: SocketAddr,
    deadline: Instant,
) -> io::Result<WebTreeObservation> {
    let mut observation = WebTreeObservation {
        manifest_assets: 0,
        manifest_bytes: 0,
    };
    // Preflight all entries before any network request. A wrong-mode or oversized
    // secondary asset must not silently disappear from readiness coverage.
    for path in context.web_asset_paths() {
        if Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        let (size, _) = context.web_asset_metadata(path).map_err(|_| invalid())?;
        if size > MAX_ASSET_BYTES {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        observation.manifest_assets += 1;
        observation.manifest_bytes = observation
            .manifest_bytes
            .checked_add(size)
            .ok_or_else(invalid)?;
    }
    verify_route(context, endpoint, "share/web/index.html", "/", deadline).await?;
    for path in context.web_asset_paths() {
        verify_asset(context, endpoint, path, deadline).await?;
    }
    Ok(observation)
}

/// Fetch a manifest-listed asset and compare the exact unencoded body to its
/// signed size/hash. No DNS, proxy configuration, cookies, redirects or content
/// decompression are used. The server's TCP identity is not established here.
pub async fn verify_asset(
    context: &NativeSlotIdentity<'_>,
    endpoint: SocketAddr,
    manifest_path: &str,
    deadline: Instant,
) -> io::Result<()> {
    // Validate membership before allocating an encoded path from caller input.
    context
        .web_asset_metadata(manifest_path)
        .map_err(|_| invalid())?;
    let relative = manifest_path
        .strip_prefix("share/web/")
        .ok_or_else(invalid)?;
    let mut route = String::from("/");
    for byte in relative.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            route.push(char::from(byte));
        } else {
            route.push_str(&format!("%{byte:02X}"));
        }
    }
    verify_route(context, endpoint, manifest_path, &route, deadline).await
}

async fn verify_route(
    context: &NativeSlotIdentity<'_>,
    endpoint: SocketAddr,
    manifest_path: &str,
    route: &str,
    deadline: Instant,
) -> io::Result<()> {
    let (size, _) = context
        .web_asset_metadata(manifest_path)
        .map_err(|_| invalid())?;
    let body = fetch_body(endpoint, route, size, deadline).await?;
    context
        .verify_web_asset(manifest_path, &body)
        .map_err(|_| invalid())?;
    if Instant::now() >= deadline {
        return Err(io::ErrorKind::TimedOut.into());
    }
    Ok(())
}

async fn fetch_body(
    endpoint: SocketAddr,
    route: &str,
    size: u64,
    deadline: Instant,
) -> io::Result<Vec<u8>> {
    if endpoint.ip() != IpAddr::V4(Ipv4Addr::LOCALHOST)
        || endpoint.port() < 1024
        || size > MAX_ASSET_BYTES
    {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    if Instant::now() >= deadline {
        return Err(io::ErrorKind::TimedOut.into());
    }
    tokio::time::timeout_at(deadline.into(), async {
        let socket = tokio::net::TcpStream::connect(endpoint).await?;
        let (mut sender, connection) = hyper::client::conn::http1::Builder::new()
            .max_buf_size(8192)
            .handshake(TokioIo::new(socket))
            .await
            .map_err(|_| invalid())?;
        let operation = async {
            let request = http::Request::builder()
                .method("GET")
                .uri(route)
                .header("Host", endpoint.to_string())
                .header("Connection", "close")
                .header("Accept-Encoding", "identity")
                .header("Cache-Control", "no-cache")
                .body(Empty::<Bytes>::new())
                .map_err(|_| invalid())?;
            let response = sender.send_request(request).await.map_err(|_| invalid())?;
            if response.status() != http::StatusCode::OK
                || response.headers().contains_key("content-encoding")
                || response.headers().contains_key("content-range")
            {
                return Err(invalid());
            }
            validate_content_type(response.headers(), route)?;
            if let Some(length) = response.headers().get("content-length") {
                let length: u64 = length
                    .to_str()
                    .map_err(|_| invalid())?
                    .parse()
                    .map_err(|_| invalid())?;
                if length != size {
                    return Err(invalid());
                }
            }
            let mut body = response.into_body();
            let mut output = Vec::new();
            while let Some(frame) = body.frame().await {
                let frame = frame.map_err(|_| invalid())?;
                let data = frame.into_data().map_err(|_| invalid())?;
                if output.len() as u64 + data.len() as u64 > size {
                    return Err(invalid());
                }
                output.extend_from_slice(&data);
            }
            if output.len() as u64 != size {
                return Err(invalid());
            }
            Ok(output)
        };
        // Drive both futures together; cancellation drops the socket directly,
        // with no spawned HTTP driver surviving success, error or timeout.
        let (body, ()) =
            tokio::try_join!(operation, async { connection.await.map_err(|_| invalid()) })?;
        Ok(body)
    })
    .await
    .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))?
}

fn validate_content_type(headers: &http::HeaderMap, route: &str) -> io::Result<()> {
    let allowed: &[&str] = if route == "/" || route.ends_with(".html") {
        &["text/html"]
    } else if route.ends_with(".js") || route.ends_with(".mjs") {
        &["text/javascript", "application/javascript"]
    } else if route.ends_with(".css") {
        &["text/css"]
    } else if route.ends_with(".wasm") {
        &["application/wasm"]
    } else if route.ends_with(".svg") {
        &["image/svg+xml"]
    } else {
        // Other signed resources still receive size/hash verification. Do not
        // invent MIME requirements for arbitrary manifest-listed data files.
        return Ok(());
    };
    let mut values = headers.get_all(http::header::CONTENT_TYPE).iter();
    let value = values
        .next()
        .ok_or_else(invalid)?
        .to_str()
        .map_err(|_| invalid())?;
    if values.next().is_some() {
        return Err(invalid());
    }
    let media_type = value.split(';').next().ok_or_else(invalid)?.trim();
    if !allowed
        .iter()
        .any(|expected| media_type.eq_ignore_ascii_case(expected))
    {
        return Err(invalid());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn browser_assets_require_one_appropriate_content_type() {
        for (route, mime) in [
            ("/", "text/html; charset=utf-8"),
            ("/index.html", "TEXT/HTML"),
            ("/assets/app.js", "text/javascript"),
            ("/assets/app.mjs", "application/javascript"),
            ("/assets/app.css", "text/css"),
            ("/module.wasm", "application/wasm"),
            ("/icon.svg", "image/svg+xml"),
        ] {
            let mut headers = http::HeaderMap::new();
            assert!(validate_content_type(&headers, route).is_err());
            headers.insert(
                http::header::CONTENT_TYPE,
                http::HeaderValue::from_static("application/octet-stream"),
            );
            assert!(validate_content_type(&headers, route).is_err());
            headers.insert(
                http::header::CONTENT_TYPE,
                http::HeaderValue::from_str(mime).unwrap(),
            );
            validate_content_type(&headers, route).unwrap();
            headers.append(
                http::header::CONTENT_TYPE,
                http::HeaderValue::from_str(mime).unwrap(),
            );
            assert!(validate_content_type(&headers, route).is_err());
        }
    }

    #[tokio::test]
    async fn stalled_body_expires_and_drops_the_connection() {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                socket.read_exact(&mut byte).await.unwrap();
                request.push(byte[0]);
                assert!(request.len() <= 4096);
            }
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 3\r\n\r\na",
                )
                .await
                .unwrap();
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(1), socket.read(&mut [0]))
                    .await
                    .unwrap()
                    .unwrap(),
                0
            );
        });
        let result = fetch_body(
            address,
            "/index.html",
            3,
            Instant::now() + Duration::from_millis(100),
        )
        .await;
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn fixed_and_chunked_bodies_are_bounded_and_redirects_are_rejected() {
        for (response, valid) in [
            (
                "HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nabc",
                true,
            ),
            (
                "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n3\r\nabc\r\n0\r\n\r\n",
                true,
            ),
            (
                "HTTP/1.1 302 Found\r\nLocation: http://example.invalid/\r\nContent-Length: 0\r\n\r\n",
                false,
            ),
            ("HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nabcd", false),
            (
                "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nabcd\r\n0\r\n\r\n",
                false,
            ),
            (
                "HTTP/1.1 200 OK\r\nContent-Length: 3\r\nContent-Encoding: gzip\r\n\r\nabc",
                false,
            ),
            ("HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nab", false),
        ] {
            let response =
                response.replacen("200 OK\r\n", "200 OK\r\nContent-Type: text/html\r\n", 1);
            let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    socket.read_exact(&mut byte).await.unwrap();
                    request.push(byte[0]);
                    assert!(request.len() <= 4096);
                }
                assert!(request.starts_with(b"GET /index.html HTTP/1.1\r\n"));
                socket.write_all(response.as_bytes()).await.unwrap();
            });
            let result = fetch_body(
                address,
                "/index.html",
                3,
                Instant::now() + Duration::from_secs(1),
            )
            .await;
            if valid {
                assert_eq!(result.unwrap(), b"abc");
            } else {
                assert!(result.is_err());
            }
            server.await.unwrap();
        }
    }
}
