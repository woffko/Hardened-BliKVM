use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

/// Most keys the limiter remembers. Keys come from unauthenticated requests,
/// so the count is bounded whatever the callers send.
pub const MAX_TRACKED_KEYS: usize = 1024;

#[derive(Debug, Clone)]
struct Attempt {
    failures: u32,
    lockout_until: Option<Instant>,
    last_failure: Instant,
}

#[derive(Debug)]
pub struct LoginRateLimiter {
    max_failures: u32,
    lockout_duration: Duration,
    attempts: HashMap<String, Attempt>,
}

impl LoginRateLimiter {
    pub fn new(max_failures: u32, lockout_duration_secs: u64) -> Self {
        Self {
            max_failures: max_failures.max(1),
            lockout_duration: Duration::from_secs(lockout_duration_secs.max(1)),
            attempts: HashMap::new(),
        }
    }

    /// Whether a request from `source_ip` for `username` must wait: the
    /// source itself is locked (failures under any name count against it),
    /// or this name from this source is.
    pub fn is_locked(&mut self, source_ip: &str, username: &str) -> bool {
        self.prune();
        let now = Instant::now();
        [source_key(source_ip), key(source_ip, username)]
            .iter()
            .any(|key| self.locked(key, now))
    }

    /// Counts a failed password for `username` from `source_ip`, against the
    /// name from that source and against the source whatever the name.
    /// Returns whether either is locked now.
    pub fn record_failure(&mut self, source_ip: &str, username: &str) -> bool {
        self.prune();
        let now = Instant::now();
        let source = source_key(source_ip);
        // A locked source gains no new names: its own lockout already holds
        // every request, and the table stays for other sources.
        let pair = (!self.locked(&source, now)).then(|| key(source_ip, username));
        let mut locked = false;
        for key in [Some(source), pair].into_iter().flatten() {
            locked |= self.count(key, now);
        }
        locked
    }

    pub fn record_success(&mut self, source_ip: &str, username: &str) {
        self.attempts.remove(&key(source_ip, username));
        self.attempts.remove(&source_key(source_ip));
    }

    fn locked(&self, key: &str, now: Instant) -> bool {
        self.attempts
            .get(key)
            .and_then(|attempt| attempt.lockout_until)
            .is_some_and(|until| now < until)
    }

    fn count(&mut self, key: String, now: Instant) -> bool {
        if !self.attempts.contains_key(&key) && self.attempts.len() >= MAX_TRACKED_KEYS {
            // Full: make room by dropping the oldest entry that is not locked,
            // or the oldest lockout when only lockouts are left. Dropping the
            // newcomer instead would leave it unlimited.
            let oldest = self
                .attempts
                .iter()
                .min_by_key(|(_, attempt)| (attempt.lockout_until.is_some(), attempt.last_failure))
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                self.attempts.remove(&oldest);
            }
        }
        let attempt = self.attempts.entry(key).or_insert(Attempt {
            failures: 0,
            lockout_until: None,
            last_failure: now,
        });
        if now.duration_since(attempt.last_failure) > self.lockout_duration {
            attempt.failures = 0;
            attempt.lockout_until = None;
        }
        attempt.failures += 1;
        attempt.last_failure = now;
        if attempt.failures >= self.max_failures {
            attempt.lockout_until = Some(now + self.lockout_duration);
            return true;
        }
        false
    }

    fn prune(&mut self) {
        let now = Instant::now();
        self.attempts.retain(|_, attempt| {
            attempt.lockout_until.map_or_else(
                || now.duration_since(attempt.last_failure) <= self.lockout_duration,
                |until| now < until,
            )
        });
    }
}

/// The address a login, a password change or a terminal unlock is counted
/// under. nginx sets `X-Forwarded-For` to exactly the peer it saw, replacing
/// whatever the client sent. Through the proxy socket, which serves nginx
/// alone, that header is the client. Over TCP it is believed from a loopback
/// peer only while no proxy socket is named (an older system's nginx), since
/// any local process reaches the TCP port; the header must be one address.
/// Anything else counts as the connection's own address (code audit of
/// 2026-10-20, AUD-16.2).
pub struct ClientSource(pub String);

pub fn source_address(
    peer: Option<std::net::SocketAddr>,
    proxied: bool,
    proxy_named: bool,
    headers: &axum::http::HeaderMap,
) -> Option<String> {
    let forwarded = headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<std::net::IpAddr>().ok());
    if proxied {
        return Some(forwarded.map_or_else(|| "proxy".to_string(), |address| address.to_string()));
    }
    let peer = peer?;
    if peer.ip().is_loopback()
        && !proxy_named
        && let Some(address) = forwarded
    {
        return Some(address.to_string());
    }
    Some(peer.ip().to_string())
}

impl axum::extract::FromRequestParts<crate::AppState> for ClientSource {
    type Rejection = crate::error::AppError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &crate::AppState,
    ) -> Result<Self, Self::Rejection> {
        use axum::extract::ConnectInfo;
        let proxied = parts
            .extensions
            .get::<ConnectInfo<crate::proxy_socket::ProxyPeer>>()
            .is_some();
        let peer = parts
            .extensions
            .get::<ConnectInfo<std::net::SocketAddr>>()
            .map(|ConnectInfo(peer)| *peer);
        source_address(
            peer,
            proxied,
            state.options.proxy_socket.is_some(),
            &parts.headers,
        )
        .map(ClientSource)
        .ok_or_else(|| crate::error::AppError::Internal("the request has no peer address".into()))
    }
}

fn key(source_ip: &str, username: &str) -> String {
    format!("{source_ip}:{username}")
}

/// The key a source's failures count under whatever the name: an IPv4
/// address as it is, an IPv6 address by its /64, which one subscriber
/// usually holds whole.
fn source_key(source_ip: &str) -> String {
    match source_ip.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V6(address)) => {
            let prefix = u128::from(address) & !((1_u128 << 64) - 1);
            format!("source {}/64", std::net::Ipv6Addr::from(prefix))
        }
        _ => format!("source {source_ip}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_forwarded_header_is_believed_from_nginx_only() {
        use axum::http::HeaderMap;
        let source = |peer: &str, proxied: bool, proxy_named: bool, headers: &HeaderMap| {
            source_address(peer.parse().ok(), proxied, proxy_named, headers)
        };
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "198.51.100.7".parse().unwrap());
        let loopback = "127.0.0.1:5000";
        let remote = "192.0.2.10:5000";
        // Without a proxy socket (an older system): from loopback over TCP.
        assert_eq!(
            source(loopback, false, false, &headers).unwrap(),
            "198.51.100.7"
        );
        assert_eq!(
            source(remote, false, false, &headers).unwrap(),
            "192.0.2.10"
        );
        // With one: over TCP never, through the socket always.
        assert_eq!(
            source(loopback, false, true, &headers).unwrap(),
            "127.0.0.1"
        );
        assert_eq!(source("", true, true, &headers).unwrap(), "198.51.100.7");
        assert_eq!(source("", true, true, &HeaderMap::new()).unwrap(), "proxy");
        assert_eq!(source("", false, true, &headers), None);
        headers.insert("x-forwarded-for", "1.2.3.4, 5.6.7.8".parse().unwrap());
        assert_eq!(
            source(loopback, false, false, &headers).unwrap(),
            "127.0.0.1"
        );
        headers.insert("x-forwarded-for", "junk".parse().unwrap());
        assert_eq!(
            source(loopback, false, false, &headers).unwrap(),
            "127.0.0.1"
        );
        assert_eq!(
            source(loopback, false, false, &HeaderMap::new()).unwrap(),
            "127.0.0.1"
        );
    }

    #[test]
    fn many_distinct_keys_stay_bounded_and_keep_lockouts() {
        let mut limiter = LoginRateLimiter::new(1, 600);
        assert!(limiter.record_failure("192.0.2.1", "victim"));
        let name = "n".repeat(64);
        for index in 0..5000 {
            limiter.record_failure("192.0.2.9", &format!("{name}{index}"));
        }
        assert!(limiter.attempts.len() <= MAX_TRACKED_KEYS);
        assert!(limiter.is_locked("192.0.2.1", "victim"));
    }

    #[test]
    fn failures_under_many_names_lock_their_source() {
        let mut limiter = LoginRateLimiter::new(5, 600);
        for index in 0..5 {
            limiter.record_failure("192.0.2.9", &format!("guess{index}"));
        }
        assert!(limiter.is_locked("192.0.2.9", "operator"));
        assert!(limiter.is_locked("192.0.2.9", "terminal:operator"));
        assert!(!limiter.is_locked("192.0.2.10", "operator"));
    }

    #[test]
    fn a_table_full_of_lockouts_still_locks_a_newcomer() {
        let mut limiter = LoginRateLimiter::new(1, 600);
        for index in 0..MAX_TRACKED_KEYS {
            let source = format!("10.0.{}.{}", index / 256, index % 256);
            limiter.record_failure(&source, &format!("guess{index}"));
        }
        assert!(limiter.attempts.len() <= MAX_TRACKED_KEYS);
        limiter.record_failure("192.0.2.1", "operator");
        assert!(limiter.is_locked("192.0.2.1", "operator"));
    }

    #[test]
    fn an_ipv6_source_is_counted_by_its_64_bit_prefix() {
        let mut limiter = LoginRateLimiter::new(2, 600);
        limiter.record_failure("2001:db8:1:2::1", "a");
        limiter.record_failure("2001:db8:1:2:ffff::9", "b");
        assert!(limiter.is_locked("2001:db8:1:2::3", "c"));
        assert!(!limiter.is_locked("2001:db8:1:3::1", "c"));
    }

    #[test]
    fn the_terminals_password_check_counts_against_the_same_source() {
        let mut limiter = LoginRateLimiter::new(3, 600);
        for _ in 0..3 {
            limiter.record_failure("192.0.2.5", "terminal:operator");
        }
        assert!(limiter.is_locked("192.0.2.5", "operator"));
    }

    #[test]
    fn a_lockout_holds_its_source_and_no_other() {
        let mut limiter = LoginRateLimiter::new(2, 60);
        assert!(!limiter.record_failure("192.0.2.1", "admin"));
        assert!(limiter.record_failure("192.0.2.1", "admin"));
        assert!(limiter.is_locked("192.0.2.1", "admin"));
        assert!(limiter.is_locked("192.0.2.1", "other"));
        assert!(!limiter.is_locked("192.0.2.2", "admin"));
        limiter.record_success("192.0.2.1", "admin");
        assert!(!limiter.is_locked("192.0.2.1", "other"));
    }
}
