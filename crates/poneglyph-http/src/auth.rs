//! Bearer-token gate for `/api/*` and `/ingest` (PRD §12).
//!
//! Stricter than the PRD minimum: whenever `server.api_token` is set, it is
//! enforced — not only on non-loopback binds. The startup refusal for
//! non-loopback binds without a token lives in [`crate::validate_security`].

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::error::ApiError;
use crate::state::AppState;

pub async fn require_token(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let expected = match state.config.server.api_token.as_deref() {
        Some(t) if !t.trim().is_empty() => t,
        _ => return next.run(req).await, // no token configured ⇒ open (loopback-only)
    };

    let presented = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    match presented {
        Some(p) if ct_eq(p.as_bytes(), expected.as_bytes()) => next.run(req).await,
        _ => ApiError(StatusCode::UNAUTHORIZED, "missing or invalid API token".into())
            .into_response(),
    }
}

/// Constant-time byte comparison (length leak is fine — token length isn't secret).
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::ct_eq;

    #[test]
    fn ct_eq_basic() {
        assert!(ct_eq(b"secret", b"secret"));
        assert!(!ct_eq(b"secret", b"secreT"));
        assert!(!ct_eq(b"secret", b"secret2"));
        assert!(!ct_eq(b"", b"x"));
        assert!(ct_eq(b"", b""));
    }
}
