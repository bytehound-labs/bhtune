//! Plain data types moved across the [`crate::Backend`] trait boundary.

use chrono::{DateTime, Utc};

/// An identifier for one tag/item a [`crate::Backend`] knows how to read or write. For OPC
/// DA this is the fully-qualified item name (e.g. `"Area1.LIC101.PV"`, matching
/// `bhtune_core::tags::LoopTags`'s tag fields exactly); other backends may use any string
/// convention of their own, since only the implementing backend interprets it.
///
/// A plain `String` alias rather than a newtype: there is no invariant here worth enforcing
/// (a tag ID is valid or not only in the sense that the backend does or doesn't recognize
/// it, which no wrapper type can check ahead of time), so a newtype would only add ceremony.
pub type TagId = String;

/// How much a [`TagValue`] should be trusted, mirroring OPC's own three-state quality model
/// rather than collapsing it to a bool. "Uncertain" (e.g. a value held at its last known
/// reading during a brief comms hiccup) is a real, actionable distinction from outright bad
/// quality — not merely a presentation nuance — so callers that need to decide whether to
/// trust a reading can react to it directly rather than losing the distinction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Quality {
    Good,
    Uncertain,
    Bad,
}

impl Quality {
    /// Whether a value with this quality should be trusted for tuning-critical decisions —
    /// feeding an MRFT tick, or treating an initial-readings snapshot as having actually
    /// succeeded. Only `Good` counts: MRFT's peak/trough detection has no way to partially
    /// discount a merely-`Uncertain` reading the way a human operator glancing at a trend
    /// might, so treating it as untrustworthy is the safe default.
    pub fn is_trustworthy(self) -> bool {
        matches!(self, Quality::Good)
    }
}

/// One tag's freshly read value, as returned by [`crate::Backend::read`].
///
/// `value` is a raw string, not a parsed `f32` — deliberately, since not every tag a
/// [`crate::Backend`] reads is numeric. Mode/direction/attribute tags hold small raw codes
/// (e.g. `"MAN"`, `"0"`) that `bhtune_core`'s own parsing functions interpret directly (see
/// `bhtune_core::ControllerDirection::from_raw_tag_value`), so this type must not assume
/// every tag's value is a number. Parsing a numeric tag's `value` into `f32` — and treating a
/// parse failure as an error rather than silently substituting a default — is left to the
/// caller that knows which of its requested tags are numeric, mirroring how
/// `opcda-bridge`'s own `Client::read` returns every value as a string for the same reason.
#[derive(Debug, Clone, PartialEq)]
pub struct TagValue {
    pub tag: TagId,
    pub value: String,
    pub quality: Quality,
    pub timestamp: DateTime<Utc>,
}

/// A value to write to a tag via [`crate::Backend::write`].
///
/// Deliberately narrower than a full OPC VARIANT-style type space: bhtune only ever writes
/// numeric process values (relay steps during MRFT, PID constants at write-back) or a raw
/// mode code (reverting a loop's Auto/Manual mode after a completed test, per
/// `bhtune_core::DcsTemplate::revert_mode`) — never, say, a boolean or an arbitrary integer.
#[derive(Debug, Clone, PartialEq)]
pub enum TagWrite {
    Float(f32),
    Raw(String),
}

/// The result of one [`crate::Backend::write`] call that reached the backend.
///
/// Kept distinct from a [`crate::BackendError`]: a backend (or the DCS/PLC behind it)
/// rejecting a write — the tag is read-only, the value is out of range, permissions — is a
/// normal, expected outcome of the call succeeding at the transport level, not a
/// connection/operation failure. This shape mirrors `bhtune_db::models::TuneWriteRow`'s own
/// `success`/`error_message` columns exactly, so a caller recording a write-back audit row
/// can copy this outcome straight into that table with no translation.
#[derive(Debug, Clone, PartialEq)]
pub struct WriteOutcome {
    pub success: bool,
    pub error_message: Option<String>,
}

impl WriteOutcome {
    /// A write the backend accepted outright.
    pub fn success() -> WriteOutcome {
        WriteOutcome {
            success: true,
            error_message: None,
        }
    }

    /// A write the backend rejected, with its own explanation of why.
    pub fn failure(error_message: impl Into<String>) -> WriteOutcome {
        WriteOutcome {
            success: false,
            error_message: Some(error_message.into()),
        }
    }
}

/// One node in a [`crate::Backend::browse`] result: either a leaf tag (readable/writable
/// directly) or a branch (has further children to browse into).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagNode {
    pub tag: TagId,
    pub is_branch: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_good_quality_is_trustworthy() {
        assert!(Quality::Good.is_trustworthy());
        assert!(!Quality::Uncertain.is_trustworthy());
        assert!(!Quality::Bad.is_trustworthy());
    }

    #[test]
    fn write_outcome_success_has_no_error_message() {
        let outcome = WriteOutcome::success();
        assert!(outcome.success);
        assert_eq!(outcome.error_message, None);
    }

    #[test]
    fn write_outcome_failure_carries_message() {
        let outcome = WriteOutcome::failure("tag is read-only");
        assert!(!outcome.success);
        assert_eq!(outcome.error_message.as_deref(), Some("tag is read-only"));
    }
}
