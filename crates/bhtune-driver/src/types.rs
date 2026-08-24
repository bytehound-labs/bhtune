//! Plain data types moved across the [`crate::Driver`] trait boundary.

use chrono::{DateTime, Utc};

/// An identifier for one tag/item a [`crate::Driver`] knows how to read or write. For OPC
/// DA this is the fully-qualified item name (e.g. `"Area1.LIC101.PV"`, matching
/// `bhtune_core::tags::LoopTags`'s tag fields exactly); other drivers may use any string
/// convention of their own, since only the implementing driver interprets it.
///
/// A plain `String` alias rather than a newtype: there is no invariant here worth enforcing
/// (a tag ID is valid or not only in the sense that the driver does or doesn't recognize
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

/// One tag's freshly read value, as returned by [`crate::Driver::read`].
///
/// `value` is a raw string, not a parsed `f32` — deliberately, since not every tag a
/// [`crate::Driver`] reads is numeric. Mode/direction/attribute tags hold small raw codes
/// (e.g. `"MAN"`, `"0"`) that `bhtune_core`'s own parsing functions interpret directly (see
/// `bhtune_core::ControllerDirection::from_raw_tag_value`), so this type must not assume
/// every tag's value is a number. Parsing a numeric tag's `value` into `f32` — and treating a
/// parse failure as an error rather than silently substituting a default — is left to the
/// caller that knows which of its requested tags are numeric, mirroring how
/// `opcda-bridge`'s own `Client::read` returns every value as a string for the same reason.
///
/// `timestamp` is `Option`, not a bare `DateTime<Utc>` — deliberately, since not every driver
/// can honestly supply one. OPC DA over the bridge, in particular, reports the item's last-
/// change time as a *local*, offset-less `"YYYY-MM-DD HH:MM:SS"` string, with `"N/A"`/
/// `"Invalid"` sentinels for items that have none (see `driver-opcda`'s `parse_timestamp`).
/// `None` when a driver cannot supply a trustworthy value, rather than a synthetic
/// stand-in — this field is diagnostic (e.g. detecting a frozen tag whose timestamp stops
/// advancing while its value doesn't change), never the tick time the tuning engine itself
/// runs on, which comes from the caller's own polling clock instead.
#[derive(Debug, Clone, PartialEq)]
pub struct TagValue {
    pub tag: TagId,
    pub value: String,
    pub quality: Quality,
    pub timestamp: Option<DateTime<Utc>>,
}

/// A value to write to a tag via [`crate::Driver::write`].
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

/// The result of one [`crate::Driver::write`] call that reached the driver.
///
/// Kept distinct from a [`crate::DriverError`]: a driver (or the DCS/PLC behind it)
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
    /// A write the driver accepted outright.
    pub fn success() -> WriteOutcome {
        WriteOutcome {
            success: true,
            error_message: None,
        }
    }

    /// A write the driver rejected, with its own explanation of why.
    pub fn failure(error_message: impl Into<String>) -> WriteOutcome {
        WriteOutcome {
            success: false,
            error_message: Some(error_message.into()),
        }
    }
}

/// How the OPC server organizes its namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NamespaceOrganization {
    Unspecified,
    Flat,
    Hierarchical,
}

/// Native or configured strategy that produced browse results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BrowseSource {
    Unspecified,
    Da3,
    Da2,
    Flat,
    Derived,
}

/// Whether a browse node is expandable, selectable as an OPC item, or both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BrowseNodeKind {
    Unspecified,
    Branch,
    Item,
    BranchAndItem,
}

impl BrowseNodeKind {
    pub fn is_branch(self) -> bool {
        matches!(self, Self::Branch | Self::BranchAndItem)
    }

    pub fn is_item(self) -> bool {
        matches!(self, Self::Item | Self::BranchAndItem)
    }
}

/// Gateway and namespace features reported for one OPC server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverCapabilities {
    pub application_version: String,
    pub protocol_version: String,
    pub max_page_size: u32,
    pub supports_browse_sessions: bool,
    pub supports_search: bool,
    pub organization: NamespaceOrganization,
    pub source: BrowseSource,
}

/// One child returned by a browse page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowseNode {
    /// Opaque navigation identity. Round-trip it unchanged when expanding.
    pub node_key: String,
    /// One local label suitable for display.
    pub display_name: String,
    pub kind: BrowseNodeKind,
    /// Exact OPC DA ItemID, present only for selectable nodes.
    pub item_id: Option<String>,
}

/// One bounded page of immediate children and its continuation metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowsePage {
    pub session_id: String,
    pub nodes: Vec<BrowseNode>,
    pub next_page_token: Option<String>,
    pub complete: bool,
    pub organization: NamespaceOrganization,
    pub source: BrowseSource,
    pub warning: Option<String>,
}

/// Parameters for one browse-page request. The connected driver supplies its configured
/// OPC server; callers only provide session/navigation state returned by earlier pages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowsePageRequest {
    pub session_id: Option<String>,
    pub parent_node_key: Option<String>,
    pub page_token: Option<String>,
    pub page_size: u32,
    pub refresh: bool,
}

impl BrowsePageRequest {
    /// Open a new browse session and request its root page.
    pub fn root(page_size: u32) -> Self {
        Self {
            session_id: None,
            parent_node_key: None,
            page_token: None,
            page_size,
            refresh: false,
        }
    }

    /// Request the first page beneath an already-discovered branch.
    pub fn children(
        session_id: impl Into<String>,
        parent_node_key: impl Into<String>,
        page_size: u32,
    ) -> Self {
        Self {
            session_id: Some(session_id.into()),
            parent_node_key: Some(parent_node_key.into()),
            page_token: None,
            page_size,
            refresh: false,
        }
    }

    /// Request the next page for a root or child browse.
    pub fn next(
        session_id: impl Into<String>,
        parent_node_key: Option<String>,
        page_token: impl Into<String>,
        page_size: u32,
    ) -> Self {
        Self {
            session_id: Some(session_id.into()),
            parent_node_key,
            page_token: Some(page_token.into()),
            page_size,
            refresh: false,
        }
    }

    pub fn with_refresh(mut self, refresh: bool) -> Self {
        self.refresh = refresh;
        self
    }
}

/// Match behavior for namespace search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SearchMatchMode {
    Exact,
    Prefix,
    Contains,
}

/// Parameters for a bounded namespace search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRequest {
    pub query: String,
    pub match_mode: SearchMatchMode,
    pub session_id: Option<String>,
    pub scope_node_key: Option<String>,
    pub max_results: u32,
    pub include_branches: bool,
    pub refresh: bool,
}

impl SearchRequest {
    pub fn new(query: impl Into<String>, match_mode: SearchMatchMode, max_results: u32) -> Self {
        Self {
            query: query.into(),
            match_mode,
            session_id: None,
            scope_node_key: None,
            max_results,
            include_branches: false,
            refresh: false,
        }
    }
}

/// One navigation step associated with a search match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowseBreadcrumb {
    pub node_key: String,
    pub display_name: String,
}

/// A progressively emitted namespace-search result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchMatch {
    pub node: BrowseNode,
    pub breadcrumbs: Vec<BrowseBreadcrumb>,
}

/// Progress emitted while a namespace search is still running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchProgress {
    pub visited_nodes: u32,
    pub matches: u32,
    pub partial: bool,
}

/// Terminal search metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchCompleted {
    pub complete: bool,
    pub cancelled: bool,
    pub truncated: bool,
    pub warning: Option<String>,
}

/// One event from the driver's namespace-search stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchEvent {
    Match(SearchMatch),
    Progress(SearchProgress),
    Completed(SearchCompleted),
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
