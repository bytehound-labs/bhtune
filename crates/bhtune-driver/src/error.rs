//! Errors a [`crate::Driver`] method can fail with.

use crate::types::TagId;

/// All the ways a [`crate::Driver`] call can fail.
///
/// Deliberately not a single opaque/`anyhow`-style error: callers need to tell "the driver
/// itself is unreachable" apart from "one read/write/browse call failed" apart from "this
/// driver doesn't support that at all" to react correctly — in particular the
/// unattended-operation guardrails planned for `cli-safety` need to distinguish a connection
/// failure (abort immediately, nothing was attempted) from an operation failure (may be worth
/// one retry) rather than treating every failure identically.
///
/// The underlying cause is boxed (`Box<dyn std::error::Error + Send + Sync>`) rather than
/// naming a concrete type, since different [`crate::Driver`] implementations wrap
/// completely unrelated error types (a future `driver-opcda`'s `opcda_bridge::Error`, a
/// simulator's own internal error, golden-trace parse errors for `driver-replay`) and this
/// trait/error model must not force all of them to share one. `#[source]` still preserves the
/// full chain for `std::error::Error::source()`/`anyhow`/logging to walk.
#[derive(Debug, thiserror::Error)]
pub enum DriverError {
    /// The driver could not be reached at all — nothing was read or written. For
    /// `driver-opcda`, this is expected to wrap `opcda_bridge::Error::Connect`.
    #[error("failed to connect to driver")]
    Connect(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// A `read`/`write`/`browse` call reached the driver but failed there — an RPC error,
    /// an unresolvable tag, a malformed response. For `driver-opcda`, this is expected to
    /// wrap `opcda_bridge::Error::Rpc`. Distinct from a rejected-but-otherwise-successful
    /// write (see [`crate::types::WriteOutcome`]), which is not an error at all.
    #[error("driver operation failed")]
    Operation(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// One specific tag's value could not be used as requested — e.g. its raw string value
    /// isn't valid for whatever the caller needed it to mean. Carries the tag so a caller
    /// reporting a failed run can name exactly which tag was the problem.
    #[error("tag '{tag}': {message}")]
    InvalidTagValue { tag: TagId, message: String },

    /// This driver does not implement the requested operation at all (e.g. `browse` on the
    /// simulator or replay drivers, which have no real tag tree to browse) — distinct from
    /// a transient [`DriverError::Operation`] failure, since retrying can never help.
    #[error("'{operation}' is not supported by this driver")]
    Unsupported { operation: &'static str },

    /// The connected gateway predates a required protocol operation.
    #[error(
        "gateway does not support {operation}; upgrade the OPC DA bridge gateway to a compatible version"
    )]
    IncompatibleGateway { operation: &'static str },

    /// A server-side browse session or continuation token is no longer usable.
    #[error("browse session or cursor is no longer valid; reopen the tag browser")]
    BrowseStateInvalid,
}

/// A `Result` alias for [`DriverError`], mirroring the ergonomics of `opcda_bridge::Result`
/// (and, further down, `sqlx`-style crate-local aliases already used by `bhtune-db`).
pub type DriverResult<T> = std::result::Result<T, DriverError>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn connect_error_displays_without_source_text_but_keeps_source_chain() {
        let source = io::Error::other("refused");
        let err = DriverError::Connect(Box::new(source));
        assert_eq!(err.to_string(), "failed to connect to driver");
        assert!(std::error::Error::source(&err).is_some());
    }

    #[test]
    fn operation_error_displays_without_source_text_but_keeps_source_chain() {
        let source = io::Error::other("timed out");
        let err = DriverError::Operation(Box::new(source));
        assert_eq!(err.to_string(), "driver operation failed");
        assert!(std::error::Error::source(&err).is_some());
    }

    #[test]
    fn invalid_tag_value_names_the_tag_and_reason() {
        let err = DriverError::InvalidTagValue {
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
        let err = DriverError::Unsupported {
            operation: "browse",
        };
        assert_eq!(err.to_string(), "'browse' is not supported by this driver");
    }

    #[test]
    fn incompatible_gateway_names_the_upgrade_action() {
        let err = DriverError::IncompatibleGateway {
            operation: "paged browse",
        };
        assert!(
            err.to_string()
                .contains("upgrade the OPC DA bridge gateway")
        );
    }

    #[test]
    fn invalid_browse_state_is_actionable() {
        assert!(
            DriverError::BrowseStateInvalid
                .to_string()
                .contains("reopen the tag browser")
        );
    }
}
