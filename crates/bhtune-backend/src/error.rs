//! Errors a [`crate::Backend`] method can fail with.

use crate::types::TagId;

/// All the ways a [`crate::Backend`] call can fail.
///
/// Deliberately not a single opaque/`anyhow`-style error: callers need to tell "the backend
/// itself is unreachable" apart from "one read/write/browse call failed" apart from "this
/// backend doesn't support that at all" to react correctly — in particular the
/// unattended-operation guardrails planned for `cli-safety` need to distinguish a connection
/// failure (abort immediately, nothing was attempted) from an operation failure (may be worth
/// one retry) rather than treating every failure identically.
///
/// The underlying cause is boxed (`Box<dyn std::error::Error + Send + Sync>`) rather than
/// naming a concrete type, since different [`crate::Backend`] implementations wrap
/// completely unrelated error types (a future `backend-opcda`'s `opcda_bridge::Error`, a
/// simulator's own internal error, golden-trace parse errors for `backend-replay`) and this
/// trait/error model must not force all of them to share one. `#[source]` still preserves the
/// full chain for `std::error::Error::source()`/`anyhow`/logging to walk.
#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    /// The backend could not be reached at all — nothing was read or written. For
    /// `backend-opcda`, this is expected to wrap `opcda_bridge::Error::Connect`.
    #[error("failed to connect to backend")]
    Connect(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// A `read`/`write`/`browse` call reached the backend but failed there — an RPC error,
    /// an unresolvable tag, a malformed response. For `backend-opcda`, this is expected to
    /// wrap `opcda_bridge::Error::Rpc`. Distinct from a rejected-but-otherwise-successful
    /// write (see [`crate::types::WriteOutcome`]), which is not an error at all.
    #[error("backend operation failed")]
    Operation(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// One specific tag's value could not be used as requested — e.g. its raw string value
    /// isn't valid for whatever the caller needed it to mean. Carries the tag so a caller
    /// reporting a failed run can name exactly which tag was the problem.
    #[error("tag '{tag}': {message}")]
    InvalidTagValue { tag: TagId, message: String },

    /// This backend does not implement the requested operation at all (e.g. `browse` on the
    /// simulator or replay backends, which have no real tag tree to browse) — distinct from
    /// a transient [`BackendError::Operation`] failure, since retrying can never help.
    #[error("'{operation}' is not supported by this backend")]
    Unsupported { operation: &'static str },
}

/// A `Result` alias for [`BackendError`], mirroring the ergonomics of `opcda_bridge::Result`
/// (and, further down, `sqlx`-style crate-local aliases already used by `bhtune-db`).
pub type BackendResult<T> = std::result::Result<T, BackendError>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn connect_error_displays_without_source_text_but_keeps_source_chain() {
        let source = io::Error::other("refused");
        let err = BackendError::Connect(Box::new(source));
        assert_eq!(err.to_string(), "failed to connect to backend");
        assert!(std::error::Error::source(&err).is_some());
    }

    #[test]
    fn operation_error_displays_without_source_text_but_keeps_source_chain() {
        let source = io::Error::other("timed out");
        let err = BackendError::Operation(Box::new(source));
        assert_eq!(err.to_string(), "backend operation failed");
        assert!(std::error::Error::source(&err).is_some());
    }

    #[test]
    fn invalid_tag_value_names_the_tag_and_reason() {
        let err = BackendError::InvalidTagValue {
            tag: "Area1.LIC101.MODE".to_string(),
            message: "unrecognized mode code 'FOO'".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "tag 'Area1.LIC101.MODE': unrecognized mode code 'FOO'"
        );
    }

    #[test]
    fn unsupported_names_the_operation() {
        let err = BackendError::Unsupported {
            operation: "browse",
        };
        assert_eq!(err.to_string(), "'browse' is not supported by this backend");
    }
}
