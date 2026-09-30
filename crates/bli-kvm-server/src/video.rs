use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use axum::{
    Extension,
    body::Body,
    extract::State,
    http::{HeaderValue, Response, header},
};
use bytes::Bytes;
use http_body::{Body as HttpBody, Frame, SizeHint};

use crate::{
    AppState,
    auth::{middleware::CurrentSession, session::Session},
    error::{AppError, AppResult},
};

pub async fn mjpeg(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
) -> AppResult<Response<Body>> {
    let stream = state
        .video
        .mjpeg()
        .await
        .map_err(|_| AppError::ServiceUnavailable("video proxy unavailable".into()))?;
    // A slow viewer gets the newest frame instead of a queue of old ones.
    let boundary = stream
        .content_type
        .to_str()
        .ok()
        .and_then(crate::mjpeg_latest::boundary)
        .map(str::to_owned)
        .ok_or_else(|| AppError::ServiceUnavailable("video proxy unavailable".into()))?;
    let mut response = Response::new(Body::new(SessionBoundBody::new(
        crate::mjpeg_latest::newest_parts(stream.body, &boundary),
        session,
        state.options.shutdown.clone(),
    )));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, stream.content_type);
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, no-cache, must-revalidate"),
    );
    response
        .headers_mut()
        .insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    Ok(response)
}

struct SessionBoundBody<B> {
    inner: Pin<Box<B>>,
    session: Session,
    shutdown: crate::shutdown::Shutdown,
    cancelled: Pin<Box<dyn Future<Output = ()> + Send>>,
}

impl<B> SessionBoundBody<B> {
    fn new(inner: B, session: Session, shutdown: crate::shutdown::Shutdown) -> Self {
        let signal = shutdown.clone();
        Self {
            inner: Box::pin(inner),
            session,
            shutdown,
            cancelled: Box::pin(async move { signal.cancelled().await }),
        }
    }
}

impl<B> HttpBody for SessionBoundBody<B>
where
    B: HttpBody<Data = Bytes>,
{
    type Data = Bytes;
    type Error = B::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        if this.shutdown.is_stopping()
            || !this.session.is_active()
            || this.cancelled.as_mut().poll(context).is_ready()
        {
            return Poll::Ready(None);
        }
        this.inner.as_mut().poll_frame(context)
    }

    fn is_end_stream(&self) -> bool {
        self.shutdown.is_stopping() || !self.session.is_active() || self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        if self.session.is_active() && !self.shutdown.is_stopping() {
            self.inner.size_hint()
        } else {
            SizeHint::with_exact(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use std::task::Waker;

    use http_body::Body as _;
    use http_body_util::Full;

    use super::*;
    use crate::auth::session::SessionStore;

    /// Records whether the upstream stream is dropped when the response ends.
    struct DropFlaggedBody {
        dropped: Arc<AtomicBool>,
    }

    impl Drop for DropFlaggedBody {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::SeqCst);
        }
    }

    impl HttpBody for DropFlaggedBody {
        type Data = Bytes;
        type Error = std::io::Error;

        fn poll_frame(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
            Poll::Pending
        }
    }

    #[tokio::test]
    async fn bound_body_ends_after_session_revocation() {
        let sessions = SessionStore::new();
        let session = sessions.issue("operator", 60).await;
        let body = SessionBoundBody::new(
            Full::new(Bytes::from_static(b"frame")),
            session.clone(),
            crate::shutdown::Shutdown::default(),
        );
        assert!(!body.is_end_stream());

        sessions.revoke(&session.token).await;

        assert!(body.is_end_stream());
        assert_eq!(body.size_hint().exact(), Some(0));
    }

    #[tokio::test]
    async fn revoking_the_session_drops_the_upstream_stream() {
        let sessions = SessionStore::new();
        let session = sessions.issue("operator", 60).await;
        let dropped = Arc::new(AtomicBool::new(false));
        let mut body = SessionBoundBody::new(
            DropFlaggedBody {
                dropped: Arc::clone(&dropped),
            },
            session.clone(),
            crate::shutdown::Shutdown::default(),
        );
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        assert!(matches!(
            Pin::new(&mut body).poll_frame(&mut context),
            Poll::Pending
        ));

        sessions.revoke(&session.token).await;
        assert!(matches!(
            Pin::new(&mut body).poll_frame(&mut context),
            Poll::Ready(None)
        ));
        drop(body);
        assert!(
            dropped.load(Ordering::SeqCst),
            "the upstream MJPEG stream must be dropped once the session ends"
        );
    }

    #[tokio::test]
    async fn shutdown_wakes_and_finishes_an_idle_upstream_body() {
        use http_body_util::BodyExt;
        let sessions = SessionStore::new();
        let session = sessions.issue("operator", 60).await;
        let shutdown = crate::shutdown::Shutdown::default();
        let dropped = Arc::new(AtomicBool::new(false));
        let mut body = SessionBoundBody::new(
            DropFlaggedBody {
                dropped: dropped.clone(),
            },
            session,
            shutdown.clone(),
        );
        let reader = tokio::spawn(async move { body.frame().await });
        tokio::task::yield_now().await;
        shutdown.stop();
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), reader)
                .await
                .unwrap()
                .unwrap()
                .is_none()
        );
        assert!(dropped.load(Ordering::SeqCst));
    }
}
