use std::{
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
    let mut response = Response::new(Body::new(SessionBoundBody::new(stream.body, session)));
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
}

impl<B> SessionBoundBody<B> {
    fn new(inner: B, session: Session) -> Self {
        Self {
            inner: Box::pin(inner),
            session,
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
        if !this.session.is_active() {
            return Poll::Ready(None);
        }
        this.inner.as_mut().poll_frame(context)
    }

    fn is_end_stream(&self) -> bool {
        !self.session.is_active() || self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        if self.session.is_active() {
            self.inner.size_hint()
        } else {
            SizeHint::with_exact(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use http_body_util::Full;

    use super::*;
    use crate::auth::session::SessionStore;

    #[tokio::test]
    async fn bound_body_ends_after_session_revocation() {
        let sessions = SessionStore::new();
        let session = sessions.issue("operator", 60).await;
        let body = SessionBoundBody::new(Full::new(Bytes::from_static(b"frame")), session.clone());
        assert!(!body.is_end_stream());

        sessions.revoke(&session.token).await;

        assert!(body.is_end_stream());
        assert_eq!(body.size_hint().exact(), Some(0));
    }
}
