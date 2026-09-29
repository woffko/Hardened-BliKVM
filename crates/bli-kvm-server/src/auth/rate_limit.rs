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

    pub fn is_locked(&mut self, source_ip: &str, username: &str) -> bool {
        self.prune();
        let now = Instant::now();
        self.attempts
            .get(&key(source_ip, username))
            .and_then(|attempt| attempt.lockout_until)
            .is_some_and(|until| now < until)
    }

    pub fn record_failure(&mut self, source_ip: &str, username: &str) -> bool {
        self.prune();
        let now = Instant::now();
        let key = key(source_ip, username);
        if !self.attempts.contains_key(&key) && self.attempts.len() >= MAX_TRACKED_KEYS {
            // Full: make room by dropping the oldest entry that is not locked;
            // a newcomer never evicts a lockout. With only lockouts left the
            // newcomer is not tracked.
            let oldest = self
                .attempts
                .iter()
                .filter(|(_, attempt)| attempt.lockout_until.is_none())
                .min_by_key(|(_, attempt)| attempt.last_failure)
                .map(|(key, _)| key.clone());
            match oldest {
                Some(oldest) => {
                    self.attempts.remove(&oldest);
                }
                None => return false,
            }
        }
        let attempt = self
            .attempts
            .entry(key)
            .or_insert(Attempt {
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

    pub fn record_success(&mut self, source_ip: &str, username: &str) {
        self.attempts.remove(&key(source_ip, username));
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

fn key(source_ip: &str, username: &str) -> String {
    format!("{source_ip}:{username}")
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn lockout_is_scoped_to_source_and_username() {
        let mut limiter = LoginRateLimiter::new(2, 60);
        assert!(!limiter.record_failure("192.0.2.1", "admin"));
        assert!(limiter.record_failure("192.0.2.1", "admin"));
        assert!(limiter.is_locked("192.0.2.1", "admin"));
        assert!(!limiter.is_locked("192.0.2.2", "admin"));
        assert!(!limiter.is_locked("192.0.2.1", "other"));
    }
}
