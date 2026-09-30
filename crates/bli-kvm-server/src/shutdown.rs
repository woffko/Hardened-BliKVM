//! Shared shutdown admission and upgraded-connection drain tracking.

use tokio::sync::watch;

pub(crate) async fn admit(
    axum::extract::State(shutdown): axum::extract::State<Shutdown>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if shutdown.is_stopping() {
        return crate::error::AppError::ServiceUnavailable("server is stopping".into())
            .into_response();
    }
    next.run(request).await
}

#[derive(Debug, Clone, Copy, Default)]
struct State {
    stopping: bool,
    upgrades: usize,
}

#[derive(Debug, Clone)]
pub struct Shutdown {
    state: watch::Sender<State>,
}

impl Default for Shutdown {
    fn default() -> Self {
        Self {
            state: watch::channel(State::default()).0,
        }
    }
}

impl Shutdown {
    pub fn stop(&self) {
        self.state.send_modify(|state| state.stopping = true);
    }

    pub fn is_stopping(&self) -> bool {
        self.state.borrow().stopping
    }

    pub async fn cancelled(&self) {
        let mut receiver = self.state.subscribe();
        let _ = receiver.wait_for(|state| state.stopping).await;
    }

    pub async fn drained(&self) {
        let mut receiver = self.state.subscribe();
        let _ = receiver
            .wait_for(|state| state.stopping && state.upgrades == 0)
            .await;
    }

    pub(crate) fn register_upgrade(&self) -> Option<UpgradeGuard> {
        let mut admitted = false;
        self.state.send_if_modified(|state| {
            if state.stopping {
                return false;
            }
            let Some(count) = state.upgrades.checked_add(1) else {
                return false;
            };
            state.upgrades = count;
            admitted = true;
            true
        });
        admitted.then(|| UpgradeGuard(self.clone()))
    }
}

pub(crate) struct UpgradeGuard(Shutdown);

impl Drop for UpgradeGuard {
    fn drop(&mut self) {
        self.0.state.send_modify(|state| state.upgrades -= 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn shutdown_rejects_late_upgrades_and_waits_for_existing_guards() {
        let shutdown = Shutdown::default();
        let guard = shutdown.register_upgrade().unwrap();
        shutdown.stop();
        shutdown.stop();
        assert!(shutdown.register_upgrade().is_none());
        shutdown.cancelled().await;
        assert!(
            tokio::time::timeout(Duration::from_millis(10), shutdown.drained())
                .await
                .is_err()
        );
        drop(guard);
        tokio::time::timeout(Duration::from_secs(1), shutdown.drained())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn stopped_router_rejects_health_and_public_write_admission() {
        use tower::ServiceExt;
        let directory = tempfile::tempdir().unwrap();
        let options = crate::ServerOptions {
            state_dir: directory.path().into(),
            ..Default::default()
        };
        let shutdown = options.shutdown.clone();
        let app = crate::app_with_options(options);
        shutdown.stop();
        for (method, uri) in [("GET", "/api/health"), ("POST", "/api/auth/login")] {
            let response = app
                .clone()
                .oneshot(
                    axum::http::Request::builder()
                        .method(method)
                        .uri(uri)
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                axum::http::StatusCode::SERVICE_UNAVAILABLE
            );
        }
    }
}
